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
use crate::domain::snapshot::Identity;
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
    match find_default_branch(primary)? {
        Some(branch) => Ok(branch),
        None => bail!(
            "could not find the default branch: origin/HEAD is not set and there is no local \
             `main` or `master`"
        ),
    }
}

/// The repository's default branch when it exists as a local branch (`refs/heads/<default>`),
/// never `origin/`; `None` otherwise.
pub(crate) fn local_default_branch(primary: &PrimaryCheckout) -> Result<Option<Branch>> {
    let Some(default) = find_default_branch(primary)? else {
        return Ok(None);
    };
    let local = has_ref(&primary.path, &format!("refs/heads/{default}"))?;
    Ok(local.then_some(default))
}

/// The commits only on each side between the local branch `branch` and the `HEAD` of the
/// worktree at `worktree`, as `git rev-list --left-right --count refs/heads/<branch>...HEAD`
/// prints them: the branch's count, a tab, `HEAD`'s count.
pub(crate) fn left_right_counts(worktree: &Path, branch: &Branch) -> Result<String> {
    let range = format!("refs/heads/{branch}...HEAD");
    text(worktree, &["rev-list", "--left-right", "--count", &range])
}

/// The default branch as [`default_branch`] finds it, `None` when there is none.
fn find_default_branch(primary: &PrimaryCheckout) -> Result<Option<Branch>> {
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
            return Ok(Some(Branch::new(branch)));
        }
    }
    for candidate in ["main", "master"] {
        if has_ref(&primary.path, &format!("refs/heads/{candidate}"))? {
            return Ok(Some(Branch::new(candidate)));
        }
    }
    Ok(None)
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

/// Where a pull request into `base` is counted from (#132): the local branch `base`, else
/// `origin/<base>`, as [`start_point`] prefers them.
pub(crate) fn base_point(primary: &PrimaryCheckout, base: &Branch) -> Result<StartPoint> {
    if has_ref(&primary.path, &format!("refs/heads/{base}"))? {
        Ok(StartPoint::Local(base.clone()))
    } else if has_ref(&primary.path, &format!("refs/remotes/origin/{base}"))? {
        Ok(StartPoint::Remote(base.clone()))
    } else {
        bail!("the base branch `{base}` exists neither locally nor as origin/{base}")
    }
}

/// The messages of the commits of the worktree at `worktree` that `base` does not have, oldest
/// first: `git log -z --reverse --format=%s%x1f%b <base>..HEAD`, each subject and body split by
/// the unit separator (`domain/pr/create.rs` reads them). Writes nothing.
pub(crate) fn commits_ahead(worktree: &Path, base: &StartPoint) -> Result<Vec<u8>> {
    let range = format!("{base}..HEAD");
    output(
        worktree,
        &["log", "-z", "--reverse", "--format=%s%x1f%b", &range, "--"],
    )
}

/// Every file of the worktree at `worktree` with changes not committed, untracked ones included:
/// `git status --porcelain -z --untracked-files=all`. Writes nothing.
pub(crate) fn uncommitted_status(worktree: &Path) -> Result<Vec<u8>> {
    output(
        worktree,
        &["status", "--porcelain", "-z", "--untracked-files=all"],
    )
}

/// What `git push` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Pushed {
    /// The branch is on `origin`, set as its upstream.
    Done,
    /// git refused or failed, saying why on stderr.
    Failed(String),
}

/// `git push -u origin <branch>` in the worktree at `worktree`, never forced: only `pr create`
/// pushes (#132, `cargo xtask gh-guard`). git never prompts (`GIT_TERMINAL_PROMPT=0`, no input):
/// a push that needs credentials it cannot find fails with git's words instead of waiting for an
/// answer nobody sees, as in Neovim. Fails only when git cannot be started.
pub(crate) fn push_branch(worktree: &Path, branch: &Branch) -> Result<Pushed> {
    let out = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["push", "-u", "origin", branch.as_str()])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .context("could not run git; is it installed and on PATH?")?;
    if out.status.success() {
        return Ok(Pushed::Done);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Ok(Pushed::Failed(stderr.trim().to_owned()))
}

/// What `git fetch` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Fetched {
    /// The ref asked for holds what `origin` has.
    Done,
    /// git refused or failed, saying why on stderr.
    Failed(String),
}

