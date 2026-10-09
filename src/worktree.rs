//! Creating a worktree: git's worktree at `<base>/<repo>/<name>`, on a new branch from the default
//! branch, with the copy list copied in and its state written to git's admin directory. Removing
//! a worktree, and pruning the worktrees whose directory is gone.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::config::{Config, Env, Setting};
use crate::fleet::{Fleet, Worktree};
use crate::{fleet, git, include, state};

/// What `worktree new` made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Created {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) branch: String,
    /// The commit-ish the branch starts from: the default branch, local or `origin/`.
    pub(crate) start: String,
    pub(crate) copied: usize,
    pub(crate) base: Setting<PathBuf>,
}

/// Creates the worktree `name` in the repository containing `dir`.
pub(crate) fn create(dir: &Path, name: &str, env: &Env) -> Result<Created> {
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
            "the worktree would be at {}, inside the primary checkout; set `base` to a directory \
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
            "the worktree was created at {} and left in place, but setting it up failed",
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

/// Copies the copy list and writes the worktree's state. Returns how many files were copied.
fn finish(primary: &Path, worktree: &Path) -> Result<usize> {
    let copied = include::copy(primary, worktree, &include::list(primary)?)?;
    state::write(&state::admin_dir(worktree)?, &state::WorktreeFile::fresh())?;
    Ok(copied)
}

/// A worktree name is a single path segment and a valid branch name.
fn validate_name(dir: &Path, name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("a worktree name cannot be empty");
    }
    if name.contains('/') {
        bail!("invalid worktree name `{name}`: it cannot contain `/`");
    }
    if name.starts_with('-') {
        bail!("invalid worktree name `{name}`: it cannot start with `-`");
    }
    let normalized = git::probe(dir, &["check-ref-format", "--branch", name])?;
    if normalized.as_deref().map(<[u8]>::trim_ascii_end) != Some(name.as_bytes()) {
        bail!("invalid worktree name `{name}`: it is not a valid branch name");
    }
    Ok(())
}

/// The commit-ish a worktree branches from: the repository's default branch, preferring the local
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

/// What `worktree rm` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Removed {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) branch: BranchOutcome,
}

/// What `worktree rm` did with the worktree's branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BranchOutcome {
    /// Deleted because it is merged into `into`.
    DeletedMerged { branch: String, into: String },
    /// Deleted because of `--force`.
    DeletedForced { branch: String },
    /// Kept, for `reason`.
    Kept { branch: String, reason: String },
    /// The worktree was detached: there was no branch.
    Detached,
}

/// Removes the worktree named or located by `target` in the repository containing `dir`. Its branch
/// is deleted only when merged into the default branch, or when `force` is set.
pub(crate) fn remove(dir: &Path, target: &str, force: bool) -> Result<Removed> {
    let fleet = Fleet::discover(dir)?;
    let worktree = resolve(&fleet, dir, target)?;
    if worktree.gone {
        bail!(
            "worktree {} at {} is broken ({}); only `orca-term worktree prune` cleans a broken worktree",
            worktree.name,
            worktree.path.display(),
            worktree
                .detail
                .as_deref()
                .unwrap_or("its directory is gone")
        );
    }
    let primary = &fleet.primary.path;

    // Decided before the worktree goes, so a failure here leaves everything in place.
    // The default branch is never deleted, not even with `--force`: it is trivially merged into
    // itself.
    let default = default_branch(primary).ok();
    let merged = match (&worktree.branch, force) {
        (Some(branch), _) if default.as_deref() == Some(branch.as_str()) => {
            Some(Err("it is the default branch".to_owned()))
        }
        (Some(branch), false) => Some(merged_into_default(primary, branch)?),
        _ => None,
    };

    remove_worktree_directory(primary, &worktree.path, force)?;
    let branch = settle_branch(primary, worktree.branch.as_deref(), merged);
    Ok(Removed {
        name: worktree.name.clone(),
        path: worktree.path.clone(),
        branch,
    })
}

/// Runs `git worktree remove` on the worktree at `path`, with `--force` when `force` is set.
fn remove_worktree_directory(primary: &Path, path: &Path, force: bool) -> Result<()> {
    let mut args = vec![OsStr::new("worktree"), OsStr::new("remove")];
    if force {
        args.push(OsStr::new("--force"));
    }
    args.push(path.as_os_str());
    git::output(primary, &args).with_context(|| {
        let hint = if force {
            String::new()
        } else {
            "; pass --force to remove it anyway, discarding its changes".to_owned()
        };
        format!("could not remove worktree {}{hint}", path.display())
    })?;
    Ok(())
}

