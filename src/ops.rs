//! Ops: one module per command, each a short sequence of adapter and domain calls. This file holds
//! what several commands share: reading the fleet.

pub(crate) mod worktree_ls;
pub(crate) mod worktree_new;
pub(crate) mod worktree_prune;
pub(crate) mod worktree_rm;

use std::path::Path;

use anyhow::Result;

use crate::adapters::{fs, git};
use crate::domain::fleet::Fleet;
use crate::domain::state::{self, BrokenReason, State};

/// Reads the fleet of the repository containing `dir`, with each worktree's state. Never writes.
pub(crate) fn read_fleet(dir: &Path) -> Result<Fleet> {
    let entries = git::worktree_list(dir)?;
    let prunable: Vec<Option<String>> = entries
        .iter()
        .skip(1)
        .map(|entry| entry.prunable.clone())
        .collect();
    let mut fleet = Fleet::from_entries(entries)?;
    for (worktree, prunable) in fleet.worktrees.iter_mut().zip(prunable) {
        worktree.state = read_state(&worktree.path, prunable);
    }
    Ok(fleet)
}

/// The state of the worktree at `worktree`, which git may mark `prunable`. A worktree whose
/// directory is gone, or whose state cannot be read, is broken: reading one worktree never fails
/// the fleet.
fn read_state(worktree: &Path, prunable: Option<String>) -> State {
    if !fs::is_dir(worktree) {
        return State::Broken(BrokenReason::DirectoryMissing);
    }
    if let Some(reason) = prunable {
        return State::Broken(BrokenReason::Prunable(reason));
    }
    match state_file(worktree) {
        Ok(state) => state,
        Err(err) => State::Broken(BrokenReason::StateUnreadable(format!("{err:#}"))),
    }
}

/// The state recorded in the state file of the worktree at `worktree`.
fn state_file(worktree: &Path) -> Result<State> {
    let path = git::admin_dir(worktree)?.state_file();
    let bytes = fs::read_optional(&path)?;
    state::read(&path, bytes.as_deref())
}
