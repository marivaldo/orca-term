//! Ops: `worktree ls`, which lists the fleet. Never writes: it runs git to count each worktree's
//! changed files, and to find the default branch on `origin` a worktree is rebased onto (#141),
//! which it never fetches.

use anyhow::Result;

use crate::adapters::{env, git};
use crate::domain::fleet::Fleet;
use crate::ops::{count_fleet_changes, read_fleet};

/// The fleet of the repository containing the current directory, with each worktree's changed
/// files counted and the default branch on `origin`, when it has one. A base git cannot find is
/// none: listing never fails for it.
pub(crate) fn run() -> Result<Fleet> {
    let cwd = env::current_dir()?;
    let mut fleet = read_fleet(&cwd)?;
    count_fleet_changes(&mut fleet);
    fleet.remote_base = git::remote_default_branch(&fleet.primary).ok().flatten();
    Ok(fleet)
}
