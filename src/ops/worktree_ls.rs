//! Ops: `worktree ls`, which lists the fleet. Never writes: it runs git to count each worktree's
//! changed files, and to find the default branch on `origin` a worktree is rebased onto (#141),
//! which it never fetches, and whether it has an `origin` a worktree is pushed to (#146).

use std::path::Path;

use anyhow::Result;

use crate::adapters::{env, git};
use crate::domain::fleet::Fleet;
use crate::ops::{count_fleet_changes, read_fleet};

/// The fleet of the repository containing the current directory, with each worktree's changed
/// files counted and the default branch on `origin`, when it has one. A base git cannot find is
/// none: listing never fails for it.
pub(crate) fn run() -> Result<Fleet> {
    read_listed(&env::current_dir()?)
}

/// The fleet of the repository containing `dir`, as [`run`] lists it: `fleets` reads every
/// project's this way (#149).
pub(crate) fn read_listed(dir: &Path) -> Result<Fleet> {
    let mut fleet = read_fleet(dir)?;
    count_fleet_changes(&mut fleet);
    fleet.remote_base = git::remote_default_branch(&fleet.primary).ok().flatten();
    fleet.has_origin = git::has_origin(&fleet.primary).unwrap_or(false);
    Ok(fleet)
}
