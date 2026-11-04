//! Domain: merging a worktree's pull request on GitHub (#136): the method, what gh is asked
//! before merging, the refusals of `pr merge`, gh's arguments to merge it, and what `pr merge`
//! reports.
//!
//! As Orca does, the merge is `gh pr merge <number> --squash|--merge|--rebase`, squash by default,
//! and never `--delete-branch`: that flag fails while the worktree has the branch checked out, and
//! nothing happens to the worktree or its local branch after the merge.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use serde::Deserialize;

use crate::domain::error::{NotOpen, Review, TypedError};
use crate::domain::pr::{self, Found, GhRun, PrFacts, PrState, PullRequest};

/// How long gh may take to merge the pull request before it is stopped.
pub(crate) const MERGE_TIMEOUT: Duration = Duration::from_mins(1);

/// The fields `gh pr view` is asked for before a merge: those `pr refresh` reads, then whether
/// GitHub can merge it and what its review decided.
const MERGE_VIEW_FIELDS: &str =
    "number,title,state,isDraft,url,statusCheckRollup,mergeable,reviewDecision";

/// How the pull request's commits land on the base branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Method {
    /// One commit holding every change (`--squash`), the default.
    #[default]
    Squash,
    /// A merge commit (`--merge`).
    Merge,
    /// Each commit replayed on the base (`--rebase`).
    Rebase,
}

impl Method {
    /// How the text and `--json` name the method.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Squash => "squash",
            Self::Merge => "merge",
            Self::Rebase => "rebase",
        }
    }

    /// gh's flag for the method.
    fn flag(self) -> &'static str {
        match self {
            Self::Squash => "--squash",
            Self::Merge => "--merge",
            Self::Rebase => "--rebase",
        }
    }
}

/// gh's arguments to report the pull request of `branch` before a merge, run in the worktree.
pub(crate) fn view_args(branch: &str) -> Vec<String> {
    ["pr", "view", branch, "--json", MERGE_VIEW_FIELDS]
        .iter()
        .map(|&arg| arg.to_owned())
        .collect()
}

/// gh's arguments to merge the pull request `number` with `method`, run in the worktree. Never
/// `--delete-branch` (`cargo xtask gh-guard`).
pub(crate) fn merge_args(number: u64, method: Method) -> Vec<String> {
    vec![
        "pr".to_owned(),
        "merge".to_owned(),
        number.to_string(),
        method.flag().to_owned(),
    ]
}

/// The fields of `gh pr view --json` only a merge reads. gh prints `mergeable` as `MERGEABLE`,
/// `CONFLICTING` or `UNKNOWN` (while GitHub is still working it out), and `reviewDecision` as
/// `APPROVED`, `CHANGES_REQUESTED`, `REVIEW_REQUIRED`, or an empty string when the base branch
/// asks for no review.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MergeView {
    #[serde(default)]
    mergeable: Option<String>,
    #[serde(default)]
    review_decision: Option<String>,
}

/// The pull request gh reported before a merge (`run`, from [`view_args`]), when it can be merged;
/// else the refusal, merging nothing: gh unable to say (`prerequisite`), no open pull request
/// (`no_open_pr`), a draft (`pr_draft`), a conflict with the base (`pr_conflicting`), or a review
/// that requested changes or still needs an approval (`not_approved`). Anything else GitHub
/// refuses is left to `gh pr merge` to say.
pub(crate) fn check_mergeable(name: &str, run: &GhRun) -> Result<PullRequest> {
    let pr = match pr::found(run) {
        Found::Pr(pr) => pr,
        Found::NoPr => return Err(no_open_pr(name, NotOpen::None)),
        Found::Unavailable(reason) => {
            return Err(TypedError::MergeGhUnavailable {
                name: name.to_owned(),
                reason,
            }
            .into());
        }
    };
    let number = pr.number;
    match pr.state {
        PrState::Merged => return Err(no_open_pr(name, NotOpen::Merged(number))),
        PrState::Closed => return Err(no_open_pr(name, NotOpen::Closed(number))),
        PrState::Draft => {
            return Err(TypedError::PrDraft {
                name: name.to_owned(),
                number,
            }
            .into());
        }
        PrState::Open => {}
    }
    let view = merge_view(run);
    if view.mergeable.as_deref() == Some("CONFLICTING") {
        return Err(TypedError::PrConflicting {
            name: name.to_owned(),
            number,
        }
        .into());
    }
    let review = match view.review_decision.as_deref() {
        Some("CHANGES_REQUESTED") => Some(Review::ChangesRequested),
        Some("REVIEW_REQUIRED") => Some(Review::Required),
        _ => None,
    };
    if let Some(review) = review {
        return Err(TypedError::NotApproved {
            name: name.to_owned(),
            number,
            review,
        }
        .into());
    }
    Ok(pr)
}

