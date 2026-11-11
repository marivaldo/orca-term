//! Domain: the fleet, every worktree of one repository, derived from what git lists.
//!
//! The fleet is `git worktree list`, the primary checkout included (ADR 0013): git always lists it
//! first, and it stays first, tagged `primary`. A bare repository has no checkout there, so its
//! first entry is left out. A worktree is keyed by its path and named by that directory's
//! basename.

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
    /// Whether git marks it bare: a bare repository, which has no files checked out.
    pub(crate) bare: bool,
}

/// The repository's primary checkout: the working tree git lists first. It is also the first
/// worktree of the fleet (ADR 0013); this keeps where it is, for the commands that work on the
/// repository as a whole.
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
    /// Whether it is the primary checkout (ADR 0013): first in its fleet, never removed.
    pub(crate) primary: bool,
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
    /// The default branch when it is on `origin`, which a worktree is rebased onto (#141); `None`
    /// when the repository has none, or it was not looked for (only `worktree ls` looks).
    pub(crate) remote_base: Option<Branch>,
    /// Whether the repository has a remote named `origin`, which a worktree is pushed to (#146);
    /// `false` when it has none, or it was not looked for (only `worktree ls` looks).
    pub(crate) has_origin: bool,
}

impl Fleet {
    /// The fleet of the worktrees git listed, primary checkout first and tagged `primary`, unless
    /// the repository is bare. Every worktree starts with no agent; its state is read afterwards.
    pub(crate) fn from_entries(entries: Vec<WorktreeEntry>) -> Result<Self> {
        let Some(first) = entries.first() else {
            bail!("git listed no worktrees");
        };
        let primary = PrimaryCheckout {
            path: first.path.clone(),
            branch: first.branch.clone(),
        };
        let worktrees = entries
            .into_iter()
            .enumerate()
            .filter(|(_, entry)| !entry.bare)
            .map(|(index, entry)| Worktree::listed(entry, index == 0))
            .collect();
        Ok(Self {
            primary,
            worktrees,
            remote_base: None,
            has_origin: false,
        })
    }

    /// The worktrees whose directory is gone: the ones `worktree prune` would clean.
    pub(crate) fn gone_worktrees(&self) -> impl Iterator<Item = &Worktree> {
        self.worktrees.iter().filter(|worktree| worktree.is_gone())
    }
}

impl Worktree {
    /// The worktree git listed as `entry`, the primary checkout when `primary`, with no agent yet.
    fn listed(entry: WorktreeEntry, primary: bool) -> Self {
        Self {
            name: worktree_name(&entry.path),
            path: entry.path,
            branch: entry.branch,
            primary,
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
        }
    }

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
            bare: false,
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
    fn the_primary_checkout_is_the_first_worktree_of_its_fleet() {
        let fleet = Fleet::from_entries(vec![
            wt("/repo", Some("main")),
            wt("/worktrees/repo/fix", Some("fix")),
        ])
        .unwrap();
        assert_eq!(fleet.primary.path, PathBuf::from("/repo"));
        assert_eq!(fleet.primary.branch, Some(Branch::new("main")));
        assert_eq!(fleet.worktrees.len(), 2);
        assert_eq!(fleet.worktrees[0].name, "repo");
        assert_eq!(fleet.worktrees[0].path, PathBuf::from("/repo"));
        assert_eq!(fleet.worktrees[0].branch, Some(Branch::new("main")));
        assert!(fleet.worktrees[0].primary, "it carries the primary tag");
        assert_eq!(fleet.worktrees[1].name, "fix");
        assert!(!fleet.worktrees[1].primary);
    }

    #[test]
    fn a_bare_repository_has_no_checkout_to_list() {
        let mut bare = wt("/repo.git", None);
        bare.bare = true;
        let fleet = Fleet::from_entries(vec![bare, wt("/w/fix", Some("fix"))]).unwrap();
        assert_eq!(fleet.primary.path, PathBuf::from("/repo.git"));
        assert_eq!(fleet.worktrees.len(), 1);
        assert_eq!(fleet.worktrees[0].name, "fix");
        assert!(!fleet.worktrees[0].primary);
    }

    #[test]
    fn worktrees_sharing_a_basename_stay_distinct_by_path() {
        let fleet = Fleet::from_entries(vec![
            wt("/repo", Some("main")),
            wt("/a/api", Some("api")),
            wt("/b/api", Some("api-2")),
        ])
        .unwrap();
        assert_eq!(fleet.worktrees[1].name, fleet.worktrees[2].name);
        assert_ne!(fleet.worktrees[1].path, fleet.worktrees[2].path);
    }

    #[test]
    fn an_unreadable_state_is_broken_but_not_gone() {
        let mut fleet = Fleet::from_entries(vec![wt("/repo", None), wt("/a", None)]).unwrap();
        fleet.worktrees[1].state =
            State::Broken(BrokenReason::StateUnreadable("corrupt".to_owned()));
        assert!(!fleet.worktrees[1].is_gone());
        assert_eq!(fleet.gone_worktrees().count(), 0);
    }

    #[test]
    fn pruned_lists_the_gone_worktrees_that_left_the_fleet() {
        let mut before =
            Fleet::from_entries(vec![wt("/repo", None), wt("/a", None), wt("/b", None)]).unwrap();
        before.worktrees[1].state = State::Broken(BrokenReason::DirectoryMissing);
        before.worktrees[2].state = State::Broken(BrokenReason::DirectoryMissing);
        let after = Fleet::from_entries(vec![wt("/repo", None), wt("/b", None)]).unwrap();
        assert_eq!(pruned(&before, &after), vec![PathBuf::from("/a")]);
    }
}
