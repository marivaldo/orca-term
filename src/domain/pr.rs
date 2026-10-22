//! Domain: a worktree's pull request on GitHub and the state of its checks (#87), as the GitHub
//! CLI reports them, and `pr.json`, where `pr refresh` keeps what it last found.
//!
//! `pr refresh` asks `gh pr view <branch> --json number,title,state,isDraft,url,statusCheckRollup`
//! in each worktree ([`view_args`]) and turns what gh printed into one of three findings
//! ([`Found`]): the pull request, with its state (`open`, `draft`, `merged` or `closed`) and its
//! checks in one word (`passing`, `failing`, `pending` or `none`); no pull request; or gh could not
//! say, with the reason (gh missing, not logged in, not a GitHub remote, too slow).
//!
//! `pr.json`, in the worktree's admin directory, is a cache, never a source of truth: only
//! `pr refresh` writes it, replacing it whole with no lock, the last write winning. `worktree ls`
//! and `worktree detail` only read it. Like every state file it carries `schema`: a file this core
//! cannot use reads as never fetched, and one of a newer schema is never written over.

use std::time::Duration;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// The newest schema of `pr.json` this core reads and writes.
pub(crate) const PR_SCHEMA: u64 = 1;

/// How long gh may take to answer for one worktree before it is stopped.
pub(crate) const GH_TIMEOUT: Duration = Duration::from_secs(10);

/// Why nothing is known when gh is not on PATH.
pub(crate) const GH_MISSING: &str = "GitHub CLI is unavailable";

/// The fields `gh pr view` is asked for.
const VIEW_FIELDS: &str = "number,title,state,isDraft,url,statusCheckRollup";

/// What gh says on stderr when the branch has no pull request.
const NO_PULL_REQUEST: &str = "no pull requests found";

/// Why nothing is known when the repository has no remote on GitHub.
pub(crate) const NOT_GITHUB: &str = "not a GitHub remote";

/// What gh says on stderr, lowercased, when the repository has no remote on GitHub: none at all,
/// or none pointing to a GitHub host.
const NO_GITHUB_REMOTE: &[&str] = &[
    "no git remotes found",
    "none of the git remotes configured for this repository point to a known github host",
];

/// gh's arguments to report the pull request of `branch`, run in the worktree.
pub(crate) fn view_args(branch: &str) -> Vec<String> {
    ["pr", "view", branch, "--json", VIEW_FIELDS]
        .iter()
        .map(|&arg| arg.to_owned())
        .collect()
}

/// Where a pull request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum PrState {
    Open,
    /// Open, but marked as a draft.
    Draft,
    Merged,
    Closed,
}

impl PrState {
    /// How the contract and the text name the state.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Draft => "draft",
            Self::Merged => "merged",
            Self::Closed => "closed",
        }
    }
}

/// The pull request's checks in one word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Checks {
    /// Every check that ran succeeded (skipped ones aside).
    Passing,
    /// At least one check failed, errored, was cancelled or timed out.
    Failing,
    /// None failed, and at least one is queued, waiting or running.
    Pending,
    /// No check reported anything but skipped.
    None,
}

impl Checks {
    /// How the contract and the text name the checks.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Passing => "passing",
            Self::Failing => "failing",
            Self::Pending => "pending",
            Self::None => "none",
        }
    }
}

/// A worktree's pull request, as `pr.json` keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct PullRequest {
    pub(crate) number: u64,
    pub(crate) title: String,
    pub(crate) state: PrState,
    pub(crate) url: String,
    pub(crate) checks: Checks,
}

/// What asking gh found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Found {
    /// The branch's pull request.
    Pr(PullRequest),
    /// The branch has no pull request (or the worktree is on no branch).
    NoPr,
    /// gh could not say, for the reason given.
    Unavailable(String),
}

/// What `pr refresh` found for a worktree, and when (milliseconds since the Unix epoch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PrFacts {
    pub(crate) fetched_at: u64,
    pub(crate) found: Found,
}

