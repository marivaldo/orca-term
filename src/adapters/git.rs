//! Adapter: git. Every git process the core starts is started here, one function per question
//! asked of git or per change made through it.
#![expect(
    clippy::disallowed_types,
    reason = "this adapter is the one place that starts git"
)]

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};

use crate::domain::fleet::WorktreeEntry;
use crate::domain::porcelain;
use crate::domain::state::ADMIN_DIR_NAME;

/// Every worktree of the repository containing `dir`, primary checkout first. Never writes.
pub(crate) fn worktree_list(dir: &Path) -> Result<Vec<WorktreeEntry>> {
    let raw = output(dir, &["worktree", "list", "--porcelain", "-z"])?;
    Ok(porcelain::parse(&raw))
}

/// The primary checkout of the repository containing `dir`. Never writes.
pub(crate) fn primary_checkout(dir: &Path) -> Result<WorktreeEntry> {
    match worktree_list(dir)?.into_iter().next() {
        Some(primary) => Ok(primary),
        None => bail!("git listed no worktrees"),
    }
}

/// The repository's common git directory, absolute.
pub(crate) fn common_dir(primary: &Path) -> Result<PathBuf> {
    let dir = text(
        primary,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    Ok(PathBuf::from(dir))
}

/// The directory holding the state of the worktree at `worktree`, inside git's per-worktree admin
/// directory.
pub(crate) fn admin_dir(worktree: &Path) -> Result<PathBuf> {
    let dir = text(
        worktree,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            ADMIN_DIR_NAME,
        ],
    )?;
    Ok(PathBuf::from(dir))
}

/// Whether git accepts `name` as a branch name exactly as written.
pub(crate) fn is_valid_branch_name(dir: &Path, name: &str) -> Result<bool> {
    let normalized = probe(dir, &["check-ref-format", "--branch", name])?;
    Ok(normalized.as_deref().map(<[u8]>::trim_ascii_end) == Some(name.as_bytes()))
}

/// The repository's default branch: `origin/HEAD` when set, else a local `main`, else a local
/// `master`.
pub(crate) fn default_branch(primary: &Path) -> Result<String> {
    let origin_head = probe(
        primary,
        &[
            "symbolic-ref",
            "--quiet",
            "--short",
            "refs/remotes/origin/HEAD",
        ],
    )?;
    if let Some(head) = origin_head {
        let head = String::from_utf8_lossy(&head);
        if let Some(branch) = head.trim().strip_prefix("origin/")
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

/// The commit-ish a worktree branches from: the repository's default branch, preferring the local
/// branch over `origin/`.
pub(crate) fn start_point(primary: &Path) -> Result<String> {
    let default = default_branch(primary)?;
    if has_ref(primary, &format!("refs/heads/{default}"))? {
        Ok(default)
    } else if has_ref(primary, &format!("refs/remotes/origin/{default}"))? {
        Ok(format!("origin/{default}"))
    } else {
        bail!("the default branch `{default}` exists neither locally nor as origin/{default}")
    }
}

/// Whether `branch` is merged into `into` (`git merge-base --is-ancestor`).
pub(crate) fn is_merged(primary: &Path, branch: &str, into: &str) -> Result<bool> {
    Ok(probe(primary, &["merge-base", "--is-ancestor", branch, into])?.is_some())
}

/// The untracked files the gitignore patterns in `patterns` match, NUL-separated.
pub(crate) fn untracked_matching(primary: &Path, patterns: &Path) -> Result<Vec<u8>> {
    let mut exclude_from = OsString::from("--exclude-from=");
    exclude_from.push(patterns);
    output(
        primary,
        &[
            OsStr::new("ls-files"),
            OsStr::new("-z"),
            OsStr::new("--others"),
            OsStr::new("--ignored"),
            exclude_from.as_os_str(),
        ],
    )
}

/// The untracked files the repository's own excludes ignore, NUL-separated.
pub(crate) fn untracked_ignored(primary: &Path) -> Result<Vec<u8>> {
    output(
        primary,
        &[
            "ls-files",
            "-z",
            "--others",
            "--ignored",
            "--exclude-standard",
        ],
    )
}

/// `git worktree add`: creates the worktree at `path` on the new branch `branch`, from `start`.
pub(crate) fn worktree_add(primary: &Path, branch: &str, path: &Path, start: &str) -> Result<()> {
    output(
        primary,
        &[
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("--quiet"),
            OsStr::new("-b"),
            OsStr::new(branch),
            path.as_os_str(),
            OsStr::new(start),
        ],
    )?;
    Ok(())
}

/// `git worktree remove`, with `--force` when `force` is set.
pub(crate) fn worktree_remove(primary: &Path, path: &Path, force: bool) -> Result<()> {
    let mut args = vec![OsStr::new("worktree"), OsStr::new("remove")];
    if force {
        args.push(OsStr::new("--force"));
    }
    args.push(path.as_os_str());
    output(primary, &args)?;
    Ok(())
}

/// `git worktree prune`: forgets the worktrees whose directory is gone.
pub(crate) fn worktree_prune(primary: &Path) -> Result<()> {
    output(primary, &["worktree", "prune"])?;
    Ok(())
}

/// `git branch -D`: deletes `branch`, merged or not.
pub(crate) fn delete_branch(primary: &Path, branch: &str) -> Result<()> {
    output(primary, &["branch", "-D", branch])?;
    Ok(())
}

fn has_ref(dir: &Path, full_ref: &str) -> Result<bool> {
    Ok(probe(dir, &["show-ref", "--verify", "--quiet", full_ref])?.is_some())
}

/// Runs `git -C <dir> <args>` and returns its stdout as raw bytes. Fails when git does.
fn output<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> Result<Vec<u8>> {
    let out = invoke(dir, args)?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stderr = stderr.trim();
        if stderr.contains("not a git repository") {
            bail!("not inside a git repository: {}", dir.display());
        }
        bail!("git {} failed: {stderr}", display_args(args));
    }
    Ok(out.stdout)
}

/// Runs `git -C <dir> <args>` and returns its stdout, or `None` when git exits non-zero: for
/// questions git answers with its exit status, such as whether a ref exists.
fn probe<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> Result<Option<Vec<u8>>> {
    let out = invoke(dir, args)?;
    Ok(out.status.success().then_some(out.stdout))
}

/// Like [`output`], trimmed and decoded as UTF-8 text.
fn text<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> Result<String> {
    let raw = output(dir, args)?;
    let text = String::from_utf8(raw)
        .with_context(|| format!("git {} printed non-UTF-8 output", display_args(args)))?;
    Ok(text.trim_end_matches(['\n', '\r']).to_owned())
}

fn invoke<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("could not run git; is it installed and on PATH?")
}

fn display_args<S: AsRef<OsStr>>(args: &[S]) -> String {
    args.iter()
        .map(|a| a.as_ref().to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}
