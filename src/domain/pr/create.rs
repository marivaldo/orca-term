//! Domain: a worktree's new pull request (#132): the message it starts from (`pr compose`), the
//! refusals of `pr create`, gh's arguments to create it, and what `pr create` reports.
//!
//! The message follows `gh pr create --fill`: the title is the subject of the branch's only commit
//! ahead of the base, or the branch's name when there are none or several; the body is the
//! repository's pull request template when it has one, else the only commit's body, else the
//! list of the commits' subjects. The core pushes the branch and runs gh itself, outside any
//! agent's sandbox, as Orca's "Push & Create PR" does.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Result, bail};

use crate::domain::branch::Branch;
use crate::domain::error::TypedError;
use crate::domain::pr::{Checks, Found, PrFacts, PrState, PullRequest};

/// How long gh may take to create the pull request before it is stopped.
pub(crate) const CREATE_TIMEOUT: Duration = Duration::from_mins(1);

/// The directories a pull request template is looked for in, relative to the worktree, in order:
/// `.github/`, the worktree's root, `docs/`.
pub(crate) const TEMPLATE_DIRS: &[&str] = &[".github", "", "docs"];

/// A template's file name, lowercased, without its extension.
const TEMPLATE_STEM: &str = "pull_request_template";

/// One commit ahead of the base: its subject (the first line of its message) and its body (the
/// rest, without the blank line between).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitMessage {
    pub(crate) subject: String,
    pub(crate) body: String,
}

/// The commits in what `git log -z --format=%s%x1f%b` printed: one record per commit, ended by a
/// NUL, its subject and body split by the unit separator (0x1f). In the order git printed them.
pub(crate) fn parse_log(bytes: &[u8]) -> Vec<CommitMessage> {
    String::from_utf8_lossy(bytes)
        .split('\0')
        .filter(|record| !record.trim().is_empty())
        .map(|record| {
            let (subject, body) = record.split_once('\x1f').unwrap_or((record, ""));
            CommitMessage {
                subject: subject.trim().to_owned(),
                body: body.trim().to_owned(),
            }
        })
        .collect()
}

/// Whether `name`, a file's name, is a pull request template: `pull_request_template`, in any
/// case, with or without an extension (`PULL_REQUEST_TEMPLATE.md`, `pull_request_template.md`),
/// as gh finds one.
pub(crate) fn is_template_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower == TEMPLATE_STEM
        || lower
            .strip_prefix(TEMPLATE_STEM)
            .is_some_and(|rest| rest.starts_with('.'))
}

/// The message a new pull request starts from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Message {
    pub(crate) title: String,
    pub(crate) body: String,
}

/// The message for `branch`, whose commits ahead of the base are `commits` (oldest first), given
/// the text of the repository's pull request template, if it has one.
pub(crate) fn compose(
    branch: &Branch,
    commits: &[CommitMessage],
    template: Option<&str>,
) -> Message {
    let title = match commits {
        [only] => only.subject.clone(),
        _ => branch.as_str().to_owned(),
    };
    let body = match (template, commits) {
        (Some(template), _) => template.trim_end().to_owned(),
        (None, [only]) => only.body.clone(),
        (None, several) => several
            .iter()
            .map(|commit| format!("- {}", commit.subject))
            .collect::<Vec<_>>()
            .join("\n"),
    };
    Message { title, body }
}

/// How many files have changes not committed, in what `git status --porcelain -z
/// --untracked-files=all` printed: one entry per file, untracked files included. A rename or a
/// copy carries its old path as one more field, which is not another file.
pub(crate) fn count_uncommitted(porcelain: &[u8]) -> usize {
    let mut count = 0;
    let mut fields = porcelain.split(|&byte| byte == 0).filter(|f| !f.is_empty());
    while let Some(entry) = fields.next() {
        count += 1;
        let status = entry.get(..2).unwrap_or_default();
        if status.contains(&b'R') || status.contains(&b'C') {
            fields.next();
        }
    }
    count
}

/// What a worktree's new pull request starts from, as `pr compose` reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Composed {
    pub(crate) message: Message,
    /// The branch the pull request goes into: the default branch, unless given.
    pub(crate) base: Branch,
    /// The worktree's branch.
    pub(crate) branch: Branch,
    /// How many commits the branch has ahead of the base.
    pub(crate) commits: usize,
    /// How many files have changes not committed, which a pull request leaves out.
    pub(crate) uncommitted: usize,
}

/// Whether the new pull request is ready for review or a draft (`--draft`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Readiness {
    Ready,
    Draft,
}

/// What the pull request's body is, for gh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Body {
    /// The text of this file, an absolute path.
    File(PathBuf),
    /// No body: no file was given, or it is missing or empty.
    Empty,
}

