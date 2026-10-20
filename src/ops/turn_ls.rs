//! Ops: `turn ls`, every turn of a worktree from its turn log, oldest first, the open one last
//! (`domain/history.rs`). Only reads: no lock, no git.

use anyhow::Result;

use crate::domain::history::History;
use crate::domain::worktree::Target;
use crate::ops::{find_turned, read_history};

/// The turns of the worktree `target` names or locates in the repository containing the current
/// directory.
pub(crate) fn run(target: &Target) -> Result<History> {
    let (worktree, admin) = find_turned(target)?;
    read_history(&worktree, &admin)
}
