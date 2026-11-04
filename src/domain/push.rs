//! Domain: pushing a worktree's branch to `origin` (#146), as Orca's **Push** and its separate,
//! explicit **Force push with lease** do.
//!
//! A plain push never forces. When git rejects it because the branch diverged from
//! `origin/<branch>` (a non-fast-forward, after a rebase for example), the push is the typed
//! `push_diverged`, and only a push asked for with a lease replaces the remote branch: the lease
//! names the commit git last saw there (`refs/remotes/origin/<branch>`), so git refuses when
//! someone else pushed since. There is never an automatic force.
//!
//! git's words are read with its messages in English (`LC_ALL=C`, set by the adapter), as
//! [`classify`] looks for them.

use std::path::PathBuf;

use crate::domain::branch::{Branch, StartPoint};
use crate::domain::pr::PrFacts;

/// What git said after a push, read by [`classify`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PushRun {
    /// The branch is on `origin` now, set as its upstream.
    Pushed,
    /// `origin` already had the branch as it is: nothing was sent.
    UpToDate,
    /// git refused a plain push because the branch diverged from `origin/<branch>` (a
    /// non-fast-forward), saying so in its words.
    Diverged(String),
    /// git refused or failed for any other reason, saying why in its words.
    Failed(String),
}

/// What git's line says when a push sent nothing because the remote branch was already there.
const UP_TO_DATE: &str = "Everything up-to-date";

/// What git's status line of a refused ref starts with.
const REJECTED: &str = "[rejected]";

/// Why git refused a ref whose remote holds commits the branch does not have.
const NON_FAST_FORWARD: &str = "(non-fast-forward)";

/// What a push did, from whether git `succeeded` and what it printed on stderr (`stderr`, where
/// git reports a push): up to date when it says so, diverged when it refused the branch as a
/// non-fast-forward, failed for any other refusal.
pub(crate) fn classify(succeeded: bool, stderr: &str) -> PushRun {
    let words = stderr.trim().to_owned();
    if succeeded {
        if stderr.lines().any(|line| line.trim() == UP_TO_DATE) {
            return PushRun::UpToDate;
        }
        return PushRun::Pushed;
    }
    let diverged = stderr.lines().any(|line| {
        let line = line.trim();
        line.contains(REJECTED) && line.ends_with(NON_FAST_FORWARD)
    });
    if diverged {
        PushRun::Diverged(words)
    } else {
        PushRun::Failed(words)
    }
}

/// The local copy git keeps of `branch` on `origin`: `refs/remotes/origin/<branch>`. The commit it
/// holds is what a push with lease expects to replace.
pub(crate) fn tracking_ref(branch: &Branch) -> String {
    format!("refs/remotes/origin/{branch}")
}

/// What a push that went through did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The branch was sent to `origin`.
    Pushed,
    /// `origin` already had it as it is.
    UpToDate,
}

impl Outcome {
    /// How `--json` names it: `pushed` or `up_to_date`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Pushed => "pushed",
            Self::UpToDate => "up_to_date",
        }
    }
}

/// What `worktree push` reports when the push went through: the worktree, its branch, what
/// happened, whether a lease was asked for, how many files with changes not committed were left
/// out, and its pull request as read again after the push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Pushed {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) branch: Branch,
    pub(crate) outcome: Outcome,
    pub(crate) with_lease: bool,
    pub(crate) uncommitted: usize,
    pub(crate) facts: PrFacts,
}

impl Pushed {
    /// Where the branch went, as shown: `origin/demo`.
    pub(crate) fn remote(&self) -> String {
        StartPoint::Remote(self.branch.clone()).to_string()
    }

    /// What the text output prints: `demo: pushed to origin/demo`, or `demo: origin/demo is
    /// already up to date`, with a second line when files with uncommitted changes were left out.
    pub(crate) fn text(&self) -> String {
        let remote = self.remote();
        let line = match self.outcome {
            Outcome::Pushed => format!("{}: pushed to {remote}", self.name),
            Outcome::UpToDate => format!("{}: {remote} is already up to date", self.name),
        };
        match self.uncommitted {
            0 => line,
            1 => format!("{line}\n1 file with uncommitted changes was left out of the push"),
            n => format!("{line}\n{n} files with uncommitted changes were left out of the push"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::pr::Found;

    #[test]
    fn a_push_git_accepted_is_pushed_or_up_to_date() {
        let sent = "To ../origin.git\n * [new branch]      demo -> demo\n";
        assert_eq!(classify(true, sent), PushRun::Pushed);
        assert_eq!(classify(true, "Everything up-to-date\n"), PushRun::UpToDate);
    }

    #[test]
    fn a_non_fast_forward_is_diverged_and_any_other_refusal_failed() {
        let diverged = "To ../origin.git\n ! [rejected]        demo -> demo (non-fast-forward)\n\
                        error: failed to push some refs to '../origin.git'\n";
        assert_eq!(
            classify(false, diverged),
            PushRun::Diverged(
                "To ../origin.git\n ! [rejected]        demo -> demo (non-fast-forward)\n\
                 error: failed to push some refs to '../origin.git'"
                    .to_owned()
            )
        );
        let stale = " ! [rejected]        demo -> demo (stale info)\nerror: failed to push\n";
        assert_eq!(
            classify(false, stale),
            PushRun::Failed(
                "! [rejected]        demo -> demo (stale info)\nerror: failed to push".to_owned()
            )
        );
        let behind = " ! [rejected]        demo -> demo (fetch first)\n";
        assert!(matches!(classify(false, behind), PushRun::Failed(_)));
        let offline = "fatal: '../gone.git' does not appear to be a git repository\n";
        assert!(matches!(classify(false, offline), PushRun::Failed(_)));
    }

    #[test]
    fn the_lease_reads_the_local_copy_of_the_remote_branch() {
        assert_eq!(
            tracking_ref(&Branch::new("demo")),
            "refs/remotes/origin/demo"
        );
    }

    #[test]
    fn the_text_says_where_the_branch_went_and_what_was_left_out() {
        let pushed = |outcome, uncommitted| Pushed {
            name: "demo".to_owned(),
            path: PathBuf::from("/w/demo"),
            branch: Branch::new("demo"),
            outcome,
            with_lease: false,
            uncommitted,
            facts: PrFacts {
                fetched_at: 0,
                found: Found::NoPr,
            },
        };
        assert_eq!(
            pushed(Outcome::Pushed, 0).text(),
            "demo: pushed to origin/demo"
        );
        assert_eq!(
            pushed(Outcome::UpToDate, 0).text(),
            "demo: origin/demo is already up to date"
        );
        assert_eq!(
            pushed(Outcome::Pushed, 1).text(),
            "demo: pushed to origin/demo\n1 file with uncommitted changes was left out of the push"
        );
        assert_eq!(
            pushed(Outcome::Pushed, 3).text(),
            "demo: pushed to origin/demo\n3 files with uncommitted changes were left out of the \
             push"
        );
        assert_eq!(Outcome::Pushed.name(), "pushed");
        assert_eq!(Outcome::UpToDate.name(), "up_to_date");
    }
}
