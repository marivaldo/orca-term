//! Domain: the git operation a worktree is in the middle of (a merge, a rebase, a cherry-pick or a
//! revert), told from the markers git leaves in the worktree's own git dir while it waits for the
//! person, or an agent, to resolve the conflicts (#86). It is never a state: the worktree's state
//! stays what its agent made it.

/// git's directory of a rebase run by its merge backend, in the worktree's git dir.
pub(crate) const REBASE_MERGE_DIR: &str = "rebase-merge";
/// git's directory of a rebase run by its apply backend, or of `git am`, in the worktree's git dir.
pub(crate) const REBASE_APPLY_DIR: &str = "rebase-apply";
/// The file, in the worktree's git dir, that says `git am`, not a rebase, made `rebase-apply/`.
const APPLYING_FILE: &str = "rebase-apply/applying";
/// git's file naming the commit being merged.
const MERGE_HEAD_FILE: &str = "MERGE_HEAD";
/// git's file naming the commit being cherry-picked.
const CHERRY_PICK_HEAD_FILE: &str = "CHERRY_PICK_HEAD";
/// git's file naming the commit being reverted.
const REVERT_HEAD_FILE: &str = "REVERT_HEAD";
/// The files of `rebase-merge/` holding the step a rebase is at, and how many it has.
pub(crate) const REBASE_MERGE_STEP_FILES: (&str, &str) = ("msgnum", "end");
/// The files of `rebase-apply/` holding the step a rebase is at, and how many it has.
pub(crate) const REBASE_APPLY_STEP_FILES: (&str, &str) = ("next", "last");

/// A git operation in progress in a worktree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Operation {
    Merge,
    Rebase,
    CherryPick,
    Revert,
}

impl Operation {
    /// How the contract and the client name it: `merge`, `rebase`, `cherry-pick`, `revert`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Merge => "merge",
            Self::Rebase => "rebase",
            Self::CherryPick => "cherry-pick",
            Self::Revert => "revert",
        }
    }

    /// How `worktree ls` says it beside the state: `merging`, `rebasing`, `cherry-picking`,
    /// `reverting`.
    pub(crate) fn verb(self) -> &'static str {
        match self {
            Self::Merge => "merging",
            Self::Rebase => "rebasing",
            Self::CherryPick => "cherry-picking",
            Self::Revert => "reverting",
        }
    }
}

/// The operation in progress in a worktree whose git dir holds the markers for which `present`
/// (given a path relative to the git dir, such as `MERGE_HEAD`) says true, or `None`. A rebase
/// wins over the others, since a rebase replaying a merge or picking a commit may leave their
/// markers too. `git am` (`rebase-apply/` holding `applying`) is none of them.
pub(crate) fn detect(present: impl Fn(&str) -> bool) -> Option<Operation> {
    let rebase_apply = present(REBASE_APPLY_DIR) && !present(APPLYING_FILE);
    if present(REBASE_MERGE_DIR) || rebase_apply {
        Some(Operation::Rebase)
    } else if present(MERGE_HEAD_FILE) {
        Some(Operation::Merge)
    } else if present(CHERRY_PICK_HEAD_FILE) {
        Some(Operation::CherryPick)
    } else if present(REVERT_HEAD_FILE) {
        Some(Operation::Revert)
    } else {
        None
    }
}

/// The step a rebase is at: `current` of `total` commits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct RebaseStep {
    pub(crate) current: u64,
    pub(crate) total: u64,
}

impl RebaseStep {
    /// The step git recorded as the texts `current` and `total` of its two step files. `None`
    /// when either is not a number, or the step is past the total.
    pub(crate) fn from_texts(current: &str, total: &str) -> Option<Self> {
        let current = current.trim().parse::<u64>().ok()?;
        let total = total.trim().parse::<u64>().ok()?;
        (current <= total).then_some(Self { current, total })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The operation of a git dir holding exactly `markers`.
    fn detect_among(markers: &[&str]) -> Option<Operation> {
        detect(|marker| markers.contains(&marker))
    }

    #[test]
    fn each_marker_names_its_operation_and_none_names_none() {
        assert_eq!(detect_among(&[]), None);
        assert_eq!(detect_among(&["ORIG_HEAD", "index"]), None);
        assert_eq!(detect_among(&["rebase-merge"]), Some(Operation::Rebase));
        assert_eq!(detect_among(&["rebase-apply"]), Some(Operation::Rebase));
        assert_eq!(detect_among(&["MERGE_HEAD"]), Some(Operation::Merge));
        assert_eq!(
            detect_among(&["CHERRY_PICK_HEAD"]),
            Some(Operation::CherryPick)
        );
        assert_eq!(detect_among(&["REVERT_HEAD"]), Some(Operation::Revert));
    }

    #[test]
    fn a_rebase_wins_over_the_markers_it_may_leave() {
        assert_eq!(
            detect_among(&["rebase-merge", "MERGE_HEAD", "CHERRY_PICK_HEAD"]),
            Some(Operation::Rebase)
        );
    }

    #[test]
    fn git_am_is_not_a_rebase() {
        assert_eq!(
            detect_among(&["rebase-apply", "rebase-apply/applying"]),
            None
        );
    }

    #[test]
    fn the_names_are_the_contracts() {
        let names = [
            Operation::Merge,
            Operation::Rebase,
            Operation::CherryPick,
            Operation::Revert,
        ]
        .map(Operation::name);
        assert_eq!(names, ["merge", "rebase", "cherry-pick", "revert"]);
        assert_eq!(Operation::CherryPick.verb(), "cherry-picking");
    }

    #[test]
    fn a_rebase_step_is_two_numbers_the_first_not_past_the_second() {
        assert_eq!(
            RebaseStep::from_texts("2\n", "5\n"),
            Some(RebaseStep {
                current: 2,
                total: 5
            })
        );
        for (current, total) in [("", "5"), ("x", "5"), ("2", ""), ("6", "5"), ("-1", "5")] {
            assert_eq!(
                RebaseStep::from_texts(current, total),
                None,
                "{current:?}/{total:?}"
            );
        }
    }
}
