//! Ops: `worktree ls`, which lists the fleet. Never writes: it runs git to count each worktree's
//! changed files.

use anyhow::Result;

use crate::adapters::env;
use crate::domain::fleet::Fleet;
use crate::ops::{count_fleet_changes, read_fleet};

/// The fleet of the repository containing the current directory, with each worktree's changed
/// files counted.
pub(crate) fn run() -> Result<Fleet> {
    let cwd = env::current_dir()?;
    let mut fleet = read_fleet(&cwd)?;
    count_fleet_changes(&mut fleet);
    Ok(fleet)
}
