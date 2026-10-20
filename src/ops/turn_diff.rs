//! Ops: `turn diff`, what one turn of a worktree changed: `git diff` from its begin snapshot to
//! its end snapshot (`domain/history.rs`). Only reads: no lock, and git writes nothing.

use anyhow::Result;

use crate::adapters::git;
use crate::domain::history::{self, TurnDiff};
use crate::domain::worktree::Target;
use crate::ops::{find_turned, read_history};

/// What turn `number` of the worktree `target` names or locates in the repository containing the
/// current directory changed. Refuses a turn that does not exist, one still open and one without
/// snapshots.
pub(crate) fn run(target: &Target, number: u64) -> Result<TurnDiff> {
    let (worktree, admin) = find_turned(target)?;
    let turns = read_history(&worktree, &admin)?.turns;
    let bounds = history::diff_bounds(&turns, number)?;
    let end_observed = history::find(&turns, number)?.end_observed;
    let patch = git::patch(&worktree.path, &bounds.begin, &bounds.end)?;
    Ok(TurnDiff {
        name: worktree.name,
        path: worktree.path,
        turn: number,
        bounds,
        end_observed,
        patch,
    })
}
