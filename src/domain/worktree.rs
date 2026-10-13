//! Domain: the rules for one worktree. What its name may be, where it goes, which worktree a
//! `worktree rm` target means, and what happens to its branch when it is removed.

use std::convert::Infallible;
use std::ffi::OsStr;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Result, bail};

use crate::domain::branch::{self, Branch, BranchNameError, StartPoint};
use crate::domain::config::Setting;
use crate::domain::fleet::{Fleet, PrimaryCheckout, Worktree};
use crate::domain::state::State;

/// A valid name for a new worktree: one path segment, not read as an option, and a valid branch
/// name, since the worktree's branch takes the same name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeName(String);

/// Why a worktree name is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum WorktreeNameError {
    #[error("a worktree name cannot be empty")]
    Empty,
    #[error("invalid worktree name `{0}`: it cannot contain `/`")]
    ContainsSlash(String),
    #[error("invalid worktree name `{0}`: it cannot start with `-`")]
    StartsWithDash(String),
    #[error("invalid worktree name `{name}`: it is not a valid branch name ({rule})")]
    NotABranchName { name: String, rule: BranchNameError },
}

impl FromStr for WorktreeName {
    type Err = WorktreeNameError;

    fn from_str(name: &str) -> Result<Self, Self::Err> {
        if name.is_empty() {
            return Err(WorktreeNameError::Empty);
        }
        if name.contains('/') {
            return Err(WorktreeNameError::ContainsSlash(name.to_owned()));
        }
        if name.starts_with('-') {
            return Err(WorktreeNameError::StartsWithDash(name.to_owned()));
        }
        if let Err(rule) = branch::check_name(name) {
            return Err(WorktreeNameError::NotABranchName {
                name: name.to_owned(),
                rule,
            });
        }
        Ok(Self(name.to_owned()))
    }
}

impl WorktreeName {
    /// The name as written.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// The branch a new worktree gets: one with the worktree's own name.
    pub(crate) fn branch(&self) -> Branch {
        Branch::new(&self.0)
    }
}

impl fmt::Display for WorktreeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What `worktree rm` is given: a worktree's name, or a path to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// A worktree's name: its directory's basename.
    Name(String),
    /// A path to the worktree, relative to the current directory or absolute.
    Path(PathBuf),
}

/// A target is a path when it contains `/` or is `.` or `..`, and a name otherwise.
impl FromStr for Target {
    type Err = Infallible;

    fn from_str(target: &str) -> Result<Self, Self::Err> {
        if target.contains('/') || target == "." || target == ".." {
            Ok(Self::Path(PathBuf::from(target)))
        } else {
            Ok(Self::Name(target.to_owned()))
        }
    }
}

/// How `worktree rm` removes a worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Removal {
    /// Refuses a worktree with changes, and deletes its branch only when merged.
    Safe,
    /// `--force`: removes a worktree with changes and deletes an unmerged branch.
    Forced,
}

/// What `worktree new` made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Created {
    pub(crate) name: WorktreeName,
    pub(crate) path: PathBuf,
    pub(crate) branch: Branch,
    /// Where the branch starts: the default branch, local or on `origin`.
    pub(crate) start: StartPoint,
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
    DeletedMerged { branch: Branch, into: StartPoint },
    /// Deleted because of `--force`.
    DeletedForced { branch: Branch },
    /// Kept, for `reason`.
    Kept { branch: Branch, reason: KeptReason },
    /// The worktree was detached: there was no branch.
    Detached,
}

/// Why `worktree rm` kept the worktree's branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum KeptReason {
    /// It is the repository's default branch, which is never deleted.
    DefaultBranch,
    /// It is not merged into the default branch, and there was no `--force`.
    NotMerged { into: StartPoint },
    /// Git could not find the default branch to compare it with, for the reason given.
    NoDefaultBranch(String),
    /// Git refused to delete it, for the reason given.
    DeleteFailed(String),
}

/// The reason as `worktree rm` prints it, after `kept branch <branch>: `.
impl fmt::Display for KeptReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DefaultBranch => write!(f, "it is the default branch"),
            Self::NotMerged { into } => write!(f, "not merged into {into}"),
            Self::NoDefaultBranch(reason) => write!(f, "{reason}"),
            Self::DeleteFailed(reason) => write!(f, "could not delete it: {reason}"),
        }
    }
}

/// What `worktree rm` decides to do with the worktree's branch, before the worktree goes. A
/// decided deletion is carried out afterwards, and becomes `Kept` if git refuses it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BranchFate {
    /// Decided without asking git.
    Decided(BranchOutcome),
    /// Delete `branch` only when it is merged into the default branch, which git has to answer.
    IfMerged { branch: Branch },
}

/// Where the worktree `name` of the repository whose primary checkout is `primary` goes:
/// `<base>/<repo>/<name>`, where `<repo>` is the primary checkout's directory name.
pub(crate) fn path_for(
    base: &Path,
    primary: &PrimaryCheckout,
    name: &WorktreeName,
) -> Result<PathBuf> {
    let Some(repo) = primary.path.file_name() else {
        bail!(
            "the primary checkout {} has no directory name",
            primary.path.display()
        );
    };
    Ok(base.join(repo).join(name.as_str()))
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
    if let State::Broken(reason) = &worktree.state
        && reason.is_gone()
    {
        bail!(
            "worktree {} at {} is broken ({reason}); only `orca-term worktree prune` cleans a broken worktree",
            worktree.name,
            worktree.path.display(),
        );
    }
    Ok(())
}

