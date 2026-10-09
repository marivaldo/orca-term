//! Domain: the rules for one worktree. What its name may be, where it goes, which worktree a
//! `worktree rm` target means, and what happens to its branch when it is removed.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::domain::config::Setting;
use crate::domain::fleet::{Fleet, Worktree};

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

/// What `worktree rm` decides to do with the worktree's branch, before the worktree goes. A
/// decided deletion is carried out afterwards, and becomes `Kept` if git refuses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BranchFate {
    /// Decided without asking git.
    Decided(BranchOutcome),
    /// Delete `branch` only when it is merged into the default branch, which git has to answer.
    IfMerged { branch: String },
}

/// The rules a worktree name must follow on its own: one path segment that cannot be read as an
/// option. Git decides separately whether it is a valid branch name.
pub(crate) fn check_name(name: &str) -> Result<()> {
    if name.is_empty() {
        bail!("a worktree name cannot be empty");
    }
    if name.contains('/') {
        bail!("invalid worktree name `{name}`: it cannot contain `/`");
    }
    if name.starts_with('-') {
        bail!("invalid worktree name `{name}`: it cannot start with `-`");
    }
    Ok(())
}

/// Where the worktree `name` of the repository whose primary checkout is `primary` goes:
/// `<base>/<repo>/<name>`, where `<repo>` is the primary checkout's directory name.
pub(crate) fn path_for(base: &Path, primary: &Path, name: &str) -> Result<PathBuf> {
    let Some(repo) = primary.file_name() else {
        bail!(
            "the primary checkout {} has no directory name",
            primary.display()
        );
    };
    Ok(base.join(repo).join(name))
}

/// Whether a `worktree rm` target locates a worktree by path (it contains `/` or is `.` or `..`)
/// rather than naming it.
pub(crate) fn target_is_path(target: &str) -> bool {
    target.contains('/') || target == "." || target == ".."
}

/// The worktree named `name` (a basename). Refuses the primary checkout, an unknown worktree and a
/// name shared by several worktrees.
pub(crate) fn find_by_name<'f>(fleet: &'f Fleet, name: &str) -> Result<&'f Worktree> {
    let primary = &fleet.primary.path;
    let matches: Vec<&Worktree> = fleet.worktrees.iter().filter(|l| l.name == name).collect();
    match matches.as_slice() {
        [worktree] => Ok(worktree),
        [] if primary.file_name() == Some(OsStr::new(name)) => bail!(
            "{} is the primary checkout, which is never a worktree",
            primary.display()
        ),
        [] => bail!("no worktree named `{name}`; `orca-term worktree ls` lists the fleet"),
        several => {
            let paths: Vec<String> = several
                .iter()
                .map(|worktree| format!("  {}", worktree.path.display()))
                .collect();
            bail!(
                "`{name}` names {} worktrees; pass the path of the one to remove:\n{}",
                several.len(),
                paths.join("\n")
            )
        }
    }
}

/// Refuses to remove a worktree whose directory is gone: only `worktree prune` cleans it.
pub(crate) fn check_removable(worktree: &Worktree) -> Result<()> {
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
    Ok(())
}

/// What to do with `branch`, the removed worktree's branch, given the repository's `default`
/// branch when known. The default branch is never deleted, not even with `force`: it is trivially
/// merged into itself.
pub(crate) fn branch_fate(branch: Option<&str>, default: Option<&str>, force: bool) -> BranchFate {
    let Some(branch) = branch else {
        return BranchFate::Decided(BranchOutcome::Detached);
    };
    let branch = branch.to_owned();
    if default == Some(branch.as_str()) {
        BranchFate::Decided(BranchOutcome::Kept {
            branch,
            reason: "it is the default branch".to_owned(),
        })
    } else if force {
        BranchFate::Decided(BranchOutcome::DeletedForced { branch })
    } else {
        BranchFate::IfMerged { branch }
    }
}

/// What happens to `branch` once git said whether it is merged into `start`, the default branch
/// it is compared with: deleted when merged, kept otherwise.
pub(crate) fn merged_outcome(branch: String, start: String, merged: bool) -> BranchOutcome {
    if merged {
        BranchOutcome::DeletedMerged {
            branch,
            into: start,
        }
    } else {
        BranchOutcome::Kept {
            branch,
            reason: format!("not merged into {start}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_one_segment_that_is_not_an_option() {
        assert!(check_name("fix-login").is_ok());
        assert!(check_name("").is_err());
        assert!(check_name("a/b").is_err());
        assert!(check_name("-x").is_err());
    }

    #[test]
    fn the_default_branch_is_kept_even_with_force() {
        assert_eq!(
            branch_fate(Some("main"), Some("main"), true),
            BranchFate::Decided(BranchOutcome::Kept {
                branch: "main".to_owned(),
                reason: "it is the default branch".to_owned()
            })
        );
        assert_eq!(
            branch_fate(Some("fix"), Some("main"), true),
            BranchFate::Decided(BranchOutcome::DeletedForced {
                branch: "fix".to_owned()
            })
        );
        assert_eq!(
            branch_fate(Some("fix"), None, false),
            BranchFate::IfMerged {
                branch: "fix".to_owned()
            }
        );
        assert_eq!(
            branch_fate(None, Some("main"), true),
            BranchFate::Decided(BranchOutcome::Detached)
        );
    }

    #[test]
    fn an_unmerged_branch_is_kept_and_says_why() {
        assert_eq!(
            merged_outcome("fix".to_owned(), "main".to_owned(), false),
            BranchOutcome::Kept {
                branch: "fix".to_owned(),
                reason: "not merged into main".to_owned()
            }
        );
        assert_eq!(
            merged_outcome("fix".to_owned(), "origin/main".to_owned(), true),
            BranchOutcome::DeletedMerged {
                branch: "fix".to_owned(),
                into: "origin/main".to_owned()
            }
        );
    }
}
