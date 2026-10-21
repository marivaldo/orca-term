//! Ops: `session ls`, every session a worktree held (#41), the most recently active first, the
//! current one marked (`domain/session.rs`). Only reads: no lock, no git beyond finding the
//! worktree.

use anyhow::Result;

use crate::adapters::env;
use crate::domain::session::SessionList;
use crate::domain::worktree::Target;
use crate::ops::{find_turned, read_sessions};

/// The sessions of the worktree `target` names or locates in the repository containing the
/// current directory.
pub(crate) fn run(target: &Target) -> Result<SessionList> {
    let (worktree, admin) = find_turned(target)?;
    Ok(SessionList {
        sessions: read_sessions(&admin)?,
        worktree,
        now: env::now_millis(),
    })
}
