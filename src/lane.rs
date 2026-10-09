//! Creating a lane: a worktree under `<base>/<repo>/<name>`, on a new branch from the default
//! branch, with the copy list copied in and its state written to git's admin directory.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::config::{Config, Env, Setting};
use crate::{fleet, git, include, state};

/// What `lane new` made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Created {
    pub name: String,
    pub path: PathBuf,
    pub branch: String,
    /// The commit-ish the branch starts from: the default branch, local or `origin/`.
    pub start: String,
    pub copied: usize,
    pub base: Setting<PathBuf>,
}

/// Creates the lane `name` in the repository containing `dir`.
pub fn create(dir: &Path, name: &str, env: &Env) -> Result<Created> {
    let primary = fleet::primary_checkout(dir)?.path;
    let primary = primary
        .canonicalize()
        .with_context(|| format!("could not resolve {}", primary.display()))?;
    validate_name(&primary, name)?;

    let common_dir = PathBuf::from(git::text(
        &primary,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?);
    let config = Config::load(&primary, &common_dir, env)?;
    let Some(repo) = primary.file_name() else {
        bail!(
            "the primary checkout {} has no directory name",
            primary.display()
        );
    };
    let path = config.base.value.join(repo).join(name);
    if physical(&path).starts_with(&primary) {
        bail!(
            "the lane would be at {}, inside the primary checkout; set `base` to a directory \
             outside it (base: {})",
            path.display(),
            config.base
        );
    }
    if path.symlink_metadata().is_ok() {
        bail!("{} already exists", path.display());
    }
    let start = start_point(&primary)?;

    let parent = path.parent().unwrap_or(&path);
    std::fs::create_dir_all(parent)
        .with_context(|| format!("could not create {}", parent.display()))?;
    git::output(
        &primary,
        &[
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("--quiet"),
            OsStr::new("-b"),
            OsStr::new(name),
            path.as_os_str(),
            OsStr::new(&start),
        ],
    )?;

    // From here on the worktree exists, and nothing is undone: a failure leaves it in place.
    let copied = finish(&primary, &path).with_context(|| {
        format!(
            "the worktree was created at {} and left in place, but setting up the lane failed",
            path.display()
        )
    })?;
    Ok(Created {
        name: name.to_owned(),
        path,
        branch: name.to_owned(),
        start,
        copied,
        base: config.base,
    })
}

/// Copies the copy list and writes the lane's state. Returns how many files were copied.
fn finish(primary: &Path, lane: &Path) -> Result<usize> {
    let copied = include::copy(primary, lane, &include::list(primary)?)?;
    state::write(&state::admin_dir(lane)?, &state::LaneFile::fresh())?;
    Ok(copied)
}

/// A lane name is a single path segment and a valid branch name.
fn validate_name(dir: &Path, name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("a lane name cannot be empty");
    }
    if name.contains('/') {
        bail!("invalid lane name `{name}`: it cannot contain `/`");
    }
    if name.starts_with('-') {
        bail!("invalid lane name `{name}`: it cannot start with `-`");
    }
    let normalized = git::probe(dir, &["check-ref-format", "--branch", name])?;
    if normalized.as_deref().map(<[u8]>::trim_ascii_end) != Some(name.as_bytes()) {
        bail!("invalid lane name `{name}`: it is not a valid branch name");
    }
    Ok(())
}

/// The commit-ish a lane branches from: the repository's default branch, preferring the local
/// branch over `origin/`.
fn start_point(primary: &Path) -> Result<String> {
    let default = default_branch(primary)?;
    if has_ref(primary, &format!("refs/heads/{default}"))? {
        Ok(default)
    } else if has_ref(primary, &format!("refs/remotes/origin/{default}"))? {
        Ok(format!("origin/{default}"))
    } else {
        bail!("the default branch `{default}` exists neither locally nor as origin/{default}")
    }
}

/// `origin/HEAD` when set, else a local `main`, else a local `master`.
fn default_branch(primary: &Path) -> Result<String> {
    if let Some(head) = git::probe(
        primary,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )? {
        let head = String::from_utf8_lossy(&head);
        let head = head.trim();
        if let Some(branch) = head.strip_prefix("origin/")
            && !branch.is_empty()
        {
            return Ok(branch.to_owned());
        }
    }
    for candidate in ["main", "master"] {
        if has_ref(primary, &format!("refs/heads/{candidate}"))? {
            return Ok(candidate.to_owned());
        }
    }
    bail!(
        "could not find the default branch: origin/HEAD is not set and there is no local `main` \
         or `master`"
    )
}

fn has_ref(dir: &Path, full_ref: &str) -> Result<bool> {
    Ok(git::probe(dir, &["show-ref", "--verify", "--quiet", full_ref])?.is_some())
}

/// `path` with its longest existing ancestor resolved through symlinks, so it can be compared with
/// a canonical path even before it exists.
fn physical(path: &Path) -> PathBuf {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_resolves_the_existing_prefix() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let real = tmp.path().canonicalize().unwrap();
        assert_eq!(
            physical(&tmp.path().join("not/yet/there")),
            real.join("not/yet/there")
        );
    }
}
