//! Adapter: the file system. Reading, writing and copying files, creating directories, resolving
//! paths through symlinks and locking a worktree all happen here.
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "this adapter is the one place that touches the file system"
)]

use std::fs::{self, File, TryLockError};
use std::io::{ErrorKind, Read as _, Seek as _, SeekFrom, Write as _};
use std::os::unix::fs::OpenOptionsExt as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::domain::state::{AdminDir, LockStatus};

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

/// At most the last `max` bytes of the file at `path`, or `None` when it does not exist.
pub(crate) fn read_tail(path: &Path, max: u64) -> Result<Option<Vec<u8>>> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err).with_context(|| format!("could not read {}", path.display())),
    };
    let result = (|| {
        let len = file.metadata()?.len();
        file.seek(SeekFrom::Start(len.saturating_sub(max)))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok::<_, std::io::Error>(bytes)
    })();
    result
        .map(Some)
        .with_context(|| format!("could not read {}", path.display()))
}

/// Appends `bytes` to the file at `path`, creating the file (never its directory) when missing.
pub(crate) fn append(path: &Path, bytes: &[u8]) -> Result<()> {
    fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(path)
        .and_then(|mut file| file.write_all(bytes))
        .with_context(|| format!("could not append to {}", path.display()))
}

/// Appends `bytes` to the file at `path` as [`append`] does, emptying the file first when it has
/// grown past `max` bytes, so it never grows without bound.
pub(crate) fn append_bounded(path: &Path, bytes: &[u8], max: u64) -> Result<()> {
    let too_big = fs::metadata(path).is_ok_and(|meta| meta.len() > max);
    if too_big {
        File::create(path).with_context(|| format!("could not empty {}", path.display()))?;
    }
    append(path, bytes)
}

/// Creates the directory `path` and every missing parent.
pub(crate) fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("could not create {}", path.display()))
}

/// Removes the file at `path`; nothing to do when it does not exist.
pub(crate) fn remove_if_exists(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(()),
        Err(err) => Err(err).with_context(|| format!("could not remove {}", path.display())),
    }
}

/// The subdirectories of `dir`, sorted by name. Empty when `dir` does not exist.
pub(crate) fn list_dirs(dir: &Path) -> Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("could not list {}", dir.display())),
    };
    let mut dirs = Vec::new();
    for entry in entries {
        let path = entry
            .with_context(|| format!("could not list {}", dir.display()))?
            .path();
        if path.is_dir() {
            dirs.push(path);
        }
    }
    dirs.sort();
    Ok(dirs)
}

/// The names of the entries of `dir` that are not directories, sorted. Empty when `dir` does not
/// exist.
pub(crate) fn list_file_names(dir: &Path) -> Result<Vec<String>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(err) => return Err(err).with_context(|| format!("could not list {}", dir.display())),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.with_context(|| format!("could not list {}", dir.display()))?;
        if !entry.path().is_dir() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// The first of `dirs` holding an executable regular file named `name`: what a shell finds for
/// `name` with those directories as PATH.
pub(crate) fn executable_in(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter().map(|dir| dir.join(name)).find(|candidate| {
        fs::metadata(candidate)
            .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
    })
}

/// `path` made absolute with every symlink resolved. Fails when it does not exist.
pub(crate) fn canonicalize(path: &Path) -> Result<PathBuf> {
    path.canonicalize()
        .with_context(|| format!("could not resolve {}", path.display()))
}

