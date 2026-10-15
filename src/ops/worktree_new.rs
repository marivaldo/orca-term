//! Ops: `worktree new`, which creates git's worktree at `<base>/<repo>/<name>` on a new branch
//! from the default branch, copies the copy list in and runs the worktree's setup.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::adapters::{env, fs, git};
use crate::domain::config;
use crate::domain::fleet::PrimaryCheckout;
use crate::domain::include;
use crate::domain::setup::SetupReport;
use crate::domain::worktree::{self, Created, WorktreeName};
use crate::ops::{config_layers, lock_worktree, notify_changed, set_up};

/// Creates the worktree `name` in the repository containing the current directory, echoing its
/// setup's output to `echo`. A failed setup is not an error: the report says the worktree is not
/// ready.
pub(crate) fn run(name: &WorktreeName, echo: &mut dyn Write) -> Result<Created> {
    let cwd = env::current_dir()?;
    let env = env::config_env();
    let listed = git::primary_checkout(&cwd)?;
    let primary = PrimaryCheckout {
        path: fs::canonicalize(&listed.path)?,
        branch: listed.branch,
    };
    // The name already follows git's rules for a branch name; git still has the last word, in
    // case a newer git adds a rule.
    if !git::is_valid_branch_name(&primary.path, name.as_str())? {
        bail!("invalid worktree name `{name}`: it is not a valid branch name");
    }

    let base = config::resolve_base(&config_layers(&primary, &primary.path, &env)?, &env)?;
    let path = worktree::path_for(&base.value, &primary, name)?;
    if fs::physical(&path).starts_with(&primary.path) {
        bail!(
            "the worktree would be at {}, inside the primary checkout; set `base` to a directory \
             outside it (base: {})",
            path.display(),
            base
        );
    }
    if fs::exists(&path) {
        bail!("{} already exists", path.display());
    }
    let start = git::start_point(&primary)?;
    let branch = name.branch();

    fs::create_dir_all(path.parent().unwrap_or(&path))?;
    git::worktree_add(&primary, &branch, &path, &start)?;

    // From here on the worktree exists, and nothing is undone: a failure leaves it in place.
    let (copied, setup) = finish(name, &primary, &path, echo).with_context(|| {
        format!(
            "the worktree was created at {} and left in place, but finishing it failed",
            path.display()
        )
    })?;
    Ok(Created {
        name: name.clone(),
        path,
        branch,
        start,
        copied,
        base,
        setup,
    })
}

/// Locks the new worktree, tells the running Neovims it exists, copies the copy list and runs the
/// setup (which tells them again as it starts and ends). Returns how many files were copied and
/// what the setup did. The lock is taken before anything else touches the worktree and held until
/// its setup ends, so nothing else may use the worktree before it is ready.
fn finish(
    name: &WorktreeName,
    primary: &PrimaryCheckout,
    worktree: &Path,
    echo: &mut dyn Write,
) -> Result<(usize, SetupReport)> {
    let lock = lock_worktree(name.as_str(), worktree)?;
    notify_changed(lock.admin());
    let copied = copy_include_list(primary, worktree, &include_list(primary)?)?;
    let setup = set_up(&lock, primary, worktree, echo)?;
    Ok((copied, setup))
}

/// The repository-relative paths to copy from `primary`. Empty when there is no
/// `.worktreeinclude`.
fn include_list(primary: &PrimaryCheckout) -> Result<Vec<PathBuf>> {
    let patterns = primary.path.join(include::FILE_NAME);
    if !fs::is_file(&patterns) {
        return Ok(Vec::new());
    }
    let matching = git::untracked_matching(primary, &patterns)?;
    let ignored = git::untracked_ignored(primary)?;
    Ok(include::copyable(&matching, &ignored))
}

/// Copies each of `paths` from `primary` into `worktree` as plain bytes, returning how many files
/// were copied. Symlinks and anything else that is not a regular file are skipped, and nothing is
/// written outside the worktree.
fn copy_include_list(
    primary: &PrimaryCheckout,
    worktree: &Path,
    paths: &[PathBuf],
) -> Result<usize> {
    let worktree_real = fs::canonicalize(worktree)?;
    let mut copied = 0;
    for rel in paths {
        if !include::stays_inside(rel) {
            bail!(
                "refusing to copy {}: it escapes the worktree",
                rel.display()
            );
        }
        let from = primary.path.join(rel);
        let Some(mode) = fs::regular_file_mode(&from)? else {
            continue;
        };
        let to = worktree.join(rel);
        let parent = to.parent().unwrap_or(worktree);
        fs::create_dir_all(parent)?;
        if !fs::canonicalize(parent)?.starts_with(&worktree_real) || fs::is_symlink(&to) {
            bail!(
                "refusing to copy {}: it would land outside the worktree",
                rel.display()
            );
        }
        fs::byte_copy(&from, &to, mode)
            .with_context(|| format!("could not copy {} into the worktree", rel.display()))?;
        copied += 1;
    }
    Ok(copied)
}
