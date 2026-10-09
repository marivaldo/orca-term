//! Ops: `worktree rm`, which removes a worktree. Its branch goes too only when merged into the
//! default branch, or with `--force`.

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::adapters::{env, fs, git};
use crate::domain::branch::Branch;
use crate::domain::fleet::{Fleet, PrimaryCheckout, Worktree};
use crate::domain::worktree::{
    self, BranchFate, BranchOutcome, KeptReason, Removal, Removed, Target,
};
use crate::ops::read_fleet;

/// Removes the worktree `target` names or locates in the repository containing the current
/// directory.
pub(crate) fn run(target: &Target, removal: Removal) -> Result<Removed> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = resolve(&fleet, &cwd, target)?;
    worktree::check_removable(worktree)?;
    let primary = &fleet.primary;

    // Decided before the worktree goes, so a failure here leaves everything in place.
    let default = git::default_branch(primary).ok();
    let planned = match worktree::branch_fate(worktree.branch.as_ref(), default.as_ref(), removal) {
        BranchFate::Decided(outcome) => outcome,
        BranchFate::IfMerged { branch } => outcome_if_merged(primary, branch)?,
    };

    git::worktree_remove(primary, &worktree.path, removal).with_context(|| {
        let hint = match removal {
            Removal::Safe => "; pass --force to remove it anyway, discarding its changes",
            Removal::Forced => "",
        };
        format!(
            "could not remove worktree {}{hint}",
            worktree.path.display()
        )
    })?;
    Ok(Removed {
        name: worktree.name.clone(),
        path: worktree.path.clone(),
        branch: delete_branch(primary, planned),
    })
}

/// The worktree `target` names (a basename) or locates (a path, relative to `dir`).
fn resolve<'f>(fleet: &'f Fleet, dir: &Path, target: &Target) -> Result<&'f Worktree> {
    match target {
        Target::Name(name) => worktree::find_by_name(fleet, name),
        Target::Path(path) => find_by_path(fleet, &dir.join(path)),
    }
}

/// The worktree at `wanted`, compared through symlinks. Refuses the primary checkout and a path
/// that is no worktree of the fleet.
fn find_by_path<'f>(fleet: &'f Fleet, wanted: &Path) -> Result<&'f Worktree> {
    let primary = &fleet.primary.path;
    if fs::same_path(primary, wanted) {
        bail!(
            "{} is the primary checkout, which is never a worktree",
            primary.display()
        );
    }
    match fleet
        .worktrees
        .iter()
        .find(|worktree| fs::same_path(&worktree.path, wanted))
    {
        Some(worktree) => Ok(worktree),
        None => bail!("{} is not a worktree of this repository", wanted.display()),
    }
}

/// What happens to `branch` when it goes only if merged: git says whether it is merged into the
/// default branch. A default branch git cannot find keeps the branch, for that reason.
fn outcome_if_merged(primary: &PrimaryCheckout, branch: Branch) -> Result<BranchOutcome> {
    let start = match git::start_point(primary) {
        Ok(start) => start,
        Err(err) => {
            return Ok(BranchOutcome::Kept {
                branch,
                reason: KeptReason::NoDefaultBranch(format!("{err:#}")),
            });
        }
    };
    if git::is_merged(primary, &branch, &start)? {
        Ok(BranchOutcome::DeletedMerged {
            branch,
            into: start,
        })
    } else {
        Ok(BranchOutcome::Kept {
            branch,
            reason: KeptReason::NotMerged { into: start },
        })
    }
}

/// Deletes the branch when `planned` says it goes, and returns `planned`, or why the branch was
/// kept when git refuses.
fn delete_branch(primary: &PrimaryCheckout, planned: BranchOutcome) -> BranchOutcome {
    let (BranchOutcome::DeletedMerged { branch, .. } | BranchOutcome::DeletedForced { branch }) =
        &planned
    else {
        return planned;
    };
    match git::delete_branch(primary, branch) {
        Ok(()) => planned,
        Err(err) => BranchOutcome::Kept {
            branch: branch.clone(),
            reason: KeptReason::DeleteFailed(format!("{err:#}")),
        },
    }
}