/// `git fetch --no-tags origin <refspec>` in the worktree at `worktree`: the only fetch the core
/// makes, and only `worktree rebase` makes it (#141, `cargo xtask gh-guard`). git never prompts
/// (`GIT_TERMINAL_PROMPT=0`, no input), as [`push_branch`] does. Fails only when git cannot be
/// started.
pub(crate) fn fetch_into(worktree: &Path, refspec: &str) -> Result<Fetched> {
    let out = Command::new("git")
        .arg("-C")
        .arg(worktree)
        .args(["fetch", "--quiet", "--no-tags", "origin", refspec])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(Stdio::null())
        .output()
        .context("could not run git; is it installed and on PATH?")?;
    if out.status.success() {
        return Ok(Fetched::Done);
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    Ok(Fetched::Failed(stderr.trim().to_owned()))
}

/// The repository's default branch when it is on `origin`: the remote `origin` is configured and
/// git keeps a copy of the branch from it (`refs/remotes/origin/<default>`). `None` otherwise.
/// Never fetches.
pub(crate) fn remote_default_branch(primary: &PrimaryCheckout) -> Result<Option<Branch>> {
    let Some(default) = find_default_branch(primary)? else {
        return Ok(None);
    };
    if config_value(&primary.path, "remote.origin.url")?.is_none() {
        return Ok(None);
    }
    let tracked = has_ref(&primary.path, &format!("refs/remotes/origin/{default}"))?;
    Ok(tracked.then_some(default))
}

/// Whether the commit `ancestor` is an ancestor of `HEAD` in the worktree at `worktree`, or `HEAD`
/// itself (`git merge-base --is-ancestor <ancestor> HEAD`).
pub(crate) fn is_ancestor_of_head(worktree: &Path, ancestor: &str) -> Result<bool> {
    let args = ["merge-base", "--is-ancestor", ancestor, "HEAD"];
    Ok(probe(worktree, &args)?.is_some())
}

/// Where the worktree's `HEAD` forked from the ref `upstream`, judged from `upstream`'s reflog
/// (`git merge-base --fork-point <upstream> HEAD`), so commits the base dropped since are not
/// replayed. `None` when git finds none.
pub(crate) fn fork_point(worktree: &Path, upstream: &str) -> Result<Option<Commit>> {
    match probe(worktree, &["merge-base", "--fork-point", upstream, "HEAD"])? {
        Some(bytes) => Commit::parse(String::from_utf8_lossy(&bytes).trim_end()).map(Some),
        None => Ok(None),
    }
}

/// How many commits `HEAD` of the worktree at `worktree` has that `base` does not
/// (`git rev-list --count <base>..HEAD`).
pub(crate) fn count_ahead_of(worktree: &Path, base: &str) -> Result<usize> {
    let range = format!("{base}..HEAD");
    let count = text(worktree, &["rev-list", "--count", &range])?;
    count
        .trim()
        .parse()
        .with_context(|| format!("git rev-list --count printed {count:?}"))
}

/// What `git rebase` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RebaseRun {
    /// The rebase went through.
    Done,
    /// git stopped or failed, saying why on stderr (and stdout, where it reports conflicts).
    Stopped(String),
}

/// `git rebase --no-autostash --onto <onto> <fork_point>` in the worktree at `worktree`, or
/// `git rebase --no-autostash <onto>` with no fork point. Never stashes, never prompts
/// (`GIT_TERMINAL_PROMPT=0`, no input) and never opens an editor (`GIT_EDITOR=true`). Fails only
/// when git cannot be started.
pub(crate) fn rebase_onto(
    worktree: &Path,
    onto: &str,
    fork_point: Option<&Commit>,
) -> Result<RebaseRun> {
    let mut args = vec!["rebase", "--no-autostash"];
    match fork_point {
        Some(fork_point) => args.extend(["--onto", onto, fork_point.as_str()]),
        None => args.push(onto),
    }
    let out = quiet_git(worktree, &args)?;
    if out.status.success() {
        return Ok(RebaseRun::Done);
    }
    let said = [out.stderr.as_slice(), out.stdout.as_slice()].concat();
    Ok(RebaseRun::Stopped(
        String::from_utf8_lossy(&said).trim().to_owned(),
    ))
}

/// `git rebase --abort` in the worktree at `worktree`: puts the branch back where it was before
/// the rebase. Never prompts.
pub(crate) fn rebase_abort(worktree: &Path) -> Result<()> {
    let args = ["rebase", "--abort"];
    let out = quiet_git(worktree, &args)?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("git {} failed: {}", display_args(&args), stderr.trim());
    }
    Ok(())
}

/// The files of the worktree at `worktree` left in conflict, NUL-separated and relative to its
/// root (`git diff --name-only -z --diff-filter=U`). Writes nothing.
pub(crate) fn conflicted_files(worktree: &Path) -> Result<Vec<u8>> {
    output(
        worktree,
        &[
            "diff",
            "--name-only",
            "-z",
            "--no-ext-diff",
            "--diff-filter=U",
        ],
    )
}