/// Deletes or keeps the removed worktree's branch, following the decision `remove` made before
/// the worktree went: `None` deletes it (forced), `Some(Ok(into))` deletes it as merged into
/// `into`, `Some(Err(reason))` keeps it.
fn settle_branch(
    primary: &Path,
    branch: Option<&str>,
    merged: Option<Result<String, String>>,
) -> BranchOutcome {
    match (branch, merged) {
        (None, _) => BranchOutcome::Detached,
        (Some(branch), None) => delete_branch(
            primary,
            branch,
            BranchOutcome::DeletedForced {
                branch: branch.to_owned(),
            },
        ),
        (Some(branch), Some(Ok(into))) => delete_branch(
            primary,
            branch,
            BranchOutcome::DeletedMerged {
                branch: branch.to_owned(),
                into,
            },
        ),
        (Some(branch), Some(Err(reason))) => BranchOutcome::Kept {
            branch: branch.to_owned(),
            reason,
        },
    }
}

/// Runs `git worktree prune` from the primary checkout and returns the paths of the worktrees it
/// cleaned. Only ever invoked by the person, through `worktree prune`.
pub(crate) fn prune(dir: &Path) -> Result<Vec<PathBuf>> {
    let before = Fleet::discover(dir)?;
    git::output(&before.primary.path, &["worktree", "prune"])?;
    let after = Fleet::discover(&before.primary.path)?;
    Ok(before
        .gone_worktrees()
        .filter(|gone| {
            !after
                .worktrees
                .iter()
                .any(|worktree| worktree.path == gone.path)
        })
        .map(|worktree| worktree.path.clone())
        .collect())
}

/// The worktree `target` names (a basename) or locates (a path, when it contains `/` or is `.` or
/// `..`). Refuses the primary checkout, an unknown worktree and a name shared by several worktrees.
fn resolve<'f>(fleet: &'f Fleet, dir: &Path, target: &str) -> Result<&'f Worktree> {
    let primary = &fleet.primary.path;
    if target.contains('/') || target == "." || target == ".." {
        let wanted = dir.join(target);
        let same = |path: &Path| {
            path == wanted
                || matches!(
                    (path.canonicalize(), wanted.canonicalize()),
                    (Ok(a), Ok(b)) if a == b
                )
        };
        if same(primary) {
            bail!(
                "{} is the primary checkout, which is never a worktree",
                primary.display()
            );
        }
        return match fleet.worktrees.iter().find(|worktree| same(&worktree.path)) {
            Some(worktree) => Ok(worktree),
            None => bail!("{} is not a worktree of this repository", wanted.display()),
        };
    }
    let matches: Vec<&Worktree> = fleet
        .worktrees
        .iter()
        .filter(|l| l.name == target)
        .collect();
    match matches.as_slice() {
        [worktree] => Ok(worktree),
        [] if primary.file_name() == Some(OsStr::new(target)) => bail!(
            "{} is the primary checkout, which is never a worktree",
            primary.display()
        ),
        [] => bail!("no worktree named `{target}`; `orca-term worktree ls` lists the fleet"),
        several => {
            let paths: Vec<String> = several
                .iter()
                .map(|worktree| format!("  {}", worktree.path.display()))
                .collect();
            bail!(
                "`{target}` names {} worktrees; pass the path of the one to remove:\n{}",
                several.len(),
                paths.join("\n")
            )
        }
    }
}

/// `Ok(default)` when `branch` is merged into the default branch, `Err(reason)` to keep it.
fn merged_into_default(
    primary: &Path,
    branch: &str,
) -> Result<std::result::Result<String, String>> {
    let start = match start_point(primary) {
        Ok(start) => start,
        Err(err) => return Ok(Err(format!("{err:#}"))),
    };
    let merged = git::probe(primary, &["merge-base", "--is-ancestor", branch, &start])?.is_some();
    Ok(if merged {
        Ok(start)
    } else {
        Err(format!("not merged into {start}"))
    })
}

/// Deletes `branch`, returning `deleted`, or why it was kept when git refuses.
fn delete_branch(primary: &Path, branch: &str, deleted: BranchOutcome) -> BranchOutcome {
    match git::output(primary, &["branch", "-D", branch]) {
        Ok(_) => deleted,
        Err(err) => BranchOutcome::Kept {
            branch: branch.to_owned(),
            reason: format!("could not delete it: {err:#}"),
        },
    }
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