/// What running gh for one worktree did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GhRun {
    /// gh is not on PATH.
    Missing,
    /// gh could not be started, for the reason given.
    NotStarted(String),
    /// gh did not answer within [`GH_TIMEOUT`] and was stopped.
    TimedOut,
    /// gh exited: whether it succeeded, and what it printed.
    Exited {
        success: bool,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
}

/// What a run of gh means for the worktree.
pub(crate) fn found(run: &GhRun) -> Found {
    match run {
        GhRun::Missing => Found::Unavailable(GH_MISSING.to_owned()),
        GhRun::NotStarted(why) => Found::Unavailable(format!("could not run gh: {why}")),
        GhRun::TimedOut => Found::Unavailable(format!(
            "gh did not answer within {} seconds",
            GH_TIMEOUT.as_secs()
        )),
        GhRun::Exited {
            success: true,
            stdout,
            ..
        } => match parse_view(stdout) {
            Ok(pr) => Found::Pr(pr),
            Err(err) => Found::Unavailable(format!("gh printed what orca-term cannot read: {err}")),
        },
        GhRun::Exited { stderr, .. } => failed(stderr),
    }
}

/// What a failed `gh pr view` means: no pull request when gh says none was found, `not a GitHub
/// remote` when the repository has no remote on GitHub, else gh's first line on stderr as the
/// reason.
fn failed(stderr: &[u8]) -> Found {
    let text = String::from_utf8_lossy(stderr);
    let lower = text.to_lowercase();
    if lower.contains(NO_PULL_REQUEST) {
        return Found::NoPr;
    }
    if NO_GITHUB_REMOTE.iter().any(|words| lower.contains(words)) {
        return Found::Unavailable(NOT_GITHUB.to_owned());
    }
    let first = text.lines().map(str::trim).find(|line| !line.is_empty());
    Found::Unavailable(first.unwrap_or("gh pr view failed").to_owned())
}

/// What `gh pr view --json` prints: only the fields asked for.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct GhView {
    number: u64,
    title: String,
    state: String,
    #[serde(default)]
    is_draft: bool,
    url: String,
    #[serde(default)]
    status_check_rollup: Option<Vec<RollupItem>>,
}

/// One entry of `statusCheckRollup`: a check run (`status` and, once completed, `conclusion`) or
/// a commit status (`state`).
#[derive(Debug, Default, Deserialize)]
struct RollupItem {
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

/// The pull request in what `gh pr view --json` printed.
fn parse_view(stdout: &[u8]) -> Result<PullRequest> {
    let view: GhView = serde_json::from_slice(stdout)?;
    let state = match (view.state.as_str(), view.is_draft) {
        ("OPEN", true) => PrState::Draft,
        ("OPEN", false) => PrState::Open,
        ("MERGED", _) => PrState::Merged,
        ("CLOSED", _) => PrState::Closed,
        (other, _) => bail!("unknown pull request state `{other}`"),
    };
    Ok(PullRequest {
        number: view.number,
        title: view.title,
        state,
        url: view.url,
        checks: checks_of(view.status_check_rollup.as_deref().unwrap_or_default()),
    })
}

/// How one check reads, in the buckets gh's own `gh pr checks` sorts them into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Bucket {
    Pass,
    Fail,
    Pending,
    Skip,
}

/// The bucket of one entry of `statusCheckRollup`: its commit status, else its conclusion once
/// completed, else its status. Anything not passed, failed or skipped is still pending.
fn bucket(item: &RollupItem) -> Bucket {
    let word = match (&item.state, item.status.as_deref()) {
        (Some(state), _) if !state.is_empty() => state.as_str(),
        (_, Some("COMPLETED")) => item.conclusion.as_deref().unwrap_or_default(),
        (_, status) => status.unwrap_or_default(),
    };
    match word {
        "SUCCESS" => Bucket::Pass,
        "SKIPPED" | "NEUTRAL" => Bucket::Skip,
        "FAILURE" | "ERROR" | "CANCELLED" | "TIMED_OUT" | "ACTION_REQUIRED" | "STARTUP_FAILURE" => {
            Bucket::Fail
        }
        _ => Bucket::Pending,
    }
}

