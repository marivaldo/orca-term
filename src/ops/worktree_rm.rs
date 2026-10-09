//! Ops: `worktree rm`, which removes a worktree. Its branch goes too only when merged into the
//! default branch, or with `--force`.

use std::path::Path;

use anyhow::{Context, Result, bail};

use crate::adapters::{env, fs, git};
use crate::domain::fleet::{Fleet, Worktree};
use crate::domain::worktree::{self, BranchFate, BranchOutcome, Removed};
use crate::ops::read_fleet;

/// Removes the worktree named or located by `target` in the repository containing the current
/// directory.
pub(crate) fn run(target: &str, force: bool) -> Result<Removed> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = resolve(&fleet, &cwd, target)?;
    worktree::check_removable(worktree)?;
    let primary = &fleet.primary.path;

    // Decided before the worktree goes, so a failure here leaves everything in place.
    let default = git::default_branch(primary).ok();
    let planned = match worktree::branch_fate(worktree.branch.as_deref(), default.as_deref(), force)
    {
        BranchFate::Decided(outcome) => outcome,
        BranchFate::IfMerged { branch } => outcome_if_merged(primary, branch)?,
    };

    git::worktree_remove(primary, &worktree.path, force).with_context(|| {
        let hint = if force {
            String::new()
        } else {
            "; pass --force to remove it anyway, discarding its changes".to_owned()
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
fn resolve<'f>(fleet: &'f Fleet, dir: &Path, target: &str) -> Result<&'f Worktree> {
    if !worktree::target_is_path(target) {
        return worktree::find_by_name(fleet, target);
    }
    let wanted = dir.join(target);
    let primary = &fleet.primary.path;
    if fs::same_path(primary, &wanted) {
        bail!(
            "{} is the primary checkout, which is never a worktree",
            primary.display()
        );
    }
    match fleet
        .worktrees
        .iter()
        .find(|worktree| fs::same_path(&worktree.path, &wanted))
    {
        Some(worktree) => Ok(worktree),
        None => bail!("{} is not a worktree of this repository", wanted.display()),
    }
}

/// What happens to `branch` when it goes only if merged: git says whether it is merged into the
/// default branch. A default branch git cannot find keeps the branch, for that reason.
fn outcome_if_merged(primary: &Path, branch: String) -> Result<BranchOutcome> {
    let start = match git::start_point(primary) {
        Ok(start) => start,
        Err(err) => {
            return Ok(BranchOutcome::Kept {
                branch,
                reason: format!("{err:#}"),
            });
        }
    };
    let merged = git::is_merged(primary, &branch, &start)?;
    Ok(worktree::merged_outcome(branch, start, merged))
}

/// Deletes the branch when `planned` says it goes, and returns `planned`, or why the branch was
/// kept when git refuses.
fn delete_branch(primary: &Path, planned: BranchOutcome) -> BranchOutcome {
    let (BranchOutcome::DeletedMerged { branch, .. } | BranchOutcome::DeletedForced { branch }) =
        &planned
    else {
        return planned;
    };
    match git::delete_branch(primary, branch) {
        Ok(()) => planned,
        Err(err) => BranchOutcome::Kept {
            branch: branch.clone(),
            reason: format!("could not delete it: {err:#}"),
        },
    }
}
