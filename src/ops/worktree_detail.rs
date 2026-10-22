//! Ops: `worktree detail`, everything the Neovim client's detail float shows about a worktree
//! (`domain/detail.rs`). Only reads: it runs git to count the drift, and fetches nothing. The step
//! of a rebase in progress is read from git's own files, with no git run.
//!
//! A worktree whose directory is gone still gives its state and the reason; it has no denials,
//! setup or drift to show, since git can no longer name its admin directory or its `HEAD`.

use std::path::Path;

use anyhow::Result;

use crate::adapters::{env, fs, git};
use crate::domain::branch::StartPoint;
use crate::domain::detail::{self, Detail, Drift, SetupDetail, SetupProgress};
use crate::domain::fleet::PrimaryCheckout;
use crate::domain::operation::{Operation, RebaseStep};
use crate::domain::setup;
use crate::domain::state::{AdminDir, State};
use crate::domain::worktree::Target;
use crate::ops::{find_target, read_fleet, read_rebase_step, read_recorded, read_status};

/// How much of the end of the setup log is read: far more than the lines shown.
const SETUP_LOG_TAIL: u64 = 64 * 1024;

/// The detail of the worktree `target` names or locates in the repository containing the current
/// directory. Refuses the primary checkout and an unknown worktree.
pub(crate) fn run(target: &Target) -> Result<Detail> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = find_target(&fleet, &cwd, target)?.clone();
    if worktree.is_gone() {
        return Ok(Detail {
            why: worktree.state.detail(),
            worktree,
            denials: Vec::new(),
            setup: None,
            drift: None,
            operation_step: None,
        });
    }
    let admin = git::admin_dir(&worktree.path)?;
    let status = read_status(&admin);
    Ok(Detail {
        why: detail::whole_detail(&worktree.state, status.as_ref()),
        denials: status
            .as_ref()
            .map(|status| status.latest_denials().to_vec())
            .unwrap_or_default(),
        setup: read_setup(&admin, &worktree.state)?,
        drift: read_drift(&fleet.primary, &worktree.path)?,
        operation_step: read_step(&admin, worktree.operation),
        worktree,
    })
}

/// The latest setup of the worktree whose admin directory is `admin`, in `state`, with the end of
/// its log. `None` when it never ran a setup, or when its state file cannot be read (the state
/// then says why).
fn read_setup(admin: &AdminDir, state: &State) -> Result<Option<SetupDetail>> {
    let Ok(recorded) = read_recorded(admin) else {
        return Ok(None);
    };
    let Some(record) = recorded.setup else {
        return Ok(None);
    };
    let log = admin.setup_log();
    let tail = match fs::read_tail(&log, SETUP_LOG_TAIL)? {
        Some(bytes) => {
            let cut = u64::try_from(bytes.len()).is_ok_and(|len| len >= SETUP_LOG_TAIL);
            setup::log_tail(&bytes, cut)
        }
        None => Vec::new(),
    };
    Ok(Some(SetupDetail {
        progress: SetupProgress::of(record, state),
        log,
        tail,
    }))
}

/// How far the `HEAD` of the worktree at `worktree` is from the repository's local default
/// branch: `git merge-base`, then `git rev-list --left-right --count`. `None` when there is no
/// local default branch (`origin/` is never used: nothing is fetched, and the local branch is the
/// one the person merges into) or when the histories share no commit.
fn read_drift(primary: &PrimaryCheckout, worktree: &Path) -> Result<Option<Drift>> {
    let Some(branch) = git::local_default_branch(primary)? else {
        return Ok(None);
    };
    if git::merge_base(worktree, &StartPoint::Local(branch.clone()))?.is_none() {
        return Ok(None);
    }
    let counts = git::left_right_counts(worktree, &branch)?;
    Drift::from_counts(branch, &counts).map(Some)
}

/// The step of the rebase the worktree whose admin directory is `admin` is in the middle of, when
/// its `operation` is a rebase. Only reads.
fn read_step(admin: &AdminDir, operation: Option<Operation>) -> Option<RebaseStep> {
    match operation {
        Some(Operation::Rebase) => read_rebase_step(admin),
        Some(Operation::Merge | Operation::CherryPick | Operation::Revert) | None => None,
    }
}
