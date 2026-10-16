//! Adapter: git. Every git process the core starts is started here, one function per question
//! asked of git or per change made through it.
#![expect(
    clippy::disallowed_types,
    reason = "this adapter is the one place that starts git"
)]

use std::ffi::{OsStr, OsString};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use anyhow::{Context, Result, bail};

use crate::domain::branch::{Branch, StartPoint};
use crate::domain::fleet::{PrimaryCheckout, WorktreeEntry};
use crate::domain::note::BlobId;
use crate::domain::porcelain;
use crate::domain::review::Commit;
use crate::domain::state::{ADMIN_DIR_NAME, AdminDir};
use crate::domain::worktree::Removal;

/// Every worktree of the repository containing `dir`, primary checkout first. Never writes.
pub(crate) fn worktree_list(dir: &Path) -> Result<Vec<WorktreeEntry>> {
    let raw = output(dir, &["worktree", "list", "--porcelain", "-z"])?;
    Ok(porcelain::parse(&raw))
}

/// The primary checkout of the repository containing `dir`. Never writes.
pub(crate) fn primary_checkout(dir: &Path) -> Result<PrimaryCheckout> {
    match worktree_list(dir)?.into_iter().next() {
        Some(primary) => Ok(PrimaryCheckout::from_entry(primary)),
        None => bail!("git listed no worktrees"),
    }
}

/// The repository's common git directory, absolute.
pub(crate) fn common_dir(primary: &PrimaryCheckout) -> Result<PathBuf> {
    let dir = text(
        &primary.path,
        &["rev-parse", "--path-format=absolute", "--git-common-dir"],
    )?;
    Ok(PathBuf::from(dir))
}

/// The directory holding the state of the worktree at `worktree`, inside git's per-worktree admin
/// directory.
pub(crate) fn admin_dir(worktree: &Path) -> Result<AdminDir> {
    let dir = text(
        worktree,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            ADMIN_DIR_NAME,
        ],
    )?;
    Ok(AdminDir::new(PathBuf::from(dir)))
}

/// Whether git accepts `name` as a branch name exactly as written.
pub(crate) fn is_valid_branch_name(dir: &Path, name: &str) -> Result<bool> {
    let normalized = probe(dir, &["check-ref-format", "--branch", name])?;
    Ok(normalized.as_deref().map(<[u8]>::trim_ascii_end) == Some(name.as_bytes()))
}

/// The repository's default branch: `origin/HEAD` when set, else a local `main`, else a local
/// `master`.
pub(crate) fn default_branch(primary: &PrimaryCheckout) -> Result<Branch> {
    let origin_head = probe(
        &primary.path,
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
            return Ok(Branch::new(branch));
        }
    }
    for candidate in ["main", "master"] {
        if has_ref(&primary.path, &format!("refs/heads/{candidate}"))? {
            return Ok(Branch::new(candidate));
        }
    }
    bail!(
        "could not find the default branch: origin/HEAD is not set and there is no local `main` \
         or `master`"
    )
}

/// The commit-ish a worktree branches from: the repository's default branch, preferring the local
/// branch over `origin/`.
pub(crate) fn start_point(primary: &PrimaryCheckout) -> Result<StartPoint> {
    let default = default_branch(primary)?;
    if has_ref(&primary.path, &format!("refs/heads/{default}"))? {
        Ok(StartPoint::Local(default))
    } else if has_ref(&primary.path, &format!("refs/remotes/origin/{default}"))? {
        Ok(StartPoint::Remote(default))
    } else {
        bail!("the default branch `{default}` exists neither locally nor as origin/{default}")
    }
}

/// Whether `branch` is merged into `into` (`git merge-base --is-ancestor`).
pub(crate) fn is_merged(
    primary: &PrimaryCheckout,
    branch: &Branch,
    into: &StartPoint,
) -> Result<bool> {
    let into = into.to_string();
    let args = ["merge-base", "--is-ancestor", branch.as_str(), &into];
    Ok(probe(&primary.path, &args)?.is_some())
}

