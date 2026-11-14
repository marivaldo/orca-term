//! Ops: the hidden `agent holder` command, which is the holder process itself (ADR 0009):
//! `agent start` runs it detached, with the worktree's admin directory and path, the agent's first
//! terminal size, the agent's command line and, for opencode, the agent whose hook it runs for
//! each event the agent reports through its pipe (#35).

use anyhow::Result;

use crate::adapters::holder::{self, HolderArgs};

/// Holds the agent `args` names until it exits.
pub(crate) fn run(args: &HolderArgs) -> Result<()> {
    holder::process::serve(args)
}
