//! Ops: `note rm`, which removes a pending note without sending it (`domain/note.rs`), and the
//! content kept for it once no other note refers to that content.

use anyhow::Result;

use crate::domain::note::Unnoted;
use crate::domain::worktree::Target;
use crate::ops::{change_notes, find_noted, notify_changed};

/// Removes the note `id` of the worktree `target` names or locates in the repository containing
/// the current directory, then tells the running Neovims.
pub(crate) fn run(target: &Target, id: u64) -> Result<Unnoted> {
    let (worktree, admin) = find_noted(target)?;
    let note = change_notes(&admin, &worktree.name, |notes| Ok(notes.remove(id)?))?;
    notify_changed(&admin);
    Ok(Unnoted {
        name: worktree.name,
        path: worktree.path,
        note,
    })
}
