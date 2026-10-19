//! Ops: `note ls`, the pending notes of a worktree (`domain/note.rs`), each with its anchor as
//! made and translated to the file as it is now. Only reads.

use anyhow::Result;

use crate::domain::note::Listed;
use crate::domain::worktree::Target;
use crate::ops::{find_noted, pending, read_notes};

/// The pending notes of the worktree `target` names or locates in the repository containing the
/// current directory, in id order.
pub(crate) fn run(target: &Target) -> Result<Listed> {
    let (worktree, admin) = find_noted(target)?;
    let notes = read_notes(&admin)?
        .all()
        .iter()
        .map(|note| pending(&worktree.path, &admin, note))
        .collect::<Result<_>>()?;
    Ok(Listed {
        name: worktree.name,
        path: worktree.path,
        notes,
    })
}
