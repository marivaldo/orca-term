//! Ops: `worktree review`, which the Neovim sidebar runs before it opens a worktree's review in
//! diffview.nvim (`domain/review.rs`). It finds the review's base, the commit where the worktree
//! forked from the default branch, then marks the worktree seen exactly as `worktree seen` does.
//! The base is found first, so a review that cannot start marks nothing.

use anyhow::{Context, Result};

use crate::adapters::{env, git};
use crate::domain::review::{self, Reviewed};
use crate::domain::seen::Mark;
use crate::domain::worktree::Target;
use crate::ops::{find_target, read_fleet, worktree_seen};

/// Finds the review's base for the worktree `target` names or locates in the repository
/// containing the current directory, marks the worktree seen and tells the running Neovims.
/// Refuses the primary checkout, an unknown worktree and one whose directory is gone.
pub(crate) fn run(target: &Target) -> Result<Reviewed> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = find_target(&fleet, &cwd, target)?;
    review::check_can_review(worktree)?;

    // The default branch, resolved as `worktree new` resolves the start point of a new worktree.
    let against = git::start_point(&fleet.primary)
        .with_context(|| format!("cannot review worktree {}", worktree.name))?;
    let base = git::merge_base(&worktree.path, &against)?
        .ok_or_else(|| review::no_base(worktree, &against))?;

    let unseen = worktree_seen::mark(worktree, Mark::Seen)?;
    let mut worktree = worktree.clone();
    worktree.unseen = unseen;
    Ok(Reviewed {
        worktree,
        base,
        against,
    })
}
