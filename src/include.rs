//! The copy list: git-ignored files named in `.worktreeinclude`, copied into a lane (ADR 0002).
//!
//! It keeps the upstream semantics: `.worktreeinclude` uses gitignore syntax, and only files that
//! are both matched by it and ignored by the repository's own excludes are copied. Symlinks are
//! skipped, and every copy is a plain byte copy, so a lane never shares an inode, a symlink target
//! or a copy-on-write extent with the primary checkout.

use std::collections::BTreeSet;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{ErrorKind, Read as _, Write as _};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::OpenOptionsExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::git;

/// The copy list's file name, at the primary checkout root.
pub const FILE_NAME: &str = ".worktreeinclude";

/// The repository-relative paths to copy from `primary`: ignored, untracked files that
/// `.worktreeinclude` matches. Empty when there is no `.worktreeinclude`.
pub fn list(primary: &Path) -> Result<Vec<PathBuf>> {
    let include = primary.join(FILE_NAME);
    if !include.is_file() {
        return Ok(Vec::new());
    }
    let mut exclude_from = OsString::from("--exclude-from=");
    exclude_from.push(&include);
    let listed = paths(&git::output(
        primary,
        &[
            OsStr::new("ls-files"),
            OsStr::new("-z"),
            OsStr::new("--others"),
            OsStr::new("--ignored"),
            exclude_from.as_os_str(),
        ],
    )?);
    let ignored = paths(&git::output(
        primary,
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
        ],
    )?);
    Ok(listed.intersection(&ignored).cloned().collect())
}

/// Copies each of `paths` from `primary` into `lane`, returning how many files were copied.
/// Symlinks and anything else that is not a regular file are skipped.
pub fn copy(primary: &Path, lane: &Path, paths: &[PathBuf]) -> Result<usize> {
    let lane_real = lane
        .canonicalize()
        .with_context(|| format!("could not resolve {}", lane.display()))?;
    let mut copied = 0;
    for rel in paths {
        if !stays_inside(rel) {
            bail!(
                "refusing to copy {}: it escapes the worktree",
                rel.display()
            );
        }
        let from = primary.join(rel);
        let meta = match fs::symlink_metadata(&from) {
            Ok(meta) => meta,
            Err(err) if err.kind() == ErrorKind::NotFound => continue,
            Err(err) => {
                return Err(err).with_context(|| format!("could not read {}", from.display()));
            }
        };
        if !meta.file_type().is_file() {
            continue;
        }
        let to = lane.join(rel);
        let parent = to.parent().unwrap_or(lane);
        fs::create_dir_all(parent)
            .with_context(|| format!("could not create {}", parent.display()))?;
        let parent_real = parent
            .canonicalize()
            .with_context(|| format!("could not resolve {}", parent.display()))?;
        let to_is_symlink = fs::symlink_metadata(&to).is_ok_and(|m| m.file_type().is_symlink());
        if !parent_real.starts_with(&lane_real) || to_is_symlink {
            bail!(
                "refusing to copy {}: it would land outside the worktree",
                rel.display()
            );
        }
        byte_copy(&from, &to, meta.permissions().mode())
            .with_context(|| format!("could not copy {} into the lane", rel.display()))?;
        copied += 1;
    }
    Ok(copied)
}

/// Copies the bytes of `from` into `to` with a read/write loop and gives `to` the mode `mode`.
///
/// `std::fs::copy` is not used on purpose: on macOS it clones the file (`fclonefileat`) and on
/// Linux it may reflink (`copy_file_range`), and the copy list promises plain copies.
fn byte_copy(from: &Path, to: &Path, mode: u32) -> std::io::Result<()> {
    let mut reader = File::open(from)?;
    let mut writer = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(to)?;
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        writer.write_all(&buf[..n])?;
    }
    writer.flush()?;
    fs::set_permissions(to, fs::Permissions::from_mode(mode))
}

/// Whether a repository-relative path stays below its root: relative, with no `..`.
fn stays_inside(rel: &Path) -> bool {
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
    fn splits_nul_separated_paths() {
        let set = paths(b".env\0config/local.yaml\0");
        assert_eq!(set.len(), 2);
        assert!(set.contains(Path::new("config/local.yaml")));
    }
}
