//! Ops: `worktree rm`, which removes a worktree. Its branch goes too only when merged into the
//! default branch, or with `--force`. A worktree whose lock is held (its setup or another
//! orca-term command is running on it) is refused, even with `--force`.

use anyhow::{Context, Result};

use crate::adapters::{env, git};
use crate::domain::branch::Branch;
use crate::domain::fleet::PrimaryCheckout;
use crate::domain::worktree::{
    self, BranchFate, BranchOutcome, KeptReason, Removal, Removed, Target,
};
use crate::ops::{find_target, lock_worktree, read_fleet};

/// Removes the worktree `target` names or locates in the repository containing the current
/// directory.
pub(crate) fn run(target: &Target, removal: Removal) -> Result<Removed> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = find_target(&fleet, &cwd, target)?;
    worktree::check_removable(worktree)?;
    // Held until the worktree is gone, so no setup can start in it meanwhile.
    let _lock = lock_worktree(&worktree.name, &worktree.path)?;
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