/// gh's arguments to create the pull request of `branch` into `base`, run in the worktree once
/// the branch is pushed, so gh never offers to push it itself.
pub(crate) fn create_args(
    base: &Branch,
    branch: &Branch,
    title: &str,
    body: &Body,
    readiness: Readiness,
) -> Vec<String> {
    let mut args: Vec<String> = [
        "pr",
        "create",
        "--base",
        base.as_str(),
        "--head",
        branch.as_str(),
        "--title",
        title,
    ]
    .iter()
    .map(|&arg| arg.to_owned())
    .collect();
    match body {
        Body::File(path) => {
            args.push("--body-file".to_owned());
            args.push(path.to_string_lossy().into_owned());
        }
        Body::Empty => {
            args.push("--body".to_owned());
            args.push(String::new());
        }
    }
    if readiness == Readiness::Draft {
        args.push("--draft".to_owned());
    }
    args
}

/// Refuses an empty title, before anything runs: GitHub needs one.
pub(crate) fn check_title(title: &str) -> Result<()> {
    if title.trim().is_empty() {
        bail!("the pull request's title is empty; pass one with --title");
    }
    Ok(())
}

/// Refuses a branch with no commits ahead of the base: a pull request would hold nothing.
pub(crate) fn check_has_commits(name: &str, composed: &Composed) -> Result<()> {
    if composed.commits == 0 {
        return Err(TypedError::NoCommits {
            name: name.to_owned(),
            branch: composed.branch.to_string(),
            base: composed.base.to_string(),
        }
        .into());
    }
    Ok(())
}

/// Refuses when gh, asked about the branch's pull request, found an open one (or a draft), or
/// could not say: gh missing, not logged in, not pointed at GitHub. A merged or closed pull
/// request does not stop a new one.
pub(crate) fn check_no_open_pr(name: &str, found: &Found) -> Result<()> {
    match found {
        Found::Pr(pr) if matches!(pr.state, PrState::Open | PrState::Draft) => {
            Err(TypedError::PrExists {
                name: name.to_owned(),
                number: pr.number,
                url: pr.url.clone(),
            }
            .into())
        }
        Found::Unavailable(reason) => Err(TypedError::GhUnavailable {
            name: name.to_owned(),
            reason: reason.clone(),
        }
        .into()),
        Found::Pr(_) | Found::NoPr => Ok(()),
    }
}

/// The pull request gh just created, from what it printed (its address, on the last line), the
/// title given and whether it is a draft: what `pr create` records when gh cannot read it back
/// yet. Its checks have not started. `None` when gh printed no pull request's address.
pub(crate) fn created_pr(stdout: &[u8], title: &str, readiness: Readiness) -> Option<PullRequest> {
    let text = String::from_utf8_lossy(stdout);
    let url = text.lines().map(str::trim).rfind(|line| !line.is_empty())?;
    let (_, number) = url.rsplit_once("/pull/")?;
    let number = number.trim_end_matches('/').parse().ok()?;
    Some(PullRequest {
        number,
        title: title.to_owned(),
        state: match readiness {
            Readiness::Ready => PrState::Open,
            Readiness::Draft => PrState::Draft,
        },
        url: url.to_owned(),
        checks: Checks::None,
    })
}

/// The first line of what a failed gh printed on stderr, or `fallback` when it printed nothing.
pub(crate) fn first_line(stderr: &[u8], fallback: &str) -> String {
    let text = String::from_utf8_lossy(stderr);
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}

/// What `pr create` reports: the worktree, its new pull request as `pr refresh` keeps it, and how
/// many files with changes not committed were left out. The branch was pushed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrCreated {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) facts: PrFacts,
    pub(crate) uncommitted: usize,
}

impl PrCreated {
    /// The pull request created.
    pub(crate) fn pr(&self) -> Option<&PullRequest> {
        match &self.facts.found {
            Found::Pr(pr) => Some(pr),
            Found::NoPr | Found::Unavailable(_) => None,
        }
    }
}