/// Where the history of the worktree at `worktree` leaves the default branch's: the best common
/// ancestor of `start` and the worktree's `HEAD` (`git merge-base <start> HEAD`). `None` when they
/// share no commit (unrelated histories), which git says by exiting 1 and printing nothing.
pub(crate) fn merge_base(worktree: &Path, start: &StartPoint) -> Result<Option<Commit>> {
    let start = start.to_string();
    let args = ["merge-base", start.as_str(), "HEAD"];
    let out = invoke(worktree, &args)?;
    if out.status.success() {
        let text = String::from_utf8_lossy(&out.stdout);
        return Commit::parse(text.trim_end()).map(Some);
    }
    // Exit 1 with nothing printed is git's answer "no common ancestor"; any other failure (a bad
    // ref, a repository git cannot read) is an error.
    if out.status.code() == Some(1) && out.stdout.is_empty() {
        return Ok(None);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    bail!("git {} failed: {}", display_args(&args), stderr.trim())
}

/// The untracked files the gitignore patterns in `patterns` match, NUL-separated.
pub(crate) fn untracked_matching(primary: &PrimaryCheckout, patterns: &Path) -> Result<Vec<u8>> {
    let mut exclude_from = OsString::from("--exclude-from=");
    exclude_from.push(patterns);
    output(
        &primary.path,
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
pub(crate) fn untracked_ignored(primary: &PrimaryCheckout) -> Result<Vec<u8>> {
    output(
        &primary.path,
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
pub(crate) fn worktree_add(
    primary: &PrimaryCheckout,
    branch: &Branch,
    path: &Path,
    start: &StartPoint,
) -> Result<()> {
    let start = start.to_string();
    output(
        &primary.path,
        &[
            OsStr::new("worktree"),
            OsStr::new("add"),
            OsStr::new("--quiet"),
            OsStr::new("-b"),
            OsStr::new(branch.as_str()),
            path.as_os_str(),
            OsStr::new(&start),
        ],
    )?;
    Ok(())
}

/// Whether the worktree at `worktree` has modified or untracked files: what makes
/// `git worktree remove` refuse it without `--force`. Ignored files do not count.
pub(crate) fn has_changes(worktree: &Path) -> Result<bool> {
    let status = output(
        worktree,
        &["status", "--porcelain", "--ignore-submodules=none"],
    )?;
    Ok(!status.is_empty())
}

/// `git worktree remove`, with `--force` when the removal is forced.
pub(crate) fn worktree_remove(
    primary: &PrimaryCheckout,
    path: &Path,
    removal: Removal,
) -> Result<()> {
    let mut args = vec![OsStr::new("worktree"), OsStr::new("remove")];
    if removal == Removal::Forced {
        args.push(OsStr::new("--force"));
    }
    args.push(path.as_os_str());
    output(&primary.path, &args)?;
    Ok(())
}

/// `git worktree prune`: forgets the worktrees whose directory is gone.
pub(crate) fn worktree_prune(primary: &PrimaryCheckout) -> Result<()> {
    output(&primary.path, &["worktree", "prune"])?;
    Ok(())
}

/// `git branch -D`: deletes `branch`, merged or not.
pub(crate) fn delete_branch(primary: &PrimaryCheckout, branch: &Branch) -> Result<()> {
    output(&primary.path, &["branch", "-D", branch.as_str()])?;
    Ok(())
}

/// The git blob id of a file whose content is `bytes`, as the repository containing `dir` names
/// objects, with no filter applied (`git hash-object --no-filters --stdin`). Writes nothing.
pub(crate) fn blob_id(dir: &Path, bytes: &[u8]) -> Result<BlobId> {
    let args = ["hash-object", "--no-filters", "--stdin"];
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not run git; is it installed and on PATH?")?;
    // git reads all of its input before it prints anything, so writing first cannot block.
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(bytes)
            .context("could not hand the file's content to git")?;
    }
    let out = child.wait_with_output().context("could not run git")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("git {} failed: {}", display_args(&args), stderr.trim());
    }
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(BlobId::parse(text.trim_end())?)
}

/// The differences from the file `old` to the file `new`, both absolute, as a diff with no
/// context lines (`git diff --no-index --unified=0`), compared as text whatever they hold and
/// with no external diff, text conversion or colour a config may set. Empty when they are the
/// same. Writes nothing.
pub(crate) fn diff_files(dir: &Path, old: &Path, new: &Path) -> Result<Vec<u8>> {
    let args = [
        OsStr::new("diff"),
        OsStr::new("--no-index"),
        OsStr::new("--no-color"),
        OsStr::new("--no-ext-diff"),
        OsStr::new("--no-textconv"),
        OsStr::new("--text"),
        OsStr::new("--unified=0"),
        OsStr::new("--"),
        old.as_os_str(),
        new.as_os_str(),
    ];
    let out = invoke(dir, &args)?;
    // With `--no-index`, git exits 0 when the files are the same and 1 when they differ.
    if let Some(0 | 1) = out.status.code() {
        return Ok(out.stdout);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    bail!("git {} failed: {}", display_args(&args), stderr.trim())
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
