//! Ops: the hidden `agent holder` command, which is the holder process itself (ADR 0009):
//! `agent start` runs it detached, with the worktree's admin directory and path, the agent's first
//! terminal size, the agent's command line and, for opencode, the agent whose hook it runs for
//! each event the agent reports through its pipe (#35). Once the agent has exited and the holder
//! has removed its files, it tells the running Neovims that the worktree changed.

use anyhow::Result;

use crate::adapters::holder::{self, HolderArgs};
use crate::ops::notify_changed;

/// Holds the agent `args` names until it exits.
pub(crate) fn run(args: &HolderArgs) -> Result<()> {
    let served = holder::process::serve(args);
    notify_changed(&args.admin);
    served
}
