//! Ops: `worktree setup`, which runs a worktree's setup again: after fixing what made it fail, or
//! after changing the script. With no script configured, it records that there is no setup, so
//! the worktree is ready.

use std::io::Write;

use anyhow::Result;

use crate::adapters::env;
use crate::domain::setup::SetupDone;
use crate::domain::worktree::{self, Target};
use crate::ops::{check_no_holder, find_target, lock_worktree, read_fleet, set_up};

/// Runs the setup of the worktree `target` names or locates in the repository containing the
/// current directory, echoing its output to `echo`. Fails as `busy` when the worktree's lock is
/// held or its agent is running. A failed setup is not an error: the report says the worktree is
/// not ready.
pub(crate) fn run(target: &Target, echo: &mut dyn Write) -> Result<SetupDone> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = find_target(&fleet, &cwd, target)?;
    worktree::check_can_set_up(worktree)?;
    check_no_holder(worktree)?;
    let lock = lock_worktree(&worktree.name, &worktree.path)?;
    let setup = set_up(&lock, &fleet.primary, &worktree.path, echo)?;
    Ok(SetupDone {
        name: worktree.name.clone(),
        path: worktree.path.clone(),
        branch: worktree.branch.clone(),
        setup,
    })
}