/// `path`, made absolute against `cwd` when it is relative.
pub(crate) fn absolute(cwd: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        cwd.join(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error;

    fn commit(subject: &str, body: &str) -> CommitMessage {
        CommitMessage {
            subject: subject.to_owned(),
            body: body.to_owned(),
        }
    }

    #[test]
    fn the_log_is_read_one_record_per_commit() {
        let log = b"Fix the login\x1fThe token expired.\n\nCloses #3\n\0\nAdd a test\x1f\n\0";
        assert_eq!(
            parse_log(log),
            [
                commit("Fix the login", "The token expired.\n\nCloses #3"),
                commit("Add a test", "")
            ]
        );
        assert_eq!(parse_log(b""), []);
    }

    #[test]
    fn one_commit_gives_its_subject_and_body() {
        let message = compose(
            &Branch::new("fix-login"),
            &[commit("Fix the login", "The token expired.")],
            None,
        );
        assert_eq!(message.title, "Fix the login");
        assert_eq!(message.body, "The token expired.");
    }

    #[test]
    fn several_commits_give_the_branch_name_and_the_list_of_subjects() {
        let message = compose(
            &Branch::new("fix-login"),
            &[commit("First", "body one"), commit("Second", "")],
            None,
        );
        assert_eq!(message.title, "fix-login");
        assert_eq!(message.body, "- First\n- Second");
    }

    #[test]
    fn no_commits_give_the_branch_name_and_no_body() {
        let message = compose(&Branch::new("fix-login"), &[], None);
        assert_eq!(
            message,
            Message {
                title: "fix-login".to_owned(),
                body: String::new()
            }
        );
    }

    #[test]
    fn a_template_wins_for_the_body_but_not_the_title() {
        let template = "## What\n\n## Why\n\n";
        let one = compose(
            &Branch::new("fix"),
            &[commit("Fix it", "body")],
            Some(template),
        );
        assert_eq!(one.title, "Fix it");
        assert_eq!(one.body, "## What\n\n## Why");
        let none = compose(&Branch::new("fix"), &[], Some(template));
        assert_eq!(none.body, "## What\n\n## Why");
    }

    #[test]
    fn a_template_is_named_pull_request_template_in_any_case() {
        for name in [
            "pull_request_template.md",
            "PULL_REQUEST_TEMPLATE.md",
            "Pull_Request_Template.MD",
            "pull_request_template",
            "pull_request_template.txt",
        ] {
            assert!(is_template_name(name), "{name}");
        }
        for name in [
            "pull_request_template_old.md",
            "README.md",
            "pull-request-template.md",
        ] {
            assert!(!is_template_name(name), "{name}");
        }
    }

    #[test]
    fn uncommitted_files_count_once_each_renames_included() {
        let status = b" M src/a.rs\0?? new.txt\0R  b.rs\0old_b.rs\0A  c.rs\0";
        assert_eq!(count_uncommitted(status), 4);
        assert_eq!(count_uncommitted(b""), 0);
    }

    #[test]
    fn gh_creates_into_the_base_from_the_branch_with_the_title_and_body() {
        let args = create_args(
            &Branch::new("main"),
            &Branch::new("fix"),
            "Fix it",
            &Body::File(PathBuf::from("/tmp/body.md")),
            Readiness::Ready,
        );
        assert_eq!(
            args,
            [
                "pr",
                "create",
                "--base",
                "main",
                "--head",
                "fix",
                "--title",
                "Fix it",
                "--body-file",
                "/tmp/body.md"
            ]
        );
        let draft = create_args(
            &Branch::new("main"),
            &Branch::new("fix"),
            "Fix it",
            &Body::Empty,
            Readiness::Draft,
        );
        assert_eq!(draft[8..], ["--body", "", "--draft"]);
    }

    #[test]
    fn an_empty_title_is_refused() {
        assert!(check_title("  ").is_err());
        assert!(check_title("Fix").is_ok());
    }

    fn composed(commits: usize) -> Composed {
        Composed {
            message: Message {
                title: "t".to_owned(),
                body: String::new(),
            },
            base: Branch::new("main"),
            branch: Branch::new("fix"),
            commits,
            uncommitted: 0,
        }
    }

    #[test]
    fn a_branch_with_no_commits_ahead_is_refused_as_no_commits() {
        let err = check_has_commits("demo", &composed(0)).unwrap_err();
        assert_eq!(error::kind(&err), "no_commits");
        assert!(check_has_commits("demo", &composed(2)).is_ok());
    }

    fn pr(state: PrState) -> Found {
        Found::Pr(PullRequest {
            number: 12,
            title: "t".to_owned(),
            state,
            url: "https://github.com/o/r/pull/12".to_owned(),
            checks: Checks::None,
        })
    }

    #[test]
    fn an_open_or_draft_pull_request_is_refused_and_a_merged_or_closed_one_is_not() {
        for state in [PrState::Open, PrState::Draft] {
            let err = check_no_open_pr("demo", &pr(state)).unwrap_err();
            assert_eq!(error::kind(&err), "pr_exists");
            assert!(err.to_string().contains("#12"), "{err}");
        }
        for state in [PrState::Merged, PrState::Closed] {
            assert!(check_no_open_pr("demo", &pr(state)).is_ok());
        }
        assert!(check_no_open_pr("demo", &Found::NoPr).is_ok());
    }

    #[test]
    fn gh_unable_to_say_is_a_prerequisite_with_its_reason() {
        let err = check_no_open_pr(
            "demo",
            &Found::Unavailable("GitHub CLI is unavailable".to_owned()),
        )
        .unwrap_err();
        assert_eq!(error::kind(&err), "prerequisite");
        assert_eq!(error::exit_code(&err), 9);
        assert!(
            err.to_string().contains("GitHub CLI is unavailable"),
            "{err}"
        );
    }

    #[test]
    fn the_created_pull_request_is_read_from_its_address() {
        let out = b"Warning: 1 uncommitted change\nhttps://github.com/o/r/pull/12\n";
        let pr = created_pr(out, "Fix it", Readiness::Draft).unwrap();
        assert_eq!(pr.number, 12);
        assert_eq!(pr.url, "https://github.com/o/r/pull/12");
        assert_eq!(pr.state, PrState::Draft);
        assert_eq!(pr.title, "Fix it");
        assert_eq!(created_pr(b"nothing\n", "t", Readiness::Ready), None);
    }
}
