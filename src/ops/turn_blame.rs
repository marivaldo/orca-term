//! Ops: `turn blame`, which turn wrote each line of a worktree's file as it is on disk now
//! (`domain/blame.rs`): `git blame` of the file on disk against the tip of the worktree's chain of
//! snapshots, each line's commit mapped through the turn log. Only reads: no lock, and git writes
//! nothing.

use std::path::Path;

use anyhow::{Result, bail};

use crate::adapters::{fs, git};
use crate::domain::blame::{self, Blame};
use crate::domain::worktree::Target;
use crate::ops::{find_turned, read_history, read_status};

/// Who wrote each line of the file at `given` (relative to the worktree's root, or absolute
/// inside it) of the worktree `target` names or locates in the repository containing the current
/// directory. Refuses a path outside the worktree, also through a symlink, and one that is not a
/// file on disk. With no chain yet, or a file no snapshot holds, every line changed since the
/// latest snapshot.
pub(crate) fn run(target: &Target, given: &Path) -> Result<Blame> {
    let (worktree, admin) = find_turned(target)?;
    let root = fs::physical(&worktree.path);
    let resolved = if given.is_absolute() {
        fs::physical(given)
    } else {
        given.to_owned()
    };
    let file = blame::worktree_file(&resolved, &root)?;
    let on_disk = worktree.path.join(&file);
    if !fs::physical(&on_disk).starts_with(&root) {
        bail!("the path {file} leaves the worktree through a symlink");
    }
    if !fs::is_file(&on_disk) {
        bail!("{file} is not a file of worktree {}", worktree.name);
    }
    let tip = read_status(&admin).and_then(|status| status.snapshot);
    let lines = match tip {
        Some(tip)
            if git::commit_exists(&worktree.path, &tip)?
                && git::has_file(&worktree.path, &tip, &file)? =>
        {
            let turns = read_history(&worktree, &admin)?.turns;
            let porcelain = git::blame_porcelain(&worktree.path, &tip, &on_disk, &file)?;
            blame::attribute(blame::parse_porcelain(&porcelain)?, &turns)
        }
        _ => {
            let content = fs::read_optional(&on_disk)?.unwrap_or_default();
            blame::unsnapshotted(&String::from_utf8_lossy(&content))
        }
    };
    Ok(Blame {
        name: worktree.name,
        path: worktree.path,
        file,
        lines,
    })
}
