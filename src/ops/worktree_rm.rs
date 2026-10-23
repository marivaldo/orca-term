//! Ops: `worktree rm`, which removes a worktree. Its branch goes too only when merged into the
//! default branch, or with `--force`. A worktree whose lock is held (its setup or another
//! orca-term command is running on it) is refused as `busy`, even with `--force`. A worktree with
//! modified or untracked files is refused as `dirty` unless `--force`.
//!
//! A running agent is stopped first, as `agent stop` stops it (#94), once every rule above has
//! passed, so a refused removal never stops it. The stop happens under the worktree's lock
//! (`agent stop` takes none, so this cannot deadlock), and the dirty rule is checked again after
//! it: files the agent wrote while stopping refuse the removal as `dirty`, the agent staying
//! stopped. An agent that does not stop refuses the removal as `busy` and nothing is removed.
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
use crate::domain::snapshot;
use crate::domain::state::{AdminDir, LockStatus};
use crate::domain::worktree::{
    self, BranchFate, BranchOutcome, KeptReason, Removal, Removed, Target,
};
use crate::ops::agent_stop::stop_holder;
use crate::ops::{Changed, find_target, lock_worktree, read_fleet};

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
    check_clean(worktree, removal, false)?;
    let primary = &fleet.primary;

    // Decided before the worktree goes, so a failure here leaves everything in place.
    let default = git::default_branch(primary).ok();
    let planned = match worktree::branch_fate(worktree.branch.as_ref(), default.as_ref(), removal) {
        BranchFate::Decided(outcome) => outcome,
        BranchFate::IfMerged { branch } => outcome_if_merged(primary, branch)?,
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
