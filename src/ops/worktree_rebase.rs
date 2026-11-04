//! Ops: `worktree rebase`, which rebases a worktree's branch onto its base as Orca's **Rebase from
//! `<base>`** does (#141, `domain/rebase.rs`), and `worktree rebase --abort`, which gives a rebase
//! in progress up. The only code that fetches (`cargo xtask gh-guard`).
//!
//! It refuses first, under the worktree's lock, changing nothing: its agent in a turn (`busy`), a
//! rebase, merge, cherry-pick or revert already in progress (`operation_in_progress`), files with
//! changes not committed (`dirty`, listing them; nothing is stashed), and a repository with no
//! default branch on `origin` (`no_remote_base`). Then it fetches that branch into a private
//! temporary ref, so a fetch running at the same time cannot move it, and rebases onto it: from
//! the fork point when git finds one (`merge-base --fork-point`), else a plain rebase. A branch
//! that already holds the base is left alone. The temporary ref is deleted whatever happened.
//!
//! A rebase that stops on conflicts stays in progress, as in Orca, and is the typed
//! `rebase_conflict`, naming the files: the person sends them to the agent (`agent resolve`) or
//! aborts it.

use anyhow::{Context, Result, bail};

use crate::adapters::git::{self, Fetched, RebaseRun};
use crate::adapters::{env, fs::WorktreeLock};
use crate::domain::error::{BusyReason, RebaseAsk, TypedError};
use crate::domain::fleet::{PrimaryCheckout, Worktree};
use crate::domain::operation::Operation;
use crate::domain::rebase::{self, Aborted, Outcome, Rebased, RemoteBase};
use crate::domain::worktree::Target;
use crate::ops::agent_swap::check_not_in_turn;
use crate::ops::{find_target, lock_worktree, notify_changed, read_fleet, read_operation};

/// Rebases the branch of the worktree `target` names or locates in the repository containing the
/// current directory onto the default branch on `origin`, then tells the running Neovims.
pub(crate) fn run(target: &Target) -> Result<Rebased> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = find_target(&fleet, &cwd, target)?;
    check_present(worktree)?;
    let lock = lock_worktree(&worktree.name, &worktree.path)?;
    check_not_in_turn(worktree, BusyReason::RebaseInTurn)?;
    check_no_operation(worktree)?;
    check_clean(worktree)?;
    let base = find_base(&fleet.primary, worktree)?;

    let temp = temp_ref(&lock, worktree);
    let outcome = fetch_and_rebase(worktree, &base, &temp);
    // Whatever happened: the temporary ref is never left behind.
    let deleted = git::delete_ref(&fleet.primary.path, &temp);
    notify_changed(lock.admin());
    let outcome = outcome?;
    deleted.with_context(|| {
        format!(
            "worktree {} was rebased, but its temporary ref {temp} could not be deleted",
            worktree.name
        )
    })?;
    Ok(Rebased {
        name: worktree.name.clone(),
        path: worktree.path.clone(),
        base: base.shown(),
        outcome,
    })
}

/// Aborts the rebase in progress in the worktree `target` names or locates (`git rebase
/// --abort`), putting its branch back where it was, then tells the running Neovims. Refuses, as
/// `no_rebase`, a worktree with no rebase in progress.
pub(crate) fn abort(target: &Target) -> Result<Aborted> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = find_target(&fleet, &cwd, target)?;
    check_present(worktree)?;
    let lock = lock_worktree(&worktree.name, &worktree.path)?;
    if worktree.operation != Some(Operation::Rebase) {
        return Err(TypedError::NoRebase {
            name: worktree.name.clone(),
            asked: RebaseAsk::Abort,
        }
        .into());
    }
    git::rebase_abort(&worktree.path)
        .with_context(|| format!("could not abort the rebase of worktree {}", worktree.name))?;
    notify_changed(lock.admin());
    Ok(Aborted {
        name: worktree.name.clone(),
        path: worktree.path.clone(),
    })
}

