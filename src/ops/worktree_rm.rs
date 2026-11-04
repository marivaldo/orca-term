//! Ops: `worktree rm`, which removes a worktree. Its branch goes too only when merged into the
//! default branch, or when its pull request is merged on GitHub with exactly the branch's tip as
//! its head (#137), or with `--force`. A worktree whose lock is held (its setup or another
//! orca-term command is running on it) is refused as `busy`, even with `--force`. A worktree with
//! modified or untracked files is refused as `dirty` unless `--force`.
//!
//! A running agent is stopped first, as `agent stop` stops it (#94), once every rule above has
//! passed, so a refused removal never stops it. The stop happens under the worktree's lock
//! (`agent stop` takes none, so this cannot deadlock), and the dirty rule is checked again after
//! it: files the agent wrote while stopping refuse the removal as `dirty`, the agent staying
//! stopped. An agent that does not stop refuses the removal as `busy` and nothing is removed.
//!
//! The pull request is read from the worktree's `pr.json`, the cache `pr refresh`, `pr create` and
//! `pr merge` keep: the removal never asks GitHub. A cache with no head commit (written before
//! #137) never proves a merge, and a branch with a commit after the merged head is kept.
//!
//! A branch deleted as merged is deleted only at the tip that was checked (#140): an agent that
//! commits while it stops moves the branch past that tip, and git then refuses the deletion
//! atomically, so the branch is kept with its new commits. `--force` deletes whatever the tip.
//!
//! Once the worktree is removed, its chain of snapshots (`refs/orca-term/worktrees/<id>`,
//! `domain/snapshot.rs`) is deleted too: that ref does not die with the admin directory.

use anyhow::{Context, Result};

use crate::adapters::{env, fs, git};
use crate::domain::branch::Branch;
use crate::domain::error::BusyReason;
use crate::domain::error::TypedError;
use crate::domain::fleet::PrimaryCheckout;
use crate::domain::fleet::Worktree;
use crate::domain::pr::{self, Found};
use crate::domain::review::Commit;
use crate::domain::snapshot;
use crate::domain::state::{AdminDir, LockStatus};
use crate::domain::worktree::{
    self, BranchFate, BranchOutcome, KeptReason, Removal, Removed, Target,
};
use crate::ops::agent_stop::stop_holder;
use crate::ops::{Changed, find_target, lock_worktree, read_fleet, read_pr};

/// Removes the worktree `target` names or locates in the repository containing the current
/// directory, stopping its agent first when one runs, then tells the running Neovims it is gone.
pub(crate) fn run(target: &Target, removal: Removal) -> Result<Removed> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = find_target(&fleet, &cwd, target)?;
    worktree::check_removable(worktree)?;
    // Held until the worktree is gone, so no setup can start in it meanwhile.
    let lock = lock_worktree(&worktree.name, &worktree.path)?;
    // Found while git's files are still there: the removal takes them away.
    let changed = Changed::of(lock.admin());
    let chain = lock.admin().worktree_id().map(snapshot::chain_ref);
    // Read now: the removal deletes the admin directory that holds it.
    let cached_pr = read_pr(lock.admin()).usable().map(|facts| facts.found);
    check_clean(worktree, removal, false)?;
    let primary = &fleet.primary;

    // Decided before the worktree goes, so a failure here leaves everything in place.
    let default = git::default_branch(primary).ok();
    let planned = match worktree::branch_fate(worktree.branch.as_ref(), default.as_ref(), removal) {
        BranchFate::Decided(outcome) => PlannedBranch {
            outcome,
            checked_tip: None,
        },
        BranchFate::IfMerged { branch } => plan_if_merged(primary, branch, cached_pr.as_ref())?,
    };

    // Every rule has passed: only now is the agent stopped, so a refusal leaves it running. Read
    // under the lock, so no `agent start` can slip in between.
    let stopped_agent = holder_alive(lock.admin())?;
    if stopped_agent {
        stop_agent(worktree, lock.admin(), primary)?;
        check_clean(worktree, removal, true)?;
    }

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
    if let Some(chain) = &chain {
        // The worktree is gone whatever happens here; a chain left behind is an orphan that
        // `worktree prune` deletes.
        let _ = git::delete_ref(&primary.path, chain);
    }
    let branch = delete_branch(primary, planned);
    if let Some(changed) = changed {
        changed.notify();
    }
    Ok(Removed {
        name: worktree.name.clone(),
        path: worktree.path.clone(),
        branch,
        stopped_agent,
    })
}

/// Refuses as `dirty` a worktree with modified or untracked files, unless `removal` is forced.
/// `stopped_agent` says its agent was stopped already, which the refusal then says.
fn check_clean(worktree: &Worktree, removal: Removal, stopped_agent: bool) -> Result<()> {
    if removal == Removal::Safe && git::has_changes(&worktree.path)? {
        return Err(TypedError::Dirty {
            name: worktree.name.clone(),
            path: worktree.path.clone(),
            stopped_agent,
        }
        .into());
    }
    Ok(())
}

