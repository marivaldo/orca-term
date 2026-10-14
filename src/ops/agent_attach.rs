//! Ops: `agent attach`, which joins this terminal to a worktree's running agent through its
//! holder (ADR 0009). The holder replays the agent's scrollback first; detaching leaves the agent
//! running.

use anyhow::{Result, bail};

use crate::adapters::terminal::{self, DetachKey};
use crate::adapters::{env, holder};
use crate::domain::agent::AttachEnd;
use crate::domain::worktree::Target;
use crate::ops::{find_target_admin, read_fleet};

/// Attaches to the agent of the worktree `target` names or locates in the repository containing
/// the current directory, until the person detaches or the agent ends.
pub(crate) fn run(target: &Target, detach_key: DetachKey) -> Result<AttachEnd> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let (worktree, admin) = find_target_admin(&fleet, &cwd, target)?;
    if !worktree.holder.is_alive() {
        bail!(
            "no agent is running in worktree {}; start one with `orca-term agent start {target}`",
            worktree.name
        );
    }
    // `connect` changes the current directory: every path is resolved by now.
    let stream = holder::connect(&admin)?;
    terminal::attach(stream, detach_key)
}
