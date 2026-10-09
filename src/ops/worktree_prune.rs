//! Ops: `worktree prune`, which cleans the worktrees whose directory is gone. Only ever invoked by
//! the person: no other command prunes.

use std::path::PathBuf;

use anyhow::Result;

use crate::adapters::{env, git};
use crate::domain::fleet;
use crate::ops::read_fleet;

/// Runs `git worktree prune` from the primary checkout and returns the paths of the worktrees it
/// cleaned.
pub(crate) fn run() -> Result<Vec<PathBuf>> {
    let cwd = env::current_dir()?;
    let before = read_fleet(&cwd)?;
    git::worktree_prune(&before.primary)?;
    let after = read_fleet(&before.primary.path)?;
    Ok(fleet::pruned(&before, &after))
}
