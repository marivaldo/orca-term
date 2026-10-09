//! Domain: reading the output of `git worktree list --porcelain -z` into worktree entries.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;

use crate::domain::fleet::WorktreeEntry;

/// Parses `git worktree list --porcelain -z`: attribute lines end in NUL, and an empty attribute
/// line ends each record. The primary checkout comes first, as git always lists it.
pub(crate) fn parse(raw: &[u8]) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for field in raw.split(|&b| b == 0) {
        if field.is_empty() {
            entries.extend(current.take());
        } else if let Some(path) = field.strip_prefix(b"worktree ") {
            entries.extend(current.take());
            current = Some(WorktreeEntry {
                path: PathBuf::from(OsStr::from_bytes(path)),
                branch: None,
                prunable: None,
            });
        } else if let (Some(branch), Some(wt)) = (field.strip_prefix(b"branch "), current.as_mut())
        {
            let branch = branch.strip_prefix(b"refs/heads/").unwrap_or(branch);
            wt.branch = Some(String::from_utf8_lossy(branch).into_owned());
        } else if let (Some(rest), Some(wt)) = (field.strip_prefix(b"prunable"), current.as_mut()) {
            let reason = rest.strip_prefix(b" ").unwrap_or(rest);
            wt.prunable = Some(String::from_utf8_lossy(reason).into_owned());
        }
    }
    entries.extend(current);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(path: &str, branch: Option<&str>) -> WorktreeEntry {
        WorktreeEntry {
            path: PathBuf::from(path),
            branch: branch.map(str::to_owned),
            prunable: None,
        }
    }

    #[test]
    fn parses_records_and_strips_refs_heads() {
        let raw = b"worktree /repo\0HEAD abc\0branch refs/heads/main\0\0\
worktree /worktrees/repo/fix\0HEAD def\0branch refs/heads/fix\0\0\
worktree /worktrees/repo/look\0HEAD 123\0detached\0\0";
        assert_eq!(
            parse(raw),
            vec![
                wt("/repo", Some("main")),
                wt("/worktrees/repo/fix", Some("fix")),
                wt("/worktrees/repo/look", None),
            ]
        );
    }

    #[test]
    fn keeps_locked_and_prunable_worktrees() {
        let raw = b"worktree /repo\0HEAD abc\0branch refs/heads/main\0\0\
worktree /gone\0HEAD def\0branch refs/heads/gone\0prunable gitdir file points to non-existent location\0\0\
worktree /held\0HEAD 456\0branch refs/heads/held\0locked\0\0";
        let parsed = parse(raw);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[1].path, PathBuf::from("/gone"));
        assert_eq!(parsed[1].branch.as_deref(), Some("gone"));
        assert_eq!(
            parsed[1].prunable.as_deref(),
            Some("gitdir file points to non-existent location")
        );
        assert_eq!(parsed[2], wt("/held", Some("held")));
    }
}
