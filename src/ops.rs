//! Ops: one module per command, each a short sequence of adapter and domain calls. This file holds
//! what several commands share: reading the fleet.

pub(crate) mod worktree_ls;
pub(crate) mod worktree_new;
pub(crate) mod worktree_prune;
pub(crate) mod worktree_rm;

use std::path::Path;

use anyhow::Result;

use crate::adapters::{fs, git};
use crate::domain::fleet::{self, Fleet, Worktree};
use crate::domain::state::{self, State};

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
        read_state(worktree, prunable.as_deref());
    }
    Ok(fleet)
}

/// Reads the worktree's state. A worktree whose directory is gone, or whose state cannot be read,
/// is broken, and the reason goes in `detail`: reading one worktree never fails the fleet.
fn read_state(worktree: &mut Worktree, prunable: Option<&str>) {
    if let Some(detail) = fleet::gone_detail(fs::is_dir(&worktree.path), prunable) {
        worktree.mark_gone(detail);
        return;
    }
    match state_file(&worktree.path) {
        Ok(state) => worktree.state = state,
        Err(err) => worktree.mark_unreadable(format!("{err:#}")),
    }
}

/// The state recorded in the state file of the worktree at `worktree`.
fn state_file(worktree: &Path) -> Result<State> {
    let path = git::admin_dir(worktree)?.join(state::WORKTREE_FILE);
    let bytes = fs::read_optional(&path)?;
    state::read(&path, bytes.as_deref())
}
