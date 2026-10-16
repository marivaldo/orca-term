//! Ops: `note edit`, which replaces a pending note's body (`domain/note.rs`) and bumps its
//! revision, so a send already in flight keeps it pending.

use anyhow::Result;

use crate::domain::note::{self, Noted};
use crate::domain::worktree::Target;
use crate::ops::{change_notes, find_noted, notify_changed, pending};

/// Replaces the body of the note `id` of the worktree `target` names or locates in the repository
/// containing the current directory with `body`, then tells the running Neovims.
pub(crate) fn run(target: &Target, id: u64, body: &str) -> Result<Noted> {
    let body = note::parse_body(body)?;
    let (worktree, admin) = find_noted(target)?;
    let edited = change_notes(&admin, &worktree.name, |notes| Ok(notes.edit(id, body)?))?;
    notify_changed(&admin);
    let note = pending(&worktree.path, &admin, &edited)?;
    Ok(Noted {
        name: worktree.name,
        path: worktree.path,
        note,
    })
}
