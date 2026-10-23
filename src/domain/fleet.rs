//! Domain: the fleet, every worktree of one repository, derived from what git lists.
//!
//! The fleet is `git worktree list` minus the primary checkout, which git always lists first. A
//! worktree is keyed by its path and named by that directory's basename.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::domain::agent::HolderStatus;
use crate::domain::branch::Branch;
use crate::domain::operation::Operation;
use crate::domain::pr::PrFacts;
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
    /// What the agent is doing or said last, in one line, beside a state from its hooks
    /// (working, waiting, finished, failed, interrupted); `None` otherwise.
    pub(crate) activity: Option<String>,
    /// The agent recorded for the worktree, by name (`claude-code`), or `None`.
    pub(crate) agent: Option<String>,
    /// Whether the worktree's holder is alive right now.
    pub(crate) holder: HolderStatus,
    /// Whether the worktree is unseen: its latest turn ended after the person last reviewed it,
    /// or the person marked it so (`domain/seen.rs`).
    pub(crate) unseen: bool,
    /// How many notes are pending on the worktree (`domain/note.rs`).
    pub(crate) notes: usize,
    /// How many tool calls the agent was denied in its latest turn, open or ended
    /// (`domain/turn.rs`). Never a state.
    pub(crate) denials: usize,
    /// The git operation the worktree is in the middle of (a merge, a rebase...), read from
    /// git's markers in its git dir (`domain/operation.rs`). Never a state.
    pub(crate) operation: Option<Operation>,
    /// What `pr refresh` last found about its pull request (`domain/pr.rs`), or `None` when it
    /// never ran for the worktree.
    pub(crate) pr: Option<PrFacts>,
    /// When its current session was last active, in Unix milliseconds: the latest of that
    /// session's turn starts and ends and the hooks' last report (`domain/session.rs`), or `None`
    /// when it has no current session or nothing says.
    pub(crate) last_activity: Option<u64>,
    /// How many files it changed against its base (#92): the paths that differ between the merge
    /// base with the start point and the files on disk, committed or not, plus the untracked
    /// files git does not ignore ([`count_changed_files`]). `None` when it cannot be counted, or
    /// was not: only `worktree ls` and `worktree detail` count it.
    pub(crate) changed_files: Option<usize>,
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
                activity: None,
                agent: None,
                holder: HolderStatus::Gone,
                unseen: false,
                notes: 0,
                denials: 0,
                operation: None,
                pr: None,
                last_activity: None,
                changed_files: None,
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
            State::NoAgent
            | State::Idle
            | State::Working
            | State::Waiting
            | State::Finished
            | State::Failed
            | State::Interrupted
            | State::SettingUp
            | State::NotReady(_) => false,
        }
    }

    /// Whether its changed files are worth counting: not while it is broken (git may not be able
    /// to read it) or its setup is still running (the setup is busy writing files).
    pub(crate) fn counts_changes(&self) -> bool {
        match &self.state {
            State::Broken(_) | State::SettingUp => false,
            State::NoAgent
            | State::Idle
            | State::Working
            | State::Waiting
            | State::Finished
            | State::Failed
            | State::Interrupted
            | State::NotReady(_) => true,
        }
    }
}

/// How many distinct files two NUL-separated lists of paths name together: `changed`, what
/// `git diff --name-only -z <base>` printed, and `untracked`, what
/// `git ls-files -z --others --exclude-standard` printed. A path in both counts once, as does a
/// path listed twice.
pub(crate) fn count_changed_files(changed: &[u8], untracked: &[u8]) -> usize {
    let mut paths: Vec<&[u8]> = changed
        .split(|byte| *byte == 0)
        .chain(untracked.split(|byte| *byte == 0))
        .filter(|path| !path.is_empty())
        .collect();
    paths.sort_unstable();
    paths.dedup();
    paths.len()
}

/// What `worktree prune` cleaned: the worktrees gone from disk that git forgot, and the chains of
/// snapshots (`domain/snapshot.rs`) left by worktrees git no longer has, by ref.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Pruned {
    pub(crate) worktrees: Vec<PathBuf>,
    pub(crate) chains: Vec<String>,
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
    fn changed_files_count_each_path_once() {
        assert_eq!(count_changed_files(b"", b""), 0, "no changes");
        assert_eq!(count_changed_files(b"a.txt\0b.txt\0", b""), 2);
        assert_eq!(count_changed_files(b"", b"new.txt\0"), 1);
        assert_eq!(
            count_changed_files(b"a.txt\0gone.txt\0", b"gone.txt\0new.txt\0"),
            3,
            "a file removed from the index but still on disk is in both lists"
        );
        assert_eq!(
            count_changed_files(b"dir/with space.txt\0", b""),
            1,
            "-z leaves paths unquoted"
        );
    }

    #[test]
    fn only_a_worktree_neither_broken_nor_setting_up_counts_its_changes() {
        let fleet = Fleet::from_entries(vec![wt("/repo", Some("main")), wt("/l/fix", Some("fix"))])
            .unwrap();
        let mut worktree = fleet.worktrees[0].clone();
        assert!(worktree.counts_changes());
        worktree.state = State::SettingUp;
        assert!(!worktree.counts_changes());
        worktree.state = State::Broken(BrokenReason::DirectoryMissing);
        assert!(!worktree.counts_changes());
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
