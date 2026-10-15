//! Ops: the hidden `agent holder` command, which is the holder process itself (ADR 0009):
//! `agent start` runs it detached, with the worktree's admin directory and path, the agent's first
//! terminal size and the agent's command line. Once the agent has exited and the holder has
//! removed its files, it tells the running Neovims that the worktree changed.

use std::path::PathBuf;

use anyhow::Result;

use crate::adapters::holder::{self, HolderArgs};
use crate::domain::agent::Size;
use crate::domain::state::AdminDir;
use crate::ops::notify_changed;

/// Holds the agent `argv` in `worktree`, with its files in `admin`, until it exits.
pub(crate) fn run(admin: PathBuf, worktree: PathBuf, size: Size, argv: Vec<String>) -> Result<()> {
    let admin = AdminDir::new(admin);
    let served = holder::process::serve(&HolderArgs {
        admin: admin.clone(),
        worktree,
        size,
        argv,
    });
    notify_changed(&admin);
    served
}