/// The checks in one word: any failure is `failing`, else anything pending is `pending`, else
/// any success is `passing`, else `none`.
fn checks_of(rollup: &[RollupItem]) -> Checks {
    let buckets: Vec<Bucket> = rollup.iter().map(bucket).collect();
    if buckets.contains(&Bucket::Fail) {
        Checks::Failing
    } else if buckets.contains(&Bucket::Pending) {
        Checks::Pending
    } else if buckets.contains(&Bucket::Pass) {
        Checks::Passing
    } else {
        Checks::None
    }
}

/// `pr.json` as it is written and read: `pr` null and `unavailable` null mean no pull request.
#[derive(Debug, Serialize, Deserialize)]
struct PrFile {
    schema: u64,
    fetched_at: u64,
    pr: Option<PullRequest>,
    unavailable: Option<String>,
}

/// Only the schema of `pr.json`, read before anything else.
#[derive(Debug, Deserialize)]
struct SchemaOnly {
    #[serde(default)]
    schema: Option<u64>,
}

/// What reading `pr.json` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PrRead {
    /// No file: `pr refresh` never ran for the worktree.
    Missing,
    /// What the last refresh found.
    Usable(PrFacts),
    /// A file whose schema is newer than this core supports.
    NewerSchema(u64),
    /// A file that is not a record, for the reason given.
    Unreadable(String),
}

impl PrRead {
    /// What the last refresh found, or `None` when there is nothing this core can use: the
    /// worktree reads as never fetched.
    pub(crate) fn usable(self) -> Option<PrFacts> {
        match self {
            Self::Usable(facts) => Some(facts),
            Self::Missing | Self::NewerSchema(_) | Self::Unreadable(_) => None,
        }
    }
}

/// What `pr.json`, whose content is `bytes` (`None` when it does not exist), holds.
pub(crate) fn read(bytes: Option<&[u8]>) -> PrRead {
    let Some(bytes) = bytes else {
        return PrRead::Missing;
    };
    match serde_json::from_slice::<SchemaOnly>(bytes) {
        Ok(SchemaOnly {
            schema: Some(schema),
        }) if schema > PR_SCHEMA => return PrRead::NewerSchema(schema),
        Ok(_) => {}
        Err(err) => return PrRead::Unreadable(err.to_string()),
    }
    match serde_json::from_slice::<PrFile>(bytes) {
        Ok(file) => PrRead::Usable(PrFacts {
            fetched_at: file.fetched_at,
            found: match (file.pr, file.unavailable) {
                (Some(pr), _) => Found::Pr(pr),
                (None, Some(reason)) => Found::Unavailable(reason),
                (None, None) => Found::NoPr,
            },
        }),
        Err(err) => PrRead::Unreadable(err.to_string()),
    }
}

/// The bytes written to `pr.json`.
pub(crate) fn to_bytes(facts: &PrFacts) -> Result<Vec<u8>> {
    let (pr, unavailable) = match &facts.found {
        Found::Pr(pr) => (Some(pr.clone()), None),
        Found::NoPr => (None, None),
        Found::Unavailable(reason) => (None, Some(reason.clone())),
    };
    Ok(serde_json::to_vec(&PrFile {
        schema: PR_SCHEMA,
        fetched_at: facts.fetched_at,
        pr,
        unavailable,
    })?)
}

/// Refuses to write over a `pr.json` whose schema is newer than this core supports. A missing or
/// unreadable file is written over.
pub(crate) fn check_overwrite(read: &PrRead) -> Result<()> {
    if let PrRead::NewerSchema(schema) = read {
        bail!(
            "pr.json has schema {schema}, newer than the schema {PR_SCHEMA} this orca-term \
             supports; upgrade orca-term to refresh this worktree's pull request"
        );
    }
    Ok(())
}

/// Whether `found` says something other than what the file held, `before`: the running Neovims
/// are told only then, so a refresh that changes nothing makes no one read the fleet again.
pub(crate) fn changed(before: &PrRead, found: &Found) -> bool {
    match before {
        PrRead::Usable(facts) => facts.found != *found,
        PrRead::Missing | PrRead::NewerSchema(_) | PrRead::Unreadable(_) => true,
    }
}