/// The text the symlink at `path` points to, or `None` when it cannot be read or is not UTF-8.
#[cfg(target_os = "linux")]
pub(crate) fn link_text(path: &Path) -> Option<String> {
    fs::read_link(path)
        .ok()?
        .into_os_string()
        .into_string()
        .ok()
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

/// Creates the file at `path`, or empties it when it exists, and opens it for writing.
pub(crate) fn create_truncated(path: &Path) -> Result<File> {
    File::create(path).with_context(|| format!("could not create {}", path.display()))
}

/// Proof that this process holds a worktree's lock, an exclusive `flock` on the `lock` file in
/// its admin directory. The lock is released when the value is dropped, or when the process dies.
///
/// Writing a worktree's state takes a `&WorktreeLock`, so a write without the lock does not
/// compile.
#[derive(Debug)]
pub(crate) struct WorktreeLock {
    admin: AdminDir,
    /// Kept open for as long as the lock is held: closing it releases the lock.
    _file: File,
}

impl WorktreeLock {
    /// The admin directory of the worktree this lock is for.
    pub(crate) fn admin(&self) -> &AdminDir {
        &self.admin
    }
}

/// Takes the lock of the worktree whose admin directory is `admin` without waiting, creating the
/// directory and the lock file when missing. `None` when another process holds it.
pub(crate) fn try_lock_worktree(admin: &AdminDir) -> Result<Option<WorktreeLock>> {
    create_dir_all(admin.path())?;
    Ok(
        try_lock_file(&admin.lock_file())?.map(|FileLock(file)| WorktreeLock {
            admin: admin.clone(),
            _file: file,
        }),
    )
}

/// Whether the lock of the worktree whose admin directory is `admin` is held right now. Only
/// reads: a missing lock file is a free lock, and a lock taken to find out is released at once.
pub(crate) fn lock_status(admin: &AdminDir) -> Result<LockStatus> {
    lock_status_at(&admin.lock_file())
}

/// An exclusive `flock` on a file, held until the value is dropped or the process dies. The
/// holder keeps one on `agent.lock` for its whole life.
#[derive(Debug)]
pub(crate) struct FileLock(File);

/// Takes an exclusive `flock` on the file at `path` without waiting, creating the file when
/// missing. `None` when another process holds it.
pub(crate) fn try_lock_file(path: &Path) -> Result<Option<FileLock>> {
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .with_context(|| format!("could not open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(FileLock(file))),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(err)) => {
            Err(err).with_context(|| format!("could not lock {}", path.display()))
        }
    }
}

/// Takes an exclusive `flock` on the file at `path`, creating the file (never its directory) when
/// missing, trying again every 20 ms for at most `wait`. `None` when another process still holds
/// it then.
pub(crate) fn lock_file_waiting(path: &Path, wait: Duration) -> Result<Option<FileLock>> {
    let deadline = Instant::now() + wait;
    loop {
        if let Some(lock) = try_lock_file(path)? {
            return Ok(Some(lock));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// Whether the file at `path` is locked right now. Only reads: a missing file is a free lock, and
/// a lock taken to find out is released at once.
pub(crate) fn lock_status_at(path: &Path) -> Result<LockStatus> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(LockStatus::Free),
        Err(err) => return Err(err).with_context(|| format!("could not open {}", path.display())),
    };
    match file.try_lock() {
        // Dropping `file` at the end of this function releases the lock just taken.
        Ok(()) => Ok(LockStatus::Free),
        Err(TryLockError::WouldBlock) => Ok(LockStatus::Held),
        Err(TryLockError::Error(err)) => {
            Err(err).with_context(|| format!("could not probe {}", path.display()))
        }
    }
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

    // That a lock is free again once released is checked by the black-box tests in
    // `tests/worktree_setup.rs`, each in a process of its own: here, another test of this process
    // starting a child would hold a copy of the lock's file until the child runs its program.
    #[test]
    fn a_held_worktree_lock_is_seen_as_held() {
        let dir = TempDir::new().unwrap();
        let admin = AdminDir::new(dir.path().join("orca-term"));
        assert_eq!(lock_status(&admin).unwrap(), LockStatus::Free);
        assert!(!admin.path().exists(), "probing never creates anything");

        let lock = try_lock_worktree(&admin).unwrap().unwrap();
        assert_eq!(lock.admin(), &admin);
        assert_eq!(lock_status(&admin).unwrap(), LockStatus::Held);
        assert!(try_lock_worktree(&admin).unwrap().is_none());
    }

    // As above, that the lock is free again once dropped is left to the black-box tests
    // (`tests/agent.rs`, a holder's lock after `agent stop`).
    #[test]
    fn a_held_file_lock_is_seen_as_held_and_a_missing_file_is_free() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("agent.lock");
        assert_eq!(lock_status_at(&path).unwrap(), LockStatus::Free);
        let lock = try_lock_file(&path).unwrap().unwrap();
        assert_eq!(lock_status_at(&path).unwrap(), LockStatus::Held);
        assert!(try_lock_file(&path).unwrap().is_none());
        drop(lock);
    }

    #[test]
    fn executables_are_found_in_the_first_directory_that_has_them() {
        let dir = TempDir::new().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        fs::create_dir_all(&a).unwrap();
        fs::create_dir_all(&b).unwrap();
        fs::write(a.join("plain"), "").unwrap();
        fs::write(b.join("tool"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(b.join("tool"), fs::Permissions::from_mode(0o755)).unwrap();
        let dirs = [a.clone(), b.clone()];
        assert_eq!(executable_in(&dirs, "tool"), Some(b.join("tool")));
        assert_eq!(executable_in(&dirs, "plain"), None, "not executable");
        assert_eq!(executable_in(&dirs, "absent"), None);
        assert_eq!(list_dirs(dir.path()).unwrap(), [a, b]);
        assert!(list_dirs(&dir.path().join("none")).unwrap().is_empty());
    }

    #[test]
    fn a_tail_is_the_end_of_the_file_and_appends_stay_bounded() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("turns.jsonl");
        assert_eq!(read_tail(&path, 4).unwrap(), None);
        append(&path, b"one\n").unwrap();
        append(&path, b"two\n").unwrap();
        assert_eq!(read_tail(&path, 4).unwrap().unwrap(), b"two\n");
        assert_eq!(read_tail(&path, 100).unwrap().unwrap(), b"one\ntwo\n");

        let log = dir.path().join("hook.log");
        append_bounded(&log, b"0123456789", 8).unwrap();
        append_bounded(&log, b"ab", 8).unwrap();
        assert_eq!(
            fs::read(&log).unwrap(),
            b"ab",
            "emptied once past the bound"
        );
        append_bounded(&log, b"cd", 8).unwrap();
        assert_eq!(fs::read(&log).unwrap(), b"abcd");
        assert!(
            append(&dir.path().join("none/x"), b"x").is_err(),
            "no directory is made"
        );
    }

    // As above, taking the lock once it is free is left to the black-box tests (`tests/hooks.rs`).
    #[test]
    fn a_lock_held_elsewhere_is_waited_for_then_given_up() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("hook.lock");
        let held = try_lock_file(&path).unwrap().unwrap();
        let started = Instant::now();
        assert!(
            lock_file_waiting(&path, Duration::from_millis(100))
                .unwrap()
                .is_none()
        );
        assert!(started.elapsed() >= Duration::from_millis(100));
        drop(held);
    }

    #[test]
    fn removing_a_missing_file_is_not_an_error() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("gone");
        remove_if_exists(&path).unwrap();
        fs::write(&path, "x").unwrap();
        remove_if_exists(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn reading_a_missing_file_is_none_and_writes_nothing() {
        let dir = TempDir::new().unwrap();
        let absent = dir.path().join("absent");
        assert_eq!(read_optional(&absent).unwrap(), None);
        assert!(!absent.exists(), "reading never writes");
    }
}
