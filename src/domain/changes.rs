//! Domain: a worktree's changed files, each with its letter and its counts of lines added and
//! deleted, and how far its branch drifted (#177): what the application's right column shows.
//!
//! The files are the ones `Worktree::changed_files` counts (#92): what differs between the merge
//! base with the start point and the files on disk, committed or not, plus the untracked files git
//! does not ignore. git reports the tracked ones twice, in one process each: `git diff
//! --name-status -z` gives each file's letter, `git diff --numstat -z` its counts, both with
//! `--find-renames`, so a moved file is one entry under its new path. An untracked file is added,
//! and its lines are counted from its bytes.

use crate::domain::detail::Drift;

/// How a file changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    Modified,
    Added,
    Deleted,
    /// Moved, with or without changes; listed under its new path.
    Renamed,
}

impl Change {
    /// The letter git and the design show for it.
    pub(crate) fn letter(self) -> char {
        match self {
            Self::Modified => 'M',
            Self::Added => 'A',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
        }
    }

    /// The change git's `--name-status` letter `status` names (`R100` for a rename, `C75` for a
    /// copy, which reads as added). Anything else (a type change, an unmerged file) is modified.
    fn from_status(status: &str) -> Self {
        match status.chars().next() {
            Some('A' | 'C') => Self::Added,
            Some('D') => Self::Deleted,
            Some('R') => Self::Renamed,
            _ => Self::Modified,
        }
    }
}

/// One changed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChangedFile {
    /// Its path relative to the worktree, with `/` between directories.
    pub(crate) path: String,
    pub(crate) change: Change,
    /// The lines it added and deleted; `None` for a binary file, or one too large to count.
    pub(crate) lines: Option<Lines>,
}

/// Lines added and deleted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Lines {
    pub(crate) added: u64,
    pub(crate) deleted: u64,
}

/// What the right column shows of a worktree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Changes {
    /// Its changed files, sorted by path.
    pub(crate) files: Vec<ChangedFile>,
    /// How far its branch is from the local default branch, `None` when that cannot be told.
    pub(crate) drift: Option<Drift>,
}

impl Changes {
    /// The lines added and deleted by every file that has counts.
    pub(crate) fn totals(&self) -> Lines {
        self.files
            .iter()
            .filter_map(|file| file.lines)
            .fold(Lines::default(), |sum, lines| Lines {
                added: sum.added + lines.added,
                deleted: sum.deleted + lines.deleted,
            })
    }
}

