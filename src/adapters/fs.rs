//! Adapter: the file system. Reading, writing and copying files, creating directories and
//! resolving paths through symlinks all happen here.
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "this adapter is the one place that touches the file system"
)]

use std::fs::{self, File};
use std::io::{ErrorKind, Read as _, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// The bytes of the file at `path`, or `None` when it does not exist.
pub(crate) fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("could not read {}", path.display())),
    }
}

/// The UTF-8 text of the file at `path`, or `None` when it does not exist.
pub(crate) fn read_text_optional(path: &Path) -> Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err).with_context(|| format!("could not read {}", path.display())),
    }
}

/// Writes a temp file beside `path` and renames it over `path`, so readers see all or nothing.
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> Result<()> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = path.with_file_name(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = File::create(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result.with_context(|| format!("could not write {}", path.display()))
}

/// Creates the directory `path` and every missing parent.
pub(crate) fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("could not create {}", path.display()))
}

/// `path` made absolute with every symlink resolved. Fails when it does not exist.
pub(crate) fn canonicalize(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("could not resolve {}", path.display()))
}

/// Whether `a` and `b` are the same path, literally or once both are resolved through symlinks.
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    a == b
        || matches!(
            (a.canonicalize(), b.canonicalize()),
            (Ok(a), Ok(b)) if a == b
        )
}

/// `path` with its longest existing ancestor resolved through symlinks, so it can be compared with
/// a canonical path even before it exists.
pub(crate) fn physical(path: &Path) -> PathBuf {
    let mut existing = path;
    let mut rest = Vec::new();
    loop {
        if let Ok(real) = existing.canonicalize() {
            return rest.iter().rev().fold(real, |acc, part| acc.join(part));
        }
        match (existing.parent(), existing.file_name()) {
            (Some(parent), Some(name)) => {
                rest.push(name.to_owned());
                existing = parent;
            }
            _ => return path.to_owned(),
        }
    }
}

/// Whether anything is at `path`, a dangling symlink included.
pub(crate) fn exists(path: &Path) -> bool {
    path.symlink_metadata().is_ok()
}

/// Whether `path` is a directory, following symlinks.
pub(crate) fn is_dir(path: &Path) -> bool {
    path.is_dir()
}

/// Whether `path` is a regular file, following symlinks.
pub(crate) fn is_file(path: &Path) -> bool {
    path.is_file()
}

/// Whether `path` itself is a symlink.
pub(crate) fn is_symlink(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// The permission bits of the regular file at `path`, without following a symlink, or `None` when
/// nothing is there or it is not a regular file.
pub(crate) fn regular_file_mode(path: &Path) -> Result<Option<u32>> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(err).with_context(|| format!("could not read {}", path.display()));
        }
    };
    Ok(meta
        .file_type()
        .is_file()
        .then(|| meta.permissions().mode()))
}

/// Copies the bytes of `from` into `to` with a read/write loop and gives `to` the mode `mode`.
///
/// `std::fs::copy` is not used on purpose: on macOS it clones the file (`fclonefileat`) and on
/// Linux it may reflink (`copy_file_range`), and the copy list promises plain copies.
pub(crate) fn byte_copy(from: &Path, to: &Path, mode: u32) -> std::io::Result<()> {
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

#[cfg(test)]
mod tests {
    use assert_fs::TempDir;

    use super::*;

    #[test]
    fn physical_resolves_the_existing_prefix() {
        let tmp = TempDir::new().unwrap();
        let real = tmp.path().canonicalize().unwrap();
        assert_eq!(
            physical(&tmp.path().join("not/yet/there")),
            real.join("not/yet/there")
        );
    }

    #[test]
    fn writing_atomically_leaves_no_temp_file() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("worktree.json");
        write_atomically(&path, b"{}").unwrap();
        assert_eq!(read_optional(&path).unwrap().as_deref(), Some(&b"{}"[..]));
        let leftovers: Vec<_> = fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "the temp file is renamed away");
    }

    #[test]
    fn reading_a_missing_file_is_none_and_writes_nothing() {
        let dir = TempDir::new().unwrap();
        let absent = dir.path().join("absent");
        assert_eq!(read_optional(&absent).unwrap(), None);
        assert!(!absent.exists(), "reading never writes");
    }
}