/// Runs `git -C <dir> <args>` with no input, no prompt for credentials and no editor, as the
/// rebase does: nobody could answer them.
fn quiet_git(dir: &Path, args: &[&str]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .stdin(Stdio::null())
        .output()
        .context("could not run git; is it installed and on PATH?")
}

/// Whether the commit `tip` is merged into `into` (`git merge-base --is-ancestor`).
pub(crate) fn is_merged(
    primary: &PrimaryCheckout,
    tip: &Commit,
    into: &StartPoint,
) -> Result<bool> {
    let into = into.to_string();
    let args = ["merge-base", "--is-ancestor", tip.as_str(), &into];
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

/// The paths that differ between the merge base of `start` and `HEAD` and the files on disk of
/// the worktree at `worktree`, committed, staged or not, NUL-separated and relative to its root:
/// `git diff --name-only -z --find-renames --merge-base <start> --`, one process. A moved file
/// names only its new path. Untracked files are not in it ([`untracked_files`]). `None` when
/// `start` and `HEAD` share no commit (unrelated histories). Writes nothing.
pub(crate) fn changed_paths(worktree: &Path, start: &StartPoint) -> Result<Option<Vec<u8>>> {
    let start = start.to_string();
    let args = [
        "diff",
        "--name-only",
        "-z",
        "--no-ext-diff",
        "--find-renames",
        "--merge-base",
        start.as_str(),
        "--",
    ];
    let out = invoke(worktree, &args)?;
    if out.status.success() {
        return Ok(Some(out.stdout));
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    if stderr.contains("no merge base") {
        return Ok(None);
    }
    bail!("git {} failed: {}", display_args(&args), stderr.trim())
}

/// The untracked files of the worktree at `worktree` that git does not ignore, NUL-separated and
/// relative to its root: `git ls-files -z --others --exclude-standard`. Writes nothing.
pub(crate) fn untracked_files(worktree: &Path) -> Result<Vec<u8>> {
    output(
        worktree,
        &["ls-files", "-z", "--others", "--exclude-standard"],
    )
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

/// Deletes `branch` only if its tip is still `tip`: `git update-ref -d refs/heads/<branch> <tip>`,
/// which git refuses atomically when the branch moved. Then drops the branch's config section
/// (its upstream), as `git branch -D` does; a branch with none has nothing to drop.
pub(crate) fn delete_branch_at(
    primary: &PrimaryCheckout,
    branch: &Branch,
    tip: &Commit,
) -> Result<()> {
    let name = format!("refs/heads/{branch}");
    output(&primary.path, &["update-ref", "-d", &name, tip.as_str()])?;
    // The branch is gone whatever happens here, and git exits non-zero when there is no section,
    // so the outcome is not checked.
    let section = format!("branch.{branch}");
    let _ = probe(&primary.path, &["config", "--remove-section", &section]);
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

/// The worktree's real index file, absolute (`git rev-parse --git-path index`). Only read: a
/// snapshot copies it and never writes it.
pub(crate) fn index_path(worktree: &Path) -> Result<PathBuf> {
    let path = text(
        worktree,
        &["rev-parse", "--path-format=absolute", "--git-path", "index"],
    )?;
    Ok(PathBuf::from(path))
}

/// The tree of every file of the worktree at `worktree`, tracked and untracked, `.gitignore`
/// respected: `git add -A` then `git write-tree`, both through the index file `index`
/// (`GIT_INDEX_FILE`), a throwaway copy. The real index, `HEAD` and the branch are never touched.
pub(crate) fn write_tree_through(worktree: &Path, index: &Path) -> Result<String> {
    let env = [("GIT_INDEX_FILE", index.as_os_str())];
    run(worktree, &["add", "-A"], &env, None)?;
    let tree = run(worktree, &["write-tree"], &env, None)?;
    Ok(String::from_utf8_lossy(&tree).trim_end().to_owned())
}

/// The value git's config gives `key` in `dir` (`git config --get`), `None` when it is unset.
pub(crate) fn config_value(dir: &Path, key: &str) -> Result<Option<String>> {
    let value = probe(dir, &["config", "--get", key])?;
    Ok(value.map(|bytes| String::from_utf8_lossy(&bytes).trim_end().to_owned()))
}

/// The commit the ref `name` points to, `None` when there is no such ref.
pub(crate) fn read_ref(dir: &Path, name: &str) -> Result<Option<Commit>> {
    match probe(dir, &["rev-parse", "--verify", "--quiet", name])? {
        Some(bytes) => Commit::parse(String::from_utf8_lossy(&bytes).trim_end()).map(Some),
        None => Ok(None),
    }
}

/// A new commit of `tree`, with `parent` as its parent when given and `message` as its message,
/// authored by `author` and committed by `committer`, both at `date` (git's internal format):
/// `git commit-tree`, the identities and dates passed through the environment, so it works
/// without any `user.name` configured. `commit-tree` never signs, whatever the config says.
pub(crate) fn commit_tree(
    dir: &Path,
    tree: &str,
    parent: Option<&str>,
    message: &str,
    author: &Identity,
    committer: &Identity,
    date: &str,
) -> Result<Commit> {
    let mut args = vec!["commit-tree", tree];
    if let Some(parent) = parent {
        args.extend(["-p", parent]);
    }
    args.extend(["-F", "-"]);
    let env = [
        ("GIT_AUTHOR_NAME", OsStr::new(&author.name)),
        ("GIT_AUTHOR_EMAIL", OsStr::new(&author.email)),
        ("GIT_AUTHOR_DATE", OsStr::new(date)),
        ("GIT_COMMITTER_NAME", OsStr::new(&committer.name)),
        ("GIT_COMMITTER_EMAIL", OsStr::new(&committer.email)),
        ("GIT_COMMITTER_DATE", OsStr::new(date)),
    ];
    let out = run(dir, &args, &env, Some(message.as_bytes()))?;
    Commit::parse(String::from_utf8_lossy(&out).trim_end())
}

/// Points the ref `name` at `new`, only if it still holds `expected` (`None`: only if it does not
/// exist): `git update-ref <name> <new> <expected>`, a compare-and-swap.
pub(crate) fn update_ref(
    dir: &Path,
    name: &str,
    new: &Commit,
    expected: Option<&str>,
) -> Result<()> {
    let args = ["update-ref", name, new.as_str(), expected.unwrap_or("")];
    output(dir, &args)?;
    Ok(())
}

/// Deletes the ref `name` (`git update-ref -d`); nothing to do when it does not exist.
pub(crate) fn delete_ref(dir: &Path, name: &str) -> Result<()> {
    output(dir, &["update-ref", "-d", name])?;
    Ok(())
}

/// The full names of the refs under `prefix` (`git for-each-ref`), sorted.
pub(crate) fn refs_under(dir: &Path, prefix: &str) -> Result<Vec<String>> {
    let names = text(dir, &["for-each-ref", "--format=%(refname)", prefix])?;
    Ok(names.lines().map(str::to_owned).collect())
}

/// Whether the commit `id` exists in the repository of `dir` (`git rev-parse --verify
/// <id>^{commit}`).
pub(crate) fn commit_exists(dir: &Path, id: &str) -> Result<bool> {
    Ok(read_ref(dir, &format!("{id}^{{commit}}"))?.is_some())
}

/// Whether the commit `commit` holds the file `file` (relative to the root, `/`-separated):
/// `git cat-file -e <commit>:<file>`.
pub(crate) fn has_file(dir: &Path, commit: &str, file: &str) -> Result<bool> {
    Ok(probe(dir, &["cat-file", "-e", &format!("{commit}:{file}")])?.is_some())
}

/// The patch from the commit `begin` to the commit `end`, as `git diff <begin> <end>` prints it,
/// with no colour, external diff or text conversion a config may set. Writes nothing.
pub(crate) fn patch(dir: &Path, begin: &str, end: &str) -> Result<Vec<u8>> {
    output(
        dir,
        &[
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            begin,
            end,
            "--",
        ],
    )
}

/// Which commit of the history of `tip` each line of the file on disk `contents` (absolute) came
/// from, the file being `file` (relative to the root) in that history: `git blame --porcelain
/// --contents <contents> <tip> -- <file>`, with no revisions ignored whatever the config says. A
/// line in no commit gets the all-zero id. Writes nothing.
pub(crate) fn blame_porcelain(
    dir: &Path,
    tip: &str,
    contents: &Path,
    file: &str,
) -> Result<String> {
    let args = [
        OsStr::new("blame"),
        OsStr::new("--porcelain"),
        OsStr::new("--ignore-revs-file="),
        OsStr::new("--contents"),
        contents.as_os_str(),
        OsStr::new(tip),
        OsStr::new("--"),
        OsStr::new(file),
    ];
    Ok(String::from_utf8_lossy(&output(dir, &args)?).into_owned())
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

/// Runs `git -C <dir> <args>` with the variables `env` added to its environment and `stdin`, when
/// given, written to its standard input; returns its stdout. Fails when git does.
fn run(dir: &Path, args: &[&str], env: &[(&str, &OsStr)], stdin: Option<&[u8]>) -> Result<Vec<u8>> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .envs(env.iter().copied())
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("could not run git; is it installed and on PATH?")?;
    // git reads all of its input before it prints anything, so writing first cannot block.
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(bytes)
            .context("could not hand git its input")?;
    }
    let out = child.wait_with_output().context("could not run git")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        bail!("git {} failed: {}", display_args(args), stderr.trim());
    }
    Ok(out.stdout)
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
