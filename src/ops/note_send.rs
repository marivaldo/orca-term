//! Ops: `note send`, which pastes pending notes into the worktree's agent as one prompt in Orca's
//! format (`domain/note/prompt.rs`) and consumes them.
//!
//! The prompt is typed into the agent's terminal through its holder, as an attached terminal
//! would: a bracketed paste, then Enter on its own a moment later. The notes are read once, before
//! the paste, and consumed only once both writes went through; a note edited in between keeps its
//! new revision and stays pending, as in Orca. With no live holder nothing is sent or consumed.

use anyhow::{Context, Result};

use crate::adapters::holder;
use crate::domain::error::TypedError;
use crate::domain::note::prompt::{self, Block};
use crate::domain::note::{NotesSent, PendingNote};
use crate::domain::worktree::Target;
use crate::ops::{change_notes, find_noted, notify_changed, pending, read_notes};

/// Sends the notes `ids` (every pending note when empty) of the worktree `target` names or locates
/// in the repository containing the current directory to its agent, consumes them, then tells the
/// running Neovims. Refuses an id with no note, a worktree with no pending notes, and, with the
/// typed `no_holder`, a worktree whose agent is not running.
pub(crate) fn run(target: &Target, ids: &[u64]) -> Result<NotesSent> {
    let (worktree, admin) = find_noted(target)?;
    let selected = read_notes(&admin)?.select(ids)?;
    if !worktree.holder.is_alive() {
        return Err(TypedError::NoHolder {
            name: worktree.name,
            path: worktree.path,
        }
        .into());
    }
    let notes: Vec<PendingNote> = selected
        .iter()
        .map(|note| pending(&worktree.path, &admin, note))
        .collect::<Result<_>>()?;
    let blocks: Vec<Block<'_>> = notes
        .iter()
        .map(|pending| Block {
            path: &pending.note.path,
            anchor: pending.current,
            body: &pending.note.body,
        })
        .collect();
    let prompt = prompt::format(&blocks);

    // Changes this process's directory; every path used after it is absolute.
    holder::type_input(
        &admin,
        &prompt::paste(&prompt),
        prompt::PASTE_SUBMIT_PAUSE,
        prompt::SUBMIT,
    )?;

    let sent: Vec<(u64, u64)> = selected.iter().map(|note| (note.id, note.rev)).collect();
    let kept = change_notes(&admin, &worktree.name, |notes| Ok(notes.consume(&sent)))
        .context("the notes were sent, but could not be consumed")?;
    notify_changed(&admin);
    Ok(NotesSent {
        name: worktree.name,
        path: worktree.path,
        prompt,
        sent: sent.iter().map(|(id, _)| *id).collect(),
        kept,
    })
}
