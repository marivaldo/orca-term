//! Domain: the fleet, every worktree of one repository, derived from what git lists.
//!
//! The fleet is `git worktree list` minus the primary checkout, which git always lists first. A
//! worktree is keyed by its path and named by that directory's basename.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::domain::state::State;

/// A worktree as git reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeEntry {
    pub(crate) path: PathBuf,
    /// The checked-out branch, without `refs/heads/`. `None` when detached or bare.
    pub(crate) branch: Option<String>,
    /// Why git marks the worktree prunable, when it does: its directory or `.git` file is gone.
    pub(crate) prunable: Option<String>,
}

/// One worktree of the fleet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Worktree {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) branch: Option<String>,
    pub(crate) state: State,
    /// Why the worktree is in its state, when there is something to say.
    pub(crate) detail: Option<String>,
    /// Whether the worktree's directory is gone, so only `worktree prune` can clean it.
    pub(crate) gone: bool,
}

/// The primary checkout and every worktree of its repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fleet {
    pub(crate) primary: WorktreeEntry,
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
                detail: None,
                gone: false,
            })
            .collect();
        Ok(Self { primary, worktrees })
    }

    /// The worktrees whose directory is gone: the ones `worktree prune` would clean.
    pub(crate) fn gone_worktrees(&self) -> impl Iterator<Item = &Worktree> {
        self.worktrees.iter().filter(|worktree| worktree.gone)
    }
}

impl Worktree {
    /// Marks the worktree broken because its directory is gone, for `detail`.
    pub(crate) fn mark_gone(&mut self, detail: String) {
        self.state = State::Broken;
        self.gone = true;
        self.detail = Some(detail);
    }

    /// Marks the worktree broken because its state cannot be read, for `detail`.
    pub(crate) fn mark_unreadable(&mut self, detail: String) {
        self.state = State::Broken;
        self.detail = Some(detail);
    }
}

/// Why a worktree counts as gone, or `None` when it is there: its directory is missing, or git
/// marks it prunable (`prunable` holds git's reason).
pub(crate) fn gone_detail(dir_exists: bool, prunable: Option<&str>) -> Option<String> {
    if !dir_exists {
        return Some("directory missing; run orca-term worktree prune".to_owned());
    }
    prunable.map(|reason| format!("git marks it prunable ({reason}); run orca-term worktree prune"))
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

    fn wt(path: &str, branch: Option<&str>) -> WorktreeEntry {
        WorktreeEntry {
            path: PathBuf::from(path),
            branch: branch.map(str::to_owned),
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
        assert_eq!(fleet.primary, wt("/repo", Some("main")));
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
    fn a_missing_directory_or_a_prunable_mark_is_gone() {
        assert_eq!(gone_detail(true, None), None);
        assert_eq!(
            gone_detail(false, Some("x")).as_deref(),
            Some("directory missing; run orca-term worktree prune")
        );
        assert_eq!(
            gone_detail(true, Some("gitdir file points to non-existent location")).as_deref(),
            Some(
                "git marks it prunable (gitdir file points to non-existent location); run \
                 orca-term worktree prune"
            )
        );
    }

    #[test]
    fn pruned_lists_the_gone_worktrees_that_left_the_fleet() {
        let mut before =
            Fleet::from_entries(vec![wt("/repo", None), wt("/a", None), wt("/b", None)]).unwrap();
        before.worktrees[0].mark_gone("gone".to_owned());
        before.worktrees[1].mark_gone("gone".to_owned());
        let after = Fleet::from_entries(vec![wt("/repo", None), wt("/b", None)]).unwrap();
        assert_eq!(pruned(&before, &after), vec![PathBuf::from("/a")]);
    }
}
