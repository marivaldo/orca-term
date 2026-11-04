//! Ops: `worktree push`, which pushes a worktree's branch to `origin` from the core, outside any
//! agent's sandbox, as Orca's **Push** does, or with a lease as its **Force push with lease**
//! does (#146, `domain/push.rs`). One of the two ops that push, with `pr create` (`cargo xtask
//! gh-guard`).
//!
//! It refuses first, pushing nothing: a worktree whose directory is gone, a worktree in the middle
//! of a rebase or on no branch, and a repository with no `origin`. Then it runs `git push -u origin
//! <branch>`, which never forces: a branch that diverged from `origin/<branch>` is the typed
//! `push_diverged`. With a lease it runs `git push --force-with-lease=<branch>:<sha> -u origin
//! <branch>`, `<sha>` being what `refs/remotes/origin/<branch>` holds, so git refuses when someone
//! else pushed since; that refusal, like any other failure, is `push_failed` with git's words.
//!
//! Files with changes not committed do not stop it: they are left out, and reported, as `pr
//! create` does. After a push it reads the worktree's pull request again, as `pr refresh` does,
//! so the row's checks show the new run.

use anyhow::{Context, Result, bail};

use crate::adapters::env;
use crate::adapters::git::{self, PushMode};
use crate::domain::branch::Branch;
use crate::domain::error::TypedError;
use crate::domain::fleet::{PrimaryCheckout, Worktree};
use crate::domain::operation::Operation;
use crate::domain::pr::create;
use crate::domain::push::{self, Outcome, PushRun, Pushed};
use crate::domain::worktree::Target;
use crate::ops::{find_target, pr_refresh, read_fleet};

/// Pushes the branch of the worktree `target` names or locates in the repository containing the
/// current directory to `origin`: plainly, or with a lease when `with_lease`.
pub(crate) fn run(target: &Target, with_lease: bool) -> Result<Pushed> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = find_target(&fleet, &cwd, target)?;
    let branch = check_pushable(worktree)?;
    check_origin(&fleet.primary, worktree)?;
    let uncommitted = create::count_uncommitted(&git::uncommitted_status(&worktree.path)?);

    let outcome = push_to_origin(worktree, branch, with_lease)?;
    let facts = pr_refresh::refresh(worktree).with_context(|| {
        format!(
            "branch {branch} was pushed to origin, but the pull request of worktree {} could not \
             be read again",
            worktree.name
        )
    })?;
    Ok(Pushed {
        name: worktree.name.clone(),
        path: worktree.path.clone(),
        branch: branch.clone(),
        outcome,
        with_lease,
        uncommitted,
        facts,
    })
}

/// The branch of `worktree`, to push. Refuses a worktree whose directory is gone, one in the
/// middle of a rebase and one on no branch (a detached `HEAD`).
fn check_pushable(worktree: &Worktree) -> Result<&Branch> {
    let name = &worktree.name;
    if worktree.is_gone() {
        bail!("the directory of worktree {name} is gone; run orca-term worktree prune");
    }
    if worktree.operation == Some(Operation::Rebase) {
        bail!(
            "worktree {name} is in the middle of a rebase, so its branch is not pushed; finish or \
             abort the rebase first (nothing was pushed)"
        );
    }
    match &worktree.branch {
        Some(branch) => Ok(branch),
        None => bail!("worktree {name} is on no branch, so there is nothing to push"),
    }
}

/// Refuses a repository with no remote named `origin`, the only remote a worktree is pushed to.
fn check_origin(primary: &PrimaryCheckout, worktree: &Worktree) -> Result<()> {
    if !git::has_origin(primary)? {
        bail!(
            "worktree {} cannot be pushed: this repository has no `origin` remote (nothing was \
             pushed)",
            worktree.name
        );
    }
    Ok(())
}

/// Pushes `branch` of `worktree` to `origin`, with a lease on what git last saw there when
/// `with_lease`. A diverged branch is the typed `push_diverged`; any other refusal `push_failed`.
fn push_to_origin(worktree: &Worktree, branch: &Branch, with_lease: bool) -> Result<Outcome> {
    let seen = if with_lease {
        git::read_ref(&worktree.path, &push::tracking_ref(branch))?
    } else {
        None
    };
    let mode = if with_lease {
        PushMode::WithLease {
            expected: seen.as_ref(),
        }
    } else {
        PushMode::Plain
    };
    match git::push_branch(&worktree.path, branch, mode)? {
        PushRun::Pushed => Ok(Outcome::Pushed),
        PushRun::UpToDate => Ok(Outcome::UpToDate),
        PushRun::Diverged(_) if !with_lease => Err(TypedError::PushDiverged {
            name: worktree.name.clone(),
            branch: branch.to_string(),
        }
        .into()),
        PushRun::Diverged(reason) | PushRun::Failed(reason) => {
            Err(TypedError::WorktreePushFailed {
                name: worktree.name.clone(),
                branch: branch.to_string(),
                reason,
            }
            .into())
        }
    }
}
