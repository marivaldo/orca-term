//! Ops: `note add`, which attaches a note to a line, a range of lines or a whole file of a
//! worktree (`domain/note.rs`). A note on lines keeps a copy of the file's content as it is now,
//! for line translation; a whole-file note may name a file that is gone, as a deleted one is.

use std::path::Path;

use anyhow::{Result, bail};

use crate::adapters::{env, fs};
use crate::domain::note::{self, Anchor, Draft, NotePath, Noted, PendingNote};
use crate::domain::worktree::Target;
use crate::ops::{change_notes, find_noted, keep_copy, notify_changed};

/// Adds a note with `body` on `anchor` of the file at `path` (relative to the worktree's root) of
/// the worktree `target` names or locates in the repository containing the current directory,
/// then tells the running Neovims. Refuses an empty body, a path outside the worktree, and lines
/// outside the file or on a file that is not there.
pub(crate) fn run(target: &Target, path: &str, anchor: Anchor, body: &str) -> Result<Noted> {
    let body = note::parse_body(body)?;
    let path = NotePath::parse(path)?;
    let (worktree, admin) = find_noted(target)?;
    let file = path.in_worktree(&worktree.path);
    check_inside(&worktree.path, &file, &path)?;
    let content = read_lines(&file, &path, anchor)?;

    let note = change_notes(&admin, &worktree.name, |notes| {
        let copy = match &content {
            Some(bytes) => Some(keep_copy(&worktree.path, &admin, bytes)?),
            None => None,
        };
        Ok(notes.add(Draft {
            path,
            anchor,
            body,
            copy,
            created: env::now_millis(),
        }))
    })?;
    notify_changed(&admin);
    Ok(Noted {
        name: worktree.name,
        path: worktree.path,
        note: PendingNote {
            current: note.anchor,
            note,
        },
    })
}

/// Refuses a path that leaves the worktree at `worktree` through a symlink: `file` must stay below
/// it once both are resolved.
fn check_inside(worktree: &Path, file: &Path, path: &NotePath) -> Result<()> {
    if !fs::physical(file).starts_with(fs::physical(worktree)) {
        bail!("the path {path} leaves the worktree through a symlink");
    }
    Ok(())
}

/// The content of `file` for a note on lines, after checking `anchor` fits in it; nothing for a
/// whole-file note. Refuses a directory, and lines on a file that is not there.
fn read_lines(file: &Path, path: &NotePath, anchor: Anchor) -> Result<Option<Vec<u8>>> {
    if fs::is_dir(file) {
        bail!("{path} is a directory; a note is attached to a file");
    }
    if !anchor.is_on_lines() {
        return Ok(None);
    }
    let content = if fs::is_file(file) {
        fs::read_optional(file)?
    } else {
        None
    };
    let Some(content) = content else {
        bail!(
            "{path} is not a file of the worktree; only a whole-file note can name a file that is gone"
        );
    };
    anchor.check_fits(path, note::line_count(&content))?;
    Ok(Some(content))
}