/// The refusal of a worktree with no open pull request.
fn no_open_pr(name: &str, found: NotOpen) -> anyhow::Error {
    TypedError::NoOpenPr {
        name: name.to_owned(),
        found,
    }
    .into()
}

/// The merge fields of what a successful `gh pr view` printed; empty for anything else.
fn merge_view(run: &GhRun) -> MergeView {
    match run {
        GhRun::Exited {
            success: true,
            stdout,
            ..
        } => serde_json::from_slice(stdout).unwrap_or_default(),
        GhRun::Missing | GhRun::NotStarted(_) | GhRun::TimedOut | GhRun::Exited { .. } => {
            MergeView::default()
        }
    }
}

/// What `pr merge` keeps once gh merged `before` (the pull request as it was asked about before
/// the merge): `after`, gh's answer asked again, whenever it names the pull request, merged or
/// not (a merge queue, auto-merge or a required check can leave it open although `gh pr merge`
/// succeeded); else, when gh could not say, `before`, marked merged, since GitHub may simply not
/// show the merge yet.
pub(crate) fn merged_pr(before: &PullRequest, after: Found) -> PullRequest {
    match after {
        Found::Pr(pr) => pr,
        Found::NoPr | Found::Unavailable(_) => PullRequest {
            state: PrState::Merged,
            ..before.clone()
        },
    }
}

/// What `pr merge` reports: the worktree, its pull request as `pr refresh` keeps it (merged, or
/// still open when GitHub holds it), and the method used. The worktree and its local branch are as
/// they were.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrMerged {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) facts: PrFacts,
    pub(crate) method: Method,
}

impl PrMerged {
    /// The pull request merged.
    pub(crate) fn pr(&self) -> Option<&PullRequest> {
        match &self.facts.found {
            Found::Pr(pr) => Some(pr),
            Found::NoPr | Found::Unavailable(_) => None,
        }
    }

