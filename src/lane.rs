//! Creating a lane: a worktree under `<base>/<repo>/<name>`, on a new branch from the default
//! branch, with the copy list copied in and its state written to git's admin directory. Removing
//! a lane, and pruning the lanes whose directory is gone.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use crate::config::{Config, Env, Setting};
use crate::fleet::{Fleet, Lane};
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

/// What `lane rm` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removed {
    pub name: String,
    pub path: PathBuf,
    pub branch: BranchOutcome,
}

/// What `lane rm` did with the lane's branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BranchOutcome {
    /// Deleted because it is merged into `into`.
    DeletedMerged { branch: String, into: String },
    /// Deleted because of `--force`.
    DeletedForced { branch: String },
    /// Kept, for `reason`.
    Kept { branch: String, reason: String },
    /// The lane was detached: there was no branch.
    Detached,
}

/// Removes the lane named or located by `target` in the repository containing `dir`. Its branch
/// is deleted only when merged into the default branch, or when `force` is set.
pub fn remove(dir: &Path, target: &str, force: bool) -> Result<Removed> {
    let fleet = Fleet::discover(dir)?;
    let lane = resolve(&fleet, dir, target)?;
    if lane.gone {
        bail!(
            "lane {} at {} is broken ({}); only `orca-term lane prune` cleans a broken lane",
            lane.name,
            lane.path.display(),
            lane.detail.as_deref().unwrap_or("its worktree is gone")
        );
    }
    let primary = &fleet.primary.path;

    // Decided before the worktree goes, so a failure here leaves everything in place.
    let merged = match (&lane.branch, force) {
        (Some(branch), false) => Some(merged_into_default(primary, branch)?),
        _ => None,
    };

    let mut args = vec![OsStr::new("worktree"), OsStr::new("remove")];
    if force {
        args.push(OsStr::new("--force"));
    }
    args.push(lane.path.as_os_str());
    git::output(primary, &args).with_context(|| {
        let hint = if force {
            String::new()
        } else {
            "; pass --force to remove it anyway, discarding its changes".to_owned()
        };
        format!("could not remove lane {}{hint}", lane.path.display())
    })?;

    let branch = match (&lane.branch, merged) {
        (None, _) => BranchOutcome::Detached,
        (Some(branch), None) => delete_branch(
            primary,
            branch,
            BranchOutcome::DeletedForced {
                branch: branch.clone(),
            },
        ),
        (Some(branch), Some(Ok(into))) => delete_branch(
            primary,
            branch,
            BranchOutcome::DeletedMerged {
                branch: branch.clone(),
                into,
            },
        ),
        (Some(branch), Some(Err(reason))) => BranchOutcome::Kept {
            branch: branch.clone(),
            reason,
        },
    };
    Ok(Removed {
        name: lane.name.clone(),
        path: lane.path.clone(),
        branch,
    })
}

/// Runs `git worktree prune` from the primary checkout and returns the paths of the lanes it
/// cleaned. Only ever invoked by the person, through `lane prune`.
pub fn prune(dir: &Path) -> Result<Vec<PathBuf>> {
    let before = Fleet::discover(dir)?;
    git::output(&before.primary.path, &["worktree", "prune"])?;
    let after = Fleet::discover(&before.primary.path)?;
    Ok(before
        .gone_lanes()
        .filter(|gone| !after.lanes.iter().any(|lane| lane.path == gone.path))
        .map(|lane| lane.path.clone())
        .collect())
}

/// The lane `target` names (a basename) or locates (a path, when it contains `/` or is `.` or
/// `..`). Refuses the primary checkout, an unknown lane and a name shared by several lanes.
fn resolve<'f>(fleet: &'f Fleet, dir: &Path, target: &str) -> Result<&'f Lane> {
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
                "{} is the primary checkout, which is never a lane",
                primary.display()
            );
        }
        return match fleet.lanes.iter().find(|lane| same(&lane.path)) {
            Some(lane) => Ok(lane),
            None => bail!("{} is not a lane of this repository", wanted.display()),
        };
    }
    let matches: Vec<&Lane> = fleet.lanes.iter().filter(|l| l.name == target).collect();
    match matches.as_slice() {
        [lane] => Ok(lane),
        [] if primary.file_name() == Some(OsStr::new(target)) => bail!(
            "{} is the primary checkout, which is never a lane",
            primary.display()
        ),
        [] => bail!("no lane named `{target}`; `orca-term lane ls` lists the fleet"),
        several => {
            let paths: Vec<String> = several
                .iter()
                .map(|lane| format!("  {}", lane.path.display()))
                .collect();
            bail!(
                "`{target}` names {} lanes; pass the path of the one to remove:\n{}",
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
