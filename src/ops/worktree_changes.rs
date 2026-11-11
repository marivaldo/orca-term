//! Ops: a worktree's changed files with their counts, and its drift (`domain/changes.rs`), what
//! the application's right column shows (#177). Only reads: git's two diffs against the merge base
//! with the start point, its list of untracked files, the untracked files' bytes to count their
//! lines, and the drift as `worktree detail` counts it. Fetches nothing.

use anyhow::Result;

use crate::adapters::{fs, git};
use crate::domain::changes::{self, Changes};
use crate::domain::fleet::{PrimaryCheckout, Worktree};
use crate::ops::worktree_detail::read_drift;

/// How much of an untracked file is read to count its lines; a larger one is not counted.
const COUNT_UP_TO: u64 = 1024 * 1024;

/// How many untracked files have their lines counted; the others are listed without counts, so a
/// directory of thousands of new files is not read every time.
const COUNT_AT_MOST: usize = 200;

/// The changed files and the drift of `worktree`, of the repository whose primary checkout is
/// `primary`. A worktree that cannot count its changes (broken, setting up) has none; one whose
/// history shares nothing with the start point lists only its untracked files.
pub(crate) fn read(primary: &PrimaryCheckout, worktree: &Worktree) -> Result<Changes> {
    if !worktree.counts_changes() {
        return Ok(Changes::default());
    }
    let path = &worktree.path;
    let start = git::start_point(primary)?;
    let name_status = git::diff_name_status(path, &start)?.unwrap_or_default();
    let numstat = git::diff_numstat(path, &start)?.unwrap_or_default();
    let untracked: Vec<(String, Option<u64>)> = git::untracked_files(path)?
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .enumerate()
        .map(|(index, name)| {
            let name = String::from_utf8_lossy(name).into_owned();
            let lines = (index < COUNT_AT_MOST)
                .then(|| untracked_lines(&path.join(&name)))
                .flatten();
            (name, lines)
        })
        .collect();
    Ok(Changes {
        files: changes::changed_files(&name_status, &numstat, untracked),
        drift: read_drift(primary, path)?,
    })
}

/// The lines of the untracked file at `file`, `None` when it is binary, larger than
/// [`COUNT_UP_TO`] or cannot be read.
fn untracked_lines(file: &std::path::Path) -> Option<u64> {
    let bytes = fs::read_tail(file, COUNT_UP_TO).ok()??;
    let whole = u64::try_from(bytes.len()).is_ok_and(|len| len < COUNT_UP_TO);
    whole.then(|| changes::count_lines(&bytes)).flatten()
}