/// Whether a holder is alive in `admin`: it holds `agent.lock`.
fn holder_alive(admin: &AdminDir) -> Result<bool> {
    Ok(fs::lock_status_at(&admin.agent_lock())? == LockStatus::Held)
}

/// Stops the agent of `worktree` as `agent stop` does, waiting until its holder is gone. An agent
/// that does not stop is the typed `busy`, with what went wrong.
///
/// Talking to the holder moves this process into the worktree's admin directory, which the
/// removal deletes, and git refuses to delete the directory it was started in. So this returns
/// to `primary` afterwards, which no removal touches.
fn stop_agent(worktree: &Worktree, admin: &AdminDir, primary: &PrimaryCheckout) -> Result<()> {
    let stopped = stop_holder(admin);
    env::enter_dir(&primary.path)?;
    stopped.map_err(|err| {
        anyhow::Error::new(TypedError::Busy {
            name: worktree.name.clone(),
            path: worktree.path.clone(),
            reason: BusyReason::AgentDidNotStop,
        })
        .context(format!("{err:#}"))
    })
}

/// What the removal plans for the worktree's branch, decided before the agent stops: the outcome,
/// and, when the branch is to be deleted as merged, the tip that was checked (#140). Only that
/// tip is deleted: a branch that moved past it is kept.
struct PlannedBranch {
    outcome: BranchOutcome,
    /// The commit the merge checks looked at; `None` when no check ran (`--force`, a kept or
    /// detached branch).
    checked_tip: Option<Commit>,
}

/// What happens to `branch` when it goes only if merged: git says whether its tip is merged into
/// the default branch; failing that, `cached_pr` (what `pr.json` holds) may prove its pull request
/// merged at that tip. A default branch git cannot find keeps the branch, for that reason.
fn plan_if_merged(
    primary: &PrimaryCheckout,
    branch: Branch,
    cached_pr: Option<&Found>,
) -> Result<PlannedBranch> {
    let start = match git::start_point(primary) {
        Ok(start) => start,
        Err(err) => {
            return Ok(kept(
                branch,
                KeptReason::NoDefaultBranch(format!("{err:#}")),
            ));
        }
    };
    // Read once: every check below looks at this commit, and only this commit may be deleted.
    let Some(tip) = git::read_ref(&primary.path, &branch_ref(&branch))? else {
        return Ok(kept(branch, KeptReason::NotMerged { into: start }));
    };
    let merged_pr = cached_pr.and_then(|found| pr::merged_as(found, tip.as_str()));
    let outcome = if git::is_merged(primary, &tip, &start)? {
        BranchOutcome::DeletedMerged {
            branch,
            into: start,
        }
    } else if let Some(number) = merged_pr {
        BranchOutcome::DeletedMergedPr { branch, number }
    } else {
        return Ok(kept(branch, KeptReason::NotMerged { into: start }));
    };
    Ok(PlannedBranch {
        outcome,
        checked_tip: Some(tip),
    })
}

/// The plan that keeps `branch`, for `reason`.
fn kept(branch: Branch, reason: KeptReason) -> PlannedBranch {
    PlannedBranch {
        outcome: BranchOutcome::Kept { branch, reason },
        checked_tip: None,
    }
}

/// The full name of the ref of `branch`: `refs/heads/<branch>`.
fn branch_ref(branch: &Branch) -> String {
    format!("refs/heads/{branch}")
}

/// Deletes the branch when `planned` says it goes, and returns the planned outcome, or why the
/// branch was kept when git refuses. A branch with a checked tip is deleted only at that tip.
fn delete_branch(primary: &PrimaryCheckout, planned: PlannedBranch) -> BranchOutcome {
    let PlannedBranch {
        outcome,
        checked_tip,
    } = planned;
    let (BranchOutcome::DeletedMerged { branch, .. }
    | BranchOutcome::DeletedMergedPr { branch, .. }
    | BranchOutcome::DeletedForced { branch }) = &outcome
    else {
        return outcome;
    };
    let deleted = match &checked_tip {
        Some(tip) => git::delete_branch_at(primary, branch, tip),
        None => git::delete_branch(primary, branch),
    };
    match deleted {
        Ok(()) => outcome,
        Err(err) => BranchOutcome::Kept {
            branch: branch.clone(),
            reason: refusal_reason(primary, branch, checked_tip.as_ref(), &err),
        },
    }
}

/// Why git refused to delete `branch`: it moved past `checked_tip`, or `err` otherwise.
fn refusal_reason(
    primary: &PrimaryCheckout,
    branch: &Branch,
    checked_tip: Option<&Commit>,
    err: &anyhow::Error,
) -> KeptReason {
    if let Some(checked) = checked_tip {
        let now = git::read_ref(&primary.path, &branch_ref(branch));
        if matches!(&now, Ok(Some(now)) if now != checked) {
            return KeptReason::MovedWhileStopping;
        }
    }
    KeptReason::DeleteFailed(format!("{err:#}"))
}