    /// Whether the pull request is recorded merged. `false` when gh accepted the merge but GitHub
    /// still shows the pull request open, as a merge queue or a required check may hold it.
    pub(crate) fn is_merged(&self) -> bool {
        self.pr().is_some_and(|pr| pr.state == PrState::Merged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error;
    use crate::domain::pr::Checks;

    /// A successful `gh pr view` printing `json`.
    fn viewed(json: &str) -> GhRun {
        GhRun::Exited {
            success: true,
            stdout: json.as_bytes().to_vec(),
            stderr: Vec::new(),
        }
    }

    /// gh's answer for pull request 12 in `state`, a draft or not, with `mergeable` and
    /// `reviewDecision`.
    fn view(state: &str, draft: bool, mergeable: &str, review: &str) -> GhRun {
        viewed(&format!(
            r#"{{"number":12,"title":"Fix","state":"{state}","isDraft":{draft},"url":"https://github.com/o/r/pull/12","statusCheckRollup":[],"mergeable":"{mergeable}","reviewDecision":"{review}"}}"#
        ))
    }

    /// The kind and message of the refusal `check_mergeable` gives for `run`.
    fn refusal(run: &GhRun) -> (&'static str, String) {
        let err = check_mergeable("demo", run).unwrap_err();
        (error::kind(&err), err.to_string())
    }

    #[test]
    fn gh_is_asked_for_the_merge_fields_then_merges_by_number_never_deleting_the_branch() {
        assert_eq!(
            view_args("demo"),
            [
                "pr",
                "view",
                "demo",
                "--json",
                "number,title,state,isDraft,url,statusCheckRollup,mergeable,reviewDecision"
            ]
        );
        assert_eq!(Method::default(), Method::Squash);
        assert_eq!(
            merge_args(12, Method::Squash),
            ["pr", "merge", "12", "--squash"]
        );
        assert_eq!(
            merge_args(12, Method::Merge),
            ["pr", "merge", "12", "--merge"]
        );
        assert_eq!(
            merge_args(7, Method::Rebase),
            ["pr", "merge", "7", "--rebase"]
        );
        assert_eq!(
            [Method::Squash, Method::Merge, Method::Rebase].map(Method::name),
            ["squash", "merge", "rebase"]
        );
    }

    #[test]
    fn an_open_mergeable_pull_request_with_no_blocking_review_can_be_merged() {
        for (mergeable, review) in [
            ("MERGEABLE", "APPROVED"),
            ("MERGEABLE", ""),
            ("UNKNOWN", "APPROVED"),
        ] {
            let pr = check_mergeable("demo", &view("OPEN", false, mergeable, review)).unwrap();
            assert_eq!(pr.number, 12);
            assert_eq!(pr.state, PrState::Open);
        }
        // gh may leave the merge fields out: nothing blocks then.
        let bare =
            r#"{"number":12,"title":"Fix","state":"OPEN","url":"https://github.com/o/r/pull/12"}"#;
        assert!(check_mergeable("demo", &viewed(bare)).is_ok());
    }

    #[test]
    fn no_open_pull_request_a_draft_a_conflict_or_a_blocking_review_is_refused() {
        let no_pr = GhRun::Exited {
            success: false,
            stdout: Vec::new(),
            stderr: b"no pull requests found for branch \"demo\"\n".to_vec(),
        };
        assert_eq!(refusal(&no_pr).0, "no_open_pr");
        assert_eq!(
            refusal(&view("MERGED", false, "UNKNOWN", "")).1,
            "PR #12 of worktree demo is merged already; there is nothing to merge"
        );
        assert_eq!(
            refusal(&view("CLOSED", false, "UNKNOWN", "")).0,
            "no_open_pr"
        );
        assert_eq!(refusal(&view("OPEN", true, "MERGEABLE", "")).0, "pr_draft");
        assert_eq!(
            refusal(&view("OPEN", false, "CONFLICTING", "APPROVED")).0,
            "pr_conflicting"
        );
        assert_eq!(
            refusal(&view("OPEN", false, "MERGEABLE", "CHANGES_REQUESTED")).0,
            "not_approved"
        );
        assert_eq!(
            refusal(&view("OPEN", false, "MERGEABLE", "REVIEW_REQUIRED")).1,
            "PR #12 of worktree demo needs an approving review first (nothing was merged)"
        );
    }

    #[test]
    fn gh_unable_to_say_is_a_prerequisite_with_its_reason() {
        let (kind, message) = refusal(&GhRun::Missing);
        assert_eq!(kind, "prerequisite");
        assert!(
            message.starts_with(
                "cannot merge the pull request of worktree demo: GitHub CLI is unavailable;"
            ),
            "{message}"
        );
    }

    /// Pull request 12, open, with passing checks: as gh shows it before the merge.
    fn open_pr() -> PullRequest {
        PullRequest {
            number: 12,
            title: "Fix".to_owned(),
            state: PrState::Open,
            url: "https://github.com/o/r/pull/12".to_owned(),
            checks: Checks::Passing,
        }
    }

    /// What `pr merge` reports with `pr` kept.
    fn report(pr: PullRequest) -> PrMerged {
        PrMerged {
            name: "demo".to_owned(),
            path: PathBuf::from("/l/demo"),
            facts: PrFacts {
                fetched_at: 1,
                found: Found::Pr(pr),
            },
            method: Method::Squash,
        }
    }

    #[test]
    fn a_merged_answer_is_kept_as_gh_gives_it() {
        let after = PullRequest {
            state: PrState::Merged,
            checks: Checks::None,
            ..open_pr()
        };
        let kept = merged_pr(&open_pr(), Found::Pr(after.clone()));
        assert_eq!(kept, after);
        assert!(report(kept).is_merged());
    }

    #[test]
    fn an_answer_still_open_or_a_draft_is_kept_open_and_not_called_merged() {
        let kept = merged_pr(&open_pr(), Found::Pr(open_pr()));
        assert_eq!(
            kept,
            open_pr(),
            "a merge queue or a required check holds it"
        );
        assert!(!report(kept).is_merged());
        let draft = PullRequest {
            state: PrState::Draft,
            ..open_pr()
        };
        assert_eq!(merged_pr(&open_pr(), Found::Pr(draft.clone())), draft);
    }

    #[test]
    fn gh_unable_to_say_keeps_the_one_asked_about_marked_merged() {
        let marked = PullRequest {
            state: PrState::Merged,
            ..open_pr()
        };
        assert_eq!(merged_pr(&open_pr(), Found::NoPr), marked);
        let slow = Found::Unavailable("slow".to_owned());
        let kept = merged_pr(&open_pr(), slow);
        assert_eq!(kept, marked);
        assert!(report(kept).is_merged());
    }
}