/// One worktree `pr refresh` asked about, and what it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefreshedWorktree {
    pub(crate) name: String,
    pub(crate) path: std::path::PathBuf,
    pub(crate) facts: PrFacts,
}

/// What `pr refresh` reports: every worktree it asked about, in the fleet's order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Refreshed {
    pub(crate) worktrees: Vec<RefreshedWorktree>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exited(success: bool, stdout: &str, stderr: &str) -> GhRun {
        GhRun::Exited {
            success,
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    fn view(state: &str, draft: bool, rollup: &str) -> String {
        format!(
            r#"{{"number":12,"title":"Fix the login","state":"{state}","isDraft":{draft},"url":"https://github.com/o/r/pull/12","statusCheckRollup":{rollup}}}"#
        )
    }

    fn pr_of(run: &GhRun) -> PullRequest {
        match found(run) {
            Found::Pr(pr) => pr,
            other => panic!("no pull request: {other:?}"),
        }
    }

    #[test]
    fn gh_is_asked_about_the_branch_and_the_fields_shown() {
        assert_eq!(
            view_args("fix"),
            [
                "pr",
                "view",
                "fix",
                "--json",
                "number,title,state,isDraft,url,statusCheckRollup"
            ]
        );
    }

    #[test]
    fn the_state_maps_open_draft_merged_and_closed() {
        for (state, draft, want) in [
            ("OPEN", false, PrState::Open),
            ("OPEN", true, PrState::Draft),
            ("MERGED", false, PrState::Merged),
            ("CLOSED", false, PrState::Closed),
        ] {
            let pr = pr_of(&exited(true, &view(state, draft, "[]"), ""));
            assert_eq!(pr.state, want, "{state} {draft}");
        }
        let pr = pr_of(&exited(true, &view("OPEN", false, "[]"), ""));
        assert_eq!(pr.number, 12);
        assert_eq!(pr.title, "Fix the login");
        assert_eq!(pr.url, "https://github.com/o/r/pull/12");
    }

    #[test]
    fn checks_fail_first_then_pend_then_pass_else_none() {
        let run = r#"{"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"}"#;
        let failed = r#"{"__typename":"CheckRun","status":"COMPLETED","conclusion":"FAILURE"}"#;
        let running = r#"{"__typename":"CheckRun","status":"IN_PROGRESS","conclusion":""}"#;
        let queued = r#"{"__typename":"CheckRun","status":"QUEUED","conclusion":""}"#;
        let skipped = r#"{"__typename":"CheckRun","status":"COMPLETED","conclusion":"SKIPPED"}"#;
        let status_error = r#"{"__typename":"StatusContext","state":"ERROR"}"#;
        let status_pending = r#"{"__typename":"StatusContext","state":"PENDING"}"#;
        let status_ok = r#"{"__typename":"StatusContext","state":"SUCCESS"}"#;
        for (rollup, want) in [
            (format!("[{run},{failed},{running}]"), Checks::Failing),
            (format!("[{run},{status_error}]"), Checks::Failing),
            (format!("[{run},{running}]"), Checks::Pending),
            (format!("[{queued}]"), Checks::Pending),
            (format!("[{status_ok},{status_pending}]"), Checks::Pending),
            (format!("[{run},{skipped},{status_ok}]"), Checks::Passing),
            (format!("[{skipped}]"), Checks::None),
            ("[]".to_owned(), Checks::None),
            ("null".to_owned(), Checks::None),
        ] {
            let pr = pr_of(&exited(true, &view("OPEN", false, &rollup), ""));
            assert_eq!(pr.checks, want, "{rollup}");
        }
    }

    #[test]
    fn no_pull_request_found_is_no_pr_and_any_other_failure_is_unavailable_with_its_first_line() {
        let none = exited(false, "", "no pull requests found for branch \"fix\"\n");
        assert_eq!(found(&none), Found::NoPr);
        let auth = exited(
            false,
            "",
            "\nTo get started with GitHub CLI, please run:  gh auth login\nmore\n",
        );
        assert_eq!(
            found(&auth),
            Found::Unavailable(
                "To get started with GitHub CLI, please run:  gh auth login".to_owned()
            )
        );
        assert_eq!(
            found(&exited(false, "", "")),
            Found::Unavailable("gh pr view failed".to_owned())
        );
    }

    #[test]
    fn a_repository_with_no_remote_on_github_is_not_a_github_remote() {
        for said in [
            "no git remotes found\n",
            "none of the git remotes configured for this repository point to a known GitHub \
             host. To tell gh about a new GitHub host, please use `gh auth login`\n",
        ] {
            assert_eq!(
                found(&exited(false, "", said)),
                Found::Unavailable("not a GitHub remote".to_owned()),
                "{said}"
            );
        }
    }

    #[test]
    fn a_missing_slow_or_unreadable_gh_is_unavailable() {
        assert_eq!(
            found(&GhRun::Missing),
            Found::Unavailable("GitHub CLI is unavailable".to_owned())
        );
        assert_eq!(
            found(&GhRun::TimedOut),
            Found::Unavailable("gh did not answer within 10 seconds".to_owned())
        );
        let Found::Unavailable(why) = found(&exited(true, "not json", "")) else {
            panic!("read as a pull request");
        };
        assert!(
            why.starts_with("gh printed what orca-term cannot read"),
            "{why}"
        );
        let Found::Unavailable(why) = found(&exited(true, &view("LOCKED", false, "[]"), "")) else {
            panic!("read as a pull request");
        };
        assert!(why.contains("unknown pull request state `LOCKED`"), "{why}");
    }

    #[test]
    fn the_file_keeps_each_finding_and_carries_its_schema() {
        let pr = PullRequest {
            number: 7,
            title: "t".to_owned(),
            state: PrState::Draft,
            url: "u".to_owned(),
            checks: Checks::Pending,
        };
        for found in [
            Found::Pr(pr),
            Found::NoPr,
            Found::Unavailable("why".to_owned()),
        ] {
            let facts = PrFacts {
                fetched_at: 42,
                found,
            };
            let bytes = to_bytes(&facts).unwrap();
            assert!(String::from_utf8_lossy(&bytes).starts_with(r#"{"schema":1,"fetched_at":42,"#));
            assert_eq!(read(Some(&bytes)), PrRead::Usable(facts));
        }
        assert_eq!(
            String::from_utf8(
                to_bytes(&PrFacts {
                    fetched_at: 1,
                    found: Found::NoPr
                })
                .unwrap()
            )
            .unwrap(),
            r#"{"schema":1,"fetched_at":1,"pr":null,"unavailable":null}"#
        );
    }

    #[test]
    fn an_unusable_file_reads_as_never_fetched_and_a_newer_one_is_never_written_over() {
        assert_eq!(read(None), PrRead::Missing);
        assert_eq!(read(None).usable(), None);
        assert!(matches!(read(Some(b"{")), PrRead::Unreadable(_)));
        let newer = read(Some(br#"{"schema":2,"fetched_at":1}"#));
        assert_eq!(newer, PrRead::NewerSchema(2));
        assert_eq!(newer.clone().usable(), None);
        assert!(check_overwrite(&newer).is_err());
        assert!(check_overwrite(&PrRead::Missing).is_ok());
        let unknown_fields = br#"{"schema":1,"fetched_at":5,"pr":null,"unavailable":null,"x":1}"#;
        assert_eq!(
            read(Some(unknown_fields)).usable(),
            Some(PrFacts {
                fetched_at: 5,
                found: Found::NoPr
            })
        );
    }

    #[test]
    fn only_a_different_finding_is_a_change() {
        let before = PrRead::Usable(PrFacts {
            fetched_at: 1,
            found: Found::NoPr,
        });
        assert!(!changed(&before, &Found::NoPr));
        assert!(changed(&before, &Found::Unavailable("x".to_owned())));
        assert!(changed(&PrRead::Missing, &Found::NoPr));
    }
}