/// What to do with `branch`, the removed worktree's branch, given the repository's `default`
/// branch when known. The default branch is never deleted, not even with `--force`: it is
/// trivially merged into itself.
pub(crate) fn branch_fate(
    branch: Option<&Branch>,
    default: Option<&Branch>,
    removal: Removal,
) -> BranchFate {
    let Some(branch) = branch else {
        return BranchFate::Decided(BranchOutcome::Detached);
    };
    let branch = branch.clone();
    if default == Some(&branch) {
        return BranchFate::Decided(BranchOutcome::Kept {
            branch,
            reason: KeptReason::DefaultBranch,
        });
    }
    match removal {
        Removal::Forced => BranchFate::Decided(BranchOutcome::DeletedForced { branch }),
        Removal::Safe => BranchFate::IfMerged { branch },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn branch(name: &str) -> Branch {
        Branch::new(name)
    }

    #[test]
    fn a_worktree_name_is_one_segment_that_is_not_an_option() {
        assert_eq!(
            "fix-login".parse::<WorktreeName>().unwrap().as_str(),
            "fix-login"
        );
        assert_eq!("".parse::<WorktreeName>(), Err(WorktreeNameError::Empty));
        assert_eq!(
            "a/b".parse::<WorktreeName>(),
            Err(WorktreeNameError::ContainsSlash("a/b".to_owned()))
        );
        assert_eq!(
            "-x".parse::<WorktreeName>(),
            Err(WorktreeNameError::StartsWithDash("-x".to_owned()))
        );
    }

    #[test]
    fn a_worktree_name_must_be_a_valid_branch_name() {
        assert_eq!(
            "bad..name".parse::<WorktreeName>(),
            Err(WorktreeNameError::NotABranchName {
                name: "bad..name".to_owned(),
                rule: BranchNameError::DoubleDot,
            })
        );
        assert!("has space".parse::<WorktreeName>().is_err());
        assert!("x.lock".parse::<WorktreeName>().is_err());
        assert!("HEAD".parse::<WorktreeName>().is_err());
    }

    #[test]
    fn a_refused_worktree_name_says_which_rule_it_breaks() {
        let message = |name: &str| name.parse::<WorktreeName>().unwrap_err().to_string();
        assert_eq!(message(""), "a worktree name cannot be empty");
        assert_eq!(
            message("a/b"),
            "invalid worktree name `a/b`: it cannot contain `/`"
        );
        assert_eq!(
            message("-x"),
            "invalid worktree name `-x`: it cannot start with `-`"
        );
        assert_eq!(
            message("bad..name"),
            "invalid worktree name `bad..name`: it is not a valid branch name (it contains `..`)"
        );
    }

    #[test]
    fn a_new_worktree_gets_a_branch_of_its_own_name() {
        let name: WorktreeName = "fix".parse().unwrap();
        assert_eq!(name.branch(), branch("fix"));
    }

    #[test]
    fn a_target_with_a_slash_or_dots_is_a_path() {
        let target = |text: &str| text.parse::<Target>().unwrap();
        assert_eq!(target("fix"), Target::Name("fix".to_owned()));
        assert_eq!(target(".hidden"), Target::Name(".hidden".to_owned()));
        assert_eq!(target("a/fix"), Target::Path(PathBuf::from("a/fix")));
        assert_eq!(target("/l/fix"), Target::Path(PathBuf::from("/l/fix")));
        assert_eq!(target("."), Target::Path(PathBuf::from(".")));
        assert_eq!(target(".."), Target::Path(PathBuf::from("..")));
    }

    #[test]
    fn the_default_branch_is_kept_even_with_force() {
        assert_eq!(
            branch_fate(
                Some(&branch("main")),
                Some(&branch("main")),
                Removal::Forced
            ),
            BranchFate::Decided(BranchOutcome::Kept {
                branch: branch("main"),
                reason: KeptReason::DefaultBranch,
            })
        );
        assert_eq!(
            branch_fate(Some(&branch("fix")), Some(&branch("main")), Removal::Forced),
            BranchFate::Decided(BranchOutcome::DeletedForced {
                branch: branch("fix")
            })
        );
        assert_eq!(
            branch_fate(Some(&branch("fix")), None, Removal::Safe),
            BranchFate::IfMerged {
                branch: branch("fix")
            }
        );
        assert_eq!(
            branch_fate(None, Some(&branch("main")), Removal::Forced),
            BranchFate::Decided(BranchOutcome::Detached)
        );
    }

    #[test]
    fn a_kept_branch_says_why() {
        assert_eq!(
            KeptReason::DefaultBranch.to_string(),
            "it is the default branch"
        );
        assert_eq!(
            KeptReason::NotMerged {
                into: StartPoint::Remote(branch("main"))
            }
            .to_string(),
            "not merged into origin/main"
        );
        assert_eq!(
            KeptReason::DeleteFailed("git branch -D failed".to_owned()).to_string(),
            "could not delete it: git branch -D failed"
        );
    }
}
