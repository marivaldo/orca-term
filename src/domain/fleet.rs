//! Domain: the fleet, every worktree of one repository, derived from what git lists.
//!
//! The fleet is `git worktree list` minus the primary checkout, which git always lists first. A
//! worktree is keyed by its path and named by that directory's basename.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::domain::branch::Branch;
use crate::domain::state::State;

/// A worktree as git reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeEntry {
    pub(crate) path: PathBuf,
    /// The checked-out branch. `None` when detached or bare.
    pub(crate) branch: Option<Branch>,
    /// Why git marks the worktree prunable, when it does: its directory or `.git` file is gone.
    pub(crate) prunable: Option<String>,
}

/// The repository's primary checkout: the working tree git lists first, never a worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrimaryCheckout {
    pub(crate) path: PathBuf,
    /// The checked-out branch. `None` when detached or bare.
    pub(crate) branch: Option<Branch>,
}

impl PrimaryCheckout {
    /// The primary checkout git listed as `entry`.
    pub(crate) fn from_entry(entry: WorktreeEntry) -> Self {
        Self {
            path: entry.path,
            branch: entry.branch,
        }
    }
}

/// One worktree of the fleet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Worktree {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) branch: Option<Branch>,
    pub(crate) state: State,
}

/// The primary checkout and every worktree of its repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fleet {
    pub(crate) primary: PrimaryCheckout,
    pub(crate) worktrees: Vec<Worktree>,
}

impl Fleet {
    /// The fleet of the worktrees git listed, primary checkout first. Every worktree starts with
    /// no agent; its state is read afterwards.
    pub(crate) fn from_entries(entries: Vec<WorktreeEntry>) -> Result<Self> {
        let mut entries = entries.into_iter();
        let Some(primary) = entries.next() else {
            bail!("git listed no worktrees");
        };
        let worktrees = entries
            .map(|wt| Worktree {
                name: worktree_name(&wt.path),
                path: wt.path,
                branch: wt.branch,
                state: State::NoAgent,
            })
            .collect();
        Ok(Self {
            primary: PrimaryCheckout::from_entry(primary),
            worktrees,
        })
    }

    /// The worktrees whose directory is gone: the ones `worktree prune` would clean.
    pub(crate) fn gone_worktrees(&self) -> impl Iterator<Item = &Worktree> {
        self.worktrees.iter().filter(|worktree| worktree.is_gone())
    }
}

impl Worktree {
    /// Whether the worktree is gone from disk, so that only `worktree prune` can clean it.
    pub(crate) fn is_gone(&self) -> bool {
        match &self.state {
            State::Broken(reason) => reason.is_gone(),
            State::NoAgent => false,
        }
    }
}

/// The paths of the worktrees gone in `before` and no longer listed in `after`: what a prune
/// cleaned.
pub(crate) fn pruned(before: &Fleet, after: &Fleet) -> Vec<PathBuf> {
    before
        .gone_worktrees()
        .filter(|gone| {
            !after
                .worktrees
                .iter()
                .any(|worktree| worktree.path == gone.path)
        })
        .map(|worktree| worktree.path.clone())
        .collect()
}

fn worktree_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.to_string_lossy(), OsStr::to_string_lossy)
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::state::BrokenReason;

    fn wt(path: &str, branch: Option<&str>) -> WorktreeEntry {
        WorktreeEntry {
            path: PathBuf::from(path),
            branch: branch.map(Branch::new),
            prunable: None,
        }
    }

    #[test]
    fn the_primary_checkout_is_never_a_worktree() {
        let fleet = Fleet::from_entries(vec![
            wt("/repo", Some("main")),
            wt("/worktrees/repo/fix", Some("fix")),
        ])
        .unwrap();
        assert_eq!(fleet.primary.path, PathBuf::from("/repo"));
        assert_eq!(fleet.primary.branch, Some(Branch::new("main")));
        assert_eq!(fleet.worktrees.len(), 1);
        assert_eq!(fleet.worktrees[0].name, "fix");
    }

    #[test]
    fn worktrees_sharing_a_basename_stay_distinct_by_path() {
        let fleet = Fleet::from_entries(vec![
            wt("/repo", Some("main")),
            wt("/a/api", Some("api")),
            wt("/b/api", Some("api-2")),
        ])
        .unwrap();
        assert_eq!(fleet.worktrees[0].name, fleet.worktrees[1].name);
        assert_ne!(fleet.worktrees[0].path, fleet.worktrees[1].path);
    }

    #[test]
    fn an_unreadable_state_is_broken_but_not_gone() {
        let mut fleet = Fleet::from_entries(vec![wt("/repo", None), wt("/a", None)]).unwrap();
        fleet.worktrees[0].state =
            State::Broken(BrokenReason::StateUnreadable("corrupt".to_owned()));
        assert!(!fleet.worktrees[0].is_gone());
        assert_eq!(fleet.gone_worktrees().count(), 0);
    }

    #[test]
    fn pruned_lists_the_gone_worktrees_that_left_the_fleet() {
        let mut before =
            Fleet::from_entries(vec![wt("/repo", None), wt("/a", None), wt("/b", None)]).unwrap();
        before.worktrees[0].state = State::Broken(BrokenReason::DirectoryMissing);
        before.worktrees[1].state = State::Broken(BrokenReason::DirectoryMissing);
        let after = Fleet::from_entries(vec![wt("/repo", None), wt("/b", None)]).unwrap();
        assert_eq!(pruned(&before, &after), vec![PathBuf::from("/a")]);
    }
}
