//! Ops: `worktree ls`, which lists the fleet. Never writes.

use anyhow::Result;

use crate::adapters::env;
use crate::domain::fleet::Fleet;
use crate::ops::read_fleet;

/// The fleet of the repository containing the current directory.
pub(crate) fn run() -> Result<Fleet> {
    let cwd = env::current_dir()?;
    read_fleet(&cwd)
}
