//! Domain: the copy list, the git-ignored files named in `.worktreeinclude` that are copied into
//! a worktree (ADR 0002).
//!
//! It keeps the upstream semantics: `.worktreeinclude` uses gitignore syntax, and only files that
//! are both matched by it and ignored by the repository's own excludes are copied. Symlinks are
//! skipped, and every copy is a plain byte copy, so a worktree never shares an inode, a symlink target
//! or a copy-on-write extent with the primary checkout.

use std::collections::BTreeSet;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Component, Path, PathBuf};

/// The copy list's file name, at the primary checkout root.
pub(crate) const FILE_NAME: &str = ".worktreeinclude";

/// The repository-relative paths to copy: the untracked files `.worktreeinclude` matches
/// (`matching`) that the repository also ignores (`ignored`), both as NUL-separated lists from
/// `git ls-files -z`.
pub(crate) fn copyable(matching: &[u8], ignored: &[u8]) -> Vec<PathBuf> {
    paths(matching)
        .intersection(&paths(ignored))
        .cloned()
        .collect()
}

/// Whether a repository-relative path stays below its root: relative, with no `..`.
pub(crate) fn stays_inside(rel: &Path) -> bool {
    !rel.as_os_str().is_empty() && rel.components().all(|c| matches!(c, Component::Normal(_)))
}

fn paths(raw: &[u8]) -> BTreeSet<PathBuf> {
    raw.split(|&b| b == 0)
        .filter(|p| !p.is_empty())
        .map(|p| PathBuf::from(OsStr::from_bytes(p)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_relative_paths_stay_inside() {
        assert!(stays_inside(Path::new(".env")));
        assert!(stays_inside(Path::new("config/local.yaml")));
        assert!(!stays_inside(Path::new("")));
        assert!(!stays_inside(Path::new("/etc/passwd")));
        assert!(!stays_inside(Path::new("../outside")));
        assert!(!stays_inside(Path::new("a/../../b")));
    }

    #[test]
    fn copies_only_what_is_both_matched_and_ignored() {
        assert_eq!(
            copyable(b".env\0notes.txt\0", b".env\0target/x\0"),
            vec![PathBuf::from(".env")]
        );
    }

    #[test]
    fn splits_nul_separated_paths() {
        let set = paths(b".env\0config/local.yaml\0");
        assert_eq!(set.len(), 2);
        assert!(set.contains(Path::new("config/local.yaml")));
    }
}