/// The changed files git reported: `name_status`, what `git diff --name-status -z
/// --find-renames` printed, `numstat`, what `git diff --numstat -z --find-renames` printed, and
/// `untracked`, each untracked file with its lines when they could be counted
/// ([`count_lines`]). Sorted by path; a path both tracked and untracked (removed from the index,
/// still on disk) is listed once, as git's diff says.
pub(crate) fn changed_files(
    name_status: &[u8],
    numstat: &[u8],
    untracked: Vec<(String, Option<u64>)>,
) -> Vec<ChangedFile> {
    let counts = numstat_lines(numstat);
    let mut files: Vec<ChangedFile> = name_status_entries(name_status)
        .into_iter()
        .map(|(change, path)| {
            let lines = counts
                .iter()
                .find(|(counted, _)| *counted == path)
                .and_then(|(_, lines)| *lines);
            ChangedFile {
                path,
                change,
                lines,
            }
        })
        .collect();
    for (path, added) in untracked {
        if files.iter().any(|file| file.path == path) {
            continue;
        }
        files.push(ChangedFile {
            path,
            change: Change::Added,
            lines: added.map(|added| Lines { added, deleted: 0 }),
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

/// The fields of a NUL-separated list, the empty ones at its end left out.
fn fields(bytes: &[u8]) -> Vec<String> {
    let mut fields: Vec<String> = bytes
        .split(|byte| *byte == 0)
        .map(|field| String::from_utf8_lossy(field).into_owned())
        .collect();
    while fields.last().is_some_and(String::is_empty) {
        fields.pop();
    }
    fields
}

/// Each file of `git diff --name-status -z`: its letter, then its path, or for a rename or a copy
/// its letter with a score, the old path and the new one.
fn name_status_entries(bytes: &[u8]) -> Vec<(Change, String)> {
    let mut fields = fields(bytes).into_iter();
    let mut entries = Vec::new();
    while let Some(status) = fields.next() {
        let two_paths = status.starts_with(['R', 'C']);
        let first = fields.next();
        let path = if two_paths { fields.next() } else { first };
        if let Some(path) = path {
            entries.push((Change::from_status(&status), path));
        }
    }
    entries
}

/// Each file of `git diff --numstat -z`: `added<TAB>deleted<TAB>path`, or for a rename
/// `added<TAB>deleted<TAB>` then the old path and the new one as fields of their own; `-` for both
/// counts of a binary file, which has none.
fn numstat_lines(bytes: &[u8]) -> Vec<(String, Option<Lines>)> {
    let mut fields = fields(bytes).into_iter();
    let mut entries = Vec::new();
    while let Some(field) = fields.next() {
        let mut parts = field.splitn(3, '\t');
        let (Some(added), Some(deleted), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let path = if path.is_empty() {
            // A rename: the old path, then the new one.
            fields.next();
            fields.next().unwrap_or_default()
        } else {
            path.to_owned()
        };
        let lines = match (added.parse(), deleted.parse()) {
            (Ok(added), Ok(deleted)) => Some(Lines { added, deleted }),
            _ => None,
        };
        entries.push((path, lines));
    }
    entries
}

/// The lines of a file whose content is `bytes`, as git counts an added file's: every line break,
/// and a last line without one. `None` for a binary file, which holds a NUL byte, as git tells
/// them.
pub(crate) fn count_lines(bytes: &[u8]) -> Option<u64> {
    if bytes.contains(&0) {
        return None;
    }
    // Each line break splits off one more piece.
    let breaks = bytes.split(|byte| *byte == b'\n').count() - 1;
    let unfinished = usize::from(bytes.last().is_some_and(|byte| *byte != b'\n'));
    u64::try_from(breaks + unfinished).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, change: Change, lines: Option<(u64, u64)>) -> ChangedFile {
        ChangedFile {
            path: path.to_owned(),
            change,
            lines: lines.map(|(added, deleted)| Lines { added, deleted }),
        }
    }

    #[test]
    fn each_tracked_file_has_its_letter_and_its_counts_sorted_by_path() {
        let name_status = b"M\0src/auth/login.rs\0A\0src/flag.rs\0D\0templates/password.html\0";
        let numstat =
            b"18\t9\tsrc/auth/login.rs\x0031\t0\tsrc/flag.rs\x000\t10\ttemplates/password.html\0";
        assert_eq!(
            changed_files(name_status, numstat, Vec::new()),
            [
                file("src/auth/login.rs", Change::Modified, Some((18, 9))),
                file("src/flag.rs", Change::Added, Some((31, 0))),
                file("templates/password.html", Change::Deleted, Some((0, 10))),
            ]
        );
    }

    #[test]
    fn a_rename_is_listed_under_its_new_path_and_a_binary_file_has_no_counts() {
        let name_status = b"R087\0old name.rs\0new name.rs\0M\0logo.png\0C100\0a.rs\0b.rs\0";
        let numstat = b"2\t1\t\0old name.rs\0new name.rs\0-\t-\tlogo.png\x000\t0\t\0a.rs\0b.rs\0";
        assert_eq!(
            changed_files(name_status, numstat, Vec::new()),
            [
                file("b.rs", Change::Added, Some((0, 0))),
                file("logo.png", Change::Modified, None),
                file("new name.rs", Change::Renamed, Some((2, 1))),
            ]
        );
    }

    #[test]
    fn untracked_files_are_added_and_listed_once() {
        let name_status = b"D\0gone.txt\0";
        let numstat = b"0\t1\tgone.txt\0";
        let untracked = vec![
            ("new.txt".to_owned(), Some(3)),
            ("gone.txt".to_owned(), Some(1)),
            ("blob.bin".to_owned(), None),
        ];
        assert_eq!(
            changed_files(name_status, numstat, untracked),
            [
                file("blob.bin", Change::Added, None),
                file("gone.txt", Change::Deleted, Some((0, 1))),
                file("new.txt", Change::Added, Some((3, 0))),
            ]
        );
        assert_eq!(changed_files(b"", b"", Vec::new()), []);
    }

    #[test]
    fn lines_are_counted_as_git_counts_them_and_a_binary_file_has_none() {
        assert_eq!(count_lines(b""), Some(0));
        assert_eq!(count_lines(b"one\ntwo\n"), Some(2));
        assert_eq!(
            count_lines(b"one\ntwo"),
            Some(2),
            "a last line with no break"
        );
        assert_eq!(count_lines(b"\x89PNG\0\0"), None);
    }

    #[test]
    fn the_totals_add_up_the_files_with_counts() {
        let changes = Changes {
            files: vec![
                file("a", Change::Modified, Some((18, 9))),
                file("b", Change::Added, Some((31, 0))),
                file("c", Change::Modified, None),
            ],
            drift: None,
        };
        assert_eq!(
            changes.totals(),
            Lines {
                added: 49,
                deleted: 9
            }
        );
    }
}