/// Refuses a worktree whose directory is gone.
fn check_present(worktree: &Worktree) -> Result<()> {
    if worktree.is_gone() {
        bail!(
            "the directory of worktree {} is gone; run orca-term worktree prune",
            worktree.name
        );
    }
    Ok(())
}

/// Refuses, as `operation_in_progress`, a worktree in the middle of a rebase, a merge, a
/// cherry-pick or a revert.
fn check_no_operation(worktree: &Worktree) -> Result<()> {
    if let Some(operation) = worktree.operation {
        return Err(TypedError::OperationInProgress {
            name: worktree.name.clone(),
            operation: operation.name(),
        }
        .into());
    }
    Ok(())
}

/// Refuses, as `dirty`, a worktree with modified or untracked files, naming them. Nothing is ever
/// stashed for a rebase.
fn check_clean(worktree: &Worktree) -> Result<()> {
    let files = rebase::uncommitted_files(&git::uncommitted_status(&worktree.path)?);
    if !files.is_empty() {
        return Err(TypedError::RebaseDirty {
            name: worktree.name.clone(),
            path: worktree.path.clone(),
            files,
        }
        .into());
    }
    Ok(())
}

/// The base to rebase onto: the default branch on `origin`. Refuses, as `no_remote_base`, a
/// repository that has none; a local branch is never used instead.
fn find_base(primary: &PrimaryCheckout, worktree: &Worktree) -> Result<RemoteBase> {
    match git::remote_default_branch(primary)? {
        Some(branch) => Ok(RemoteBase { branch }),
        None => Err(TypedError::NoRemoteBase {
            name: worktree.name.clone(),
        }
        .into()),
    }
}

/// The temporary ref this rebase fetches its base into, named after the worktree's git id (its
/// name when git gives none) and the time.
fn temp_ref(lock: &WorktreeLock, worktree: &Worktree) -> String {
    let id = lock.admin().worktree_id().unwrap_or(&worktree.name);
    rebase::temp_ref(id, env::now_millis())
}

/// Fetches `base` into `temp`, then rebases the branch of `worktree` onto it. A branch that
/// already holds every commit of the base is left alone.
fn fetch_and_rebase(worktree: &Worktree, base: &RemoteBase, temp: &str) -> Result<Outcome> {
    match git::fetch_into(&worktree.path, &base.refspec_into(temp))? {
        Fetched::Done => {}
        Fetched::Failed(reason) => bail!(
            "could not fetch {} for worktree {}: {reason} (nothing was rebased)",
            base.shown(),
            worktree.name
        ),
    }
    if git::is_ancestor_of_head(&worktree.path, temp)? {
        return Ok(Outcome::UpToDate);
    }
    let fork_point = git::fork_point(&worktree.path, &base.tracking_ref())?;
    match git::rebase_onto(&worktree.path, temp, fork_point.as_ref())? {
        RebaseRun::Done => Ok(Outcome::Rebased {
            replayed: git::count_ahead_of(&worktree.path, temp)?,
        }),
        RebaseRun::Stopped(said) => Err(stopped(worktree, base, &said)),
    }
}

/// Why a rebase git stopped did not go through: the typed `rebase_conflict`, naming the files in
/// conflict, while the rebase stays in progress; otherwise git's own words.
fn stopped(worktree: &Worktree, base: &RemoteBase, said: &str) -> anyhow::Error {
    let in_progress = git::admin_dir(&worktree.path)
        .ok()
        .and_then(|admin| read_operation(&admin))
        == Some(Operation::Rebase);
    if !in_progress {
        return anyhow::anyhow!(
            "git rebase failed for worktree {}: {said} (nothing was rebased)",
            worktree.name
        );
    }
    let files = match git::conflicted_files(&worktree.path) {
        Ok(raw) => rebase::paths(&raw),
        Err(err) => return err.context("the rebase stopped, and its conflicts could not be read"),
    };
    TypedError::RebaseConflict {
        name: worktree.name.clone(),
        base: base.shown(),
        files,
    }
    .into()
}
