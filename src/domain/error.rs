//! Domain: the failures a client must tell apart (ADR 0010). Each one has a `kind`, which `--json`
//! prints as `error.kind`, and an exit code of its own.
//!
//! A typed error travels inside an `anyhow::Error` like any other, so the code between the failure
//! and `main` needs no conversions; [`kind`] and [`exit_code`] find it again by downcasting. Every
//! other failure has the kind `other` and exits 1. Exit code 2 is clap's, for a usage error.

use std::path::{Path, PathBuf};

use crate::domain::agent::Agent;
use crate::domain::prerequisite::DOCTOR_HINT;
use crate::domain::sandbox;
use crate::domain::trust::Subject;

/// The exit code of a failure that is not typed.
pub(crate) const FAILURE_EXIT: u8 = 1;

/// The `error.kind` of a failure that is not typed.
pub(crate) const OTHER_KIND: &str = "other";

/// A failure the client reacts to on its own, rather than only showing its message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum TypedError {
    /// The worktree is in use, as `reason` says, so a command that changes it must wait.
    #[error("{}", busy_message(name, path, *reason))]
    Busy {
        name: String,
        path: PathBuf,
        reason: BusyReason,
    },
    /// The sandbox `agent` runs in cannot run on this machine: `missing` names the tools absent
    /// from PATH (empty when the system has no sandbox at all), and `hint` the fix.
    #[error("{}", no_sandbox_message(*agent, missing, hint))]
    NoSandbox {
        agent: Agent,
        missing: Vec<String>,
        hint: String,
    },
    /// The worktree has modified or untracked files, so `worktree rm` keeps it unless forced.
    /// `stopped_agent` says its agent was stopped before the files were found: they appeared
    /// while it stopped, and it stays stopped (#94).
    #[error("{}", dirty_message(name, path, *stopped_agent))]
    Dirty {
        name: String,
        path: PathBuf,
        stopped_agent: bool,
    },
    /// The worktree's current session cannot be resumed on a cold start, for `reason`;
    /// `session` is the id `status.json` recorded, as written there. Nothing was launched.
    #[error("{}", session_gone_message(name, session, reason))]
    SessionGone {
        name: String,
        session: String,
        reason: GoneReason,
    },
    /// No holder is alive in the worktree, so its pending notes have no agent to be sent to.
    /// Nothing was sent and every note stays pending.
    #[error(
        "worktree {name} at {} has no running agent to send notes to; start it with \
         `orca-term agent start {name}`, then send again (the notes stay pending)",
        path.display()
    )]
    NoHolder { name: String, path: PathBuf },
    /// A program the command needs is missing or too old (`domain/prerequisite.rs`): `problems`
    /// are the doctor's lines for each one, which name the fix. Nothing is installed.
    #[error("{}; {DOCTOR_HINT}", problems.join("; "))]
    Prerequisite { problems: Vec<String> },
    /// What the worktree's committed `orca-term.yaml` asks for, its setup script or its sandbox's
    /// writable paths (`subject`), is not approved in this repository (`domain/trust.rs`): nobody
    /// could be asked, or the person said no. A script did not run, and the worktree is not ready;
    /// an agent did not start. `file` is the committed file, so a client can show both before
    /// approving.
    #[error("{}", untrusted_message(name, subject, file))]
    Untrusted {
        name: String,
        subject: Subject,
        file: PathBuf,
    },
    /// The worktree's branch has no commits ahead of the base, so `pr create` has nothing to open
    /// a pull request with (#132). Nothing was pushed.
    #[error(
        "worktree {name} has no commits ahead of {base} on its branch {branch}, so there is \
         nothing to open a pull request with; commit your changes first (nothing was pushed)"
    )]
    NoCommits {
        name: String,
        branch: String,
        base: String,
    },
    /// The worktree's branch already has an open pull request (or a draft), `number` at `url`,
    /// as gh says, so `pr create` opens no other (#132). Nothing was pushed.
    #[error(
        "worktree {name} already has an open pull request, #{number}: {url} (nothing was pushed)"
    )]
    PrExists {
        name: String,
        number: u64,
        url: String,
    },
    /// `git push` of the worktree's branch failed, for `reason`, git's own words (#132). No pull
    /// request was created.
    #[error(
        "could not push branch {branch} of worktree {name} to origin: {reason}; no pull request \
         was created"
    )]
    PushFailed {
        name: String,
        branch: String,
        reason: String,
    },
    /// gh cannot create the worktree's pull request, for `reason`, in the words `pr refresh` keeps
    /// (`GitHub CLI is unavailable`, gh's first line when it is not logged in). Its kind and exit
    /// code are `prerequisite`'s, but `doctor` does not check gh, so the message names gh's own fix
    /// (#132). Nothing was pushed.
    #[error(
        "cannot create a pull request for worktree {name}: {reason}; install the GitHub CLI (gh) \
         and log in with `gh auth login`, then try again (nothing was pushed)"
    )]
    GhUnavailable { name: String, reason: String },
    /// The worktree's branch has no open pull request for `pr merge` to merge: none at all, or one
    /// already merged or closed, as gh says (#136). Nothing was merged.
    #[error("{}", no_open_pr_message(name, *found))]
    NoOpenPr { name: String, found: NotOpen },
    /// The worktree's pull request `number` is a draft, which GitHub does not merge (#136).
    /// Nothing was merged.
    #[error(
        "PR #{number} of worktree {name} is a draft; mark it ready for review first (nothing was \
         merged)"
    )]
    PrDraft { name: String, number: u64 },
    /// GitHub reports the worktree's pull request `number` as conflicting with its base branch
    /// (`mergeable` is `CONFLICTING`, #136). Nothing was merged.
    #[error(
        "PR #{number} of worktree {name} conflicts with its base branch on GitHub; update the \
         branch first (nothing was merged)"
    )]
    PrConflicting { name: String, number: u64 },
    /// The review of the worktree's pull request `number` does not allow a merge yet: changes were
    /// requested, or an approving review is required (`reviewDecision`, #136). Nothing was merged.
    #[error("{}", not_approved_message(name, *number, *review))]
    NotApproved {
        name: String,
        number: u64,
        review: Review,
    },
    /// gh cannot merge the worktree's pull request, for `reason`, in the words `pr refresh` keeps.
    /// Its kind and exit code are `prerequisite`'s, and its message names gh's fix as
    /// [`TypedError::GhUnavailable`] does for `pr create` (#136). Nothing was merged.
    #[error(
        "cannot merge the pull request of worktree {name}: {reason}; install the GitHub CLI (gh) \
         and log in with `gh auth login`, then try again (nothing was merged)"
    )]
    MergeGhUnavailable { name: String, reason: String },
    /// The worktree has modified or untracked `files`, so `worktree rebase` starts no rebase
    /// (#141): it never stashes them. Its kind and exit code are `dirty`'s. Nothing was rebased.
    #[error(
        "worktree {name} at {} has changes not committed ({}); commit or stash them first \
         (nothing was rebased)",
        path.display(),
        listed(files)
    )]
    RebaseDirty {
        name: String,
        path: PathBuf,
        files: Vec<String>,
    },
    /// A git operation is already in progress in the worktree (`operation`, as `worktree ls`
    /// names it: `rebase`, `merge`, `cherry-pick` or `revert`), so `worktree rebase` starts none
    /// (#141). Nothing was rebased.
    #[error("{}", operation_in_progress_message(name, operation))]
    OperationInProgress {
        name: String,
        operation: &'static str,
    },
    /// The repository has no default branch on `origin` to rebase the worktree onto (#141). A
    /// rebase never goes onto a local branch instead. Nothing was rebased.
    #[error(
        "worktree {name} cannot be rebased: this repository has no default branch on an `origin` \
         remote to rebase onto, and a rebase never goes onto a local branch (nothing was rebased)"
    )]
    NoRemoteBase { name: String },
    /// The rebase of the worktree onto `base` stopped on conflicts in `files` (#141). It stays in
    /// progress, as in Orca, for the agent to resolve or the person to abort.
    #[error(
        "the rebase of worktree {name} onto {base} stopped on conflicts in {}; it stays in \
         progress: send them to the agent or abort the rebase, from the worktree menu or with \
         `orca-term agent resolve {name}` or `orca-term worktree rebase {name} --abort`",
        listed(files)
    )]
    RebaseConflict {
        name: String,
        base: String,
        files: Vec<String>,
    },
    /// The worktree has no rebase in progress, so there is nothing to abort or to resolve, as
    /// `asked` says (#141). Nothing was changed.
    #[error("{}", no_rebase_message(name, *asked))]
    NoRebase { name: String, asked: RebaseAsk },
    /// No holder is alive in the worktree, so `agent resolve` has no agent to type its prompt
    /// into (#141). Its kind and exit code are `no_holder`'s. The rebase stays in progress.
    #[error(
        "worktree {name} at {} has no running agent to resolve the conflicts; start it with \
         `orca-term agent start {name}`, then try again (the rebase stays in progress)",
        path.display()
    )]
    ResolveNoHolder { name: String, path: PathBuf },
    /// `worktree push` of the worktree's branch failed, for `reason`, git's own words (#146): any
    /// failure but a diverged branch, the refusal of a lease included. Its kind and exit code are
    /// `push_failed`'s.
    #[error("could not push branch {branch} of worktree {name} to origin: {reason}")]
    WorktreePushFailed {
        name: String,
        branch: String,
        reason: String,
    },
    /// git refused a plain push of the worktree's branch because it diverged from
    /// `origin/<branch>`, after a rebase for example (#146). A plain push never forces; only one
    /// asked for with `--force-with-lease` replaces the remote branch. Nothing was pushed.
    #[error(
        "branch {branch} of worktree {name} diverged from origin/{branch} (after a rebase?), so \
         git refused the push; `orca-term worktree push {name} --force-with-lease` would replace \
         origin/{branch} with it (nothing was pushed)"
    )]
    PushDiverged { name: String, branch: String },
    /// `project add` was given a directory that is not inside a git repository, so there is no
    /// primary checkout to add (#149). Nothing was added.
    #[error(
        "{} is not inside a git repository, so it cannot be added as a project (nothing was \
         added)",
        path.display()
    )]
    NotARepository { path: PathBuf },
    /// `project rm` was given a name that several projects share, the `candidates` (#149).
    /// Nothing was removed.
    #[error(
        "several projects are named {name}: {}; remove one by its path (nothing was removed)",
        listed_paths(candidates)
    )]
    AmbiguousProject {
        name: String,
        candidates: Vec<PathBuf>,
    },
}

/// `paths` as a refusal lists them: comma-separated.
fn listed_paths(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// What was asked of a rebase that is not in progress (#141).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RebaseAsk {
    /// `worktree rebase --abort`.
    Abort,
    /// `agent resolve`.
    Resolve,
}

/// The most file names a refusal lists before it says how many more there are.
const LISTED_FILES: usize = 5;

/// `files` as a refusal lists them: the first few, comma-separated, then how many more.
fn listed(files: &[String]) -> String {
    let shown = files
        .iter()
        .take(LISTED_FILES)
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    match files.len().saturating_sub(LISTED_FILES) {
        0 => shown,
        more => format!("{shown} and {more} more"),
    }
}

/// The message of [`TypedError::OperationInProgress`]: a rebase names how to resolve or abort it.
fn operation_in_progress_message(name: &str, operation: &str) -> String {
    if operation == "rebase" {
        format!(
            "worktree {name} has a rebase in progress already; send its conflicts to the agent \
             with `orca-term agent resolve {name}`, or abort it with `orca-term worktree rebase \
             {name} --abort` (nothing was rebased)"
        )
    } else {
        format!(
            "worktree {name} has a {operation} in progress; finish or abort it first (nothing \
             was rebased)"
        )
    }
}

/// The message of [`TypedError::NoRebase`].
fn no_rebase_message(name: &str, asked: RebaseAsk) -> String {
    match asked {
        RebaseAsk::Abort => format!("worktree {name} has no rebase in progress to abort"),
        RebaseAsk::Resolve => format!(
            "worktree {name} has no rebase in progress, so there are no conflicts to resolve"
        ),
    }
}

/// Why a worktree has no open pull request to merge (#136).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotOpen {
    /// Its branch has no pull request at all (or it is on no branch).
    None,
    /// Its pull request `number` is merged already.
    Merged(u64),
    /// Its pull request `number` was closed without a merge.
    Closed(u64),
}

/// A review decision that blocks a merge (#136).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Review {
    /// A reviewer requested changes (`CHANGES_REQUESTED`).
    ChangesRequested,
    /// The base branch needs an approving review, and there is none yet (`REVIEW_REQUIRED`).
    Required,
}

/// The message of [`TypedError::NoOpenPr`].
fn no_open_pr_message(name: &str, found: NotOpen) -> String {
    match found {
        NotOpen::None => format!(
            "worktree {name} has no pull request to merge; create one first with `orca-term pr \
             create {name}` (nothing was merged)"
        ),
        NotOpen::Merged(number) => {
            format!("PR #{number} of worktree {name} is merged already; there is nothing to merge")
        }
        NotOpen::Closed(number) => format!(
            "PR #{number} of worktree {name} is closed; reopen it on GitHub first (nothing was \
             merged)"
        ),
    }
}

/// The message of [`TypedError::NotApproved`].
fn not_approved_message(name: &str, number: u64, review: Review) -> String {
    match review {
        Review::ChangesRequested => format!(
            "PR #{number} of worktree {name} has changes requested in its review; address them \
             and get it approved first (nothing was merged)"
        ),
        Review::Required => format!(
            "PR #{number} of worktree {name} needs an approving review first (nothing was merged)"
        ),
    }
}

/// The message of [`TypedError::Untrusted`], naming the command that asks.
fn untrusted_message(name: &str, subject: &Subject, file: &Path) -> String {
    let file = file.display();
    match subject {
        Subject::Setup { .. } => format!(
            "the setup script of worktree {name}, from {file}, is not approved in this \
             repository, so it did not run and the worktree is not ready; read it, then run \
             `orca-term worktree setup {name}` in a terminal to be asked, or pass --approve-setup"
        ),
        Subject::Sandbox { writable } => format!(
            "the sandbox's extra writable paths of worktree {name} ({}), from {file}, are not \
             approved in this repository, so the agent did not start; read them, then run \
             `orca-term agent start {name}` in a terminal to be asked, or pass --approve-sandbox",
            writable.join(", ")
        ),
    }
}

/// Why a recorded session cannot be resumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GoneReason {
    /// The transcript claude kept the session in, at this path, no longer exists.
    TranscriptGone(PathBuf),
    /// No transcript was recorded for the session (or not as an absolute path).
    NoTranscript,
    /// The id is not one an agent makes, so it is never put on the agent's command line.
    InvalidId,
    /// opencode's own list of its sessions (`opencode session list`) does not hold it: deleted
    /// in opencode since (#41).
    NotListed,
}

/// Why a worktree is busy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BusyReason {
    /// Another process holds the worktree's lock: its setup, or another orca-term command, is
    /// running on it.
    Locked,
    /// Its agent is running: a holder is alive, and git must not touch the admin directory under
    /// it.
    AgentRunning,
    /// Another agent, `running`, runs in it, so `wanted` cannot start there (#35).
    AnotherAgent { running: Agent, wanted: Agent },
    /// Its agent is in a turn, working or waiting for the person, so it is not swapped under it.
    InTurn,
    /// Its agent is in a turn, working or waiting for the person, so its session is not changed
    /// under it (`session resume`, `session new`, #41).
    SessionInTurn,
    /// Its agent was asked to stop so the worktree could be removed, but did not (#94): nothing
    /// was removed.
    AgentDidNotStop,
    /// Its agent is in a turn, working or waiting for the person, so its branch is not rebased
    /// under it (#141).
    RebaseInTurn,
}

fn busy_message(name: &str, path: &Path, reason: BusyReason) -> String {
    let path = path.display();
    match reason {
        BusyReason::Locked => format!(
            "worktree {name} at {path} is busy: its setup or another orca-term command is running \
             on it; try again once it finishes"
        ),
        BusyReason::AgentRunning => format!(
            "worktree {name} at {path} is busy: its agent is running; stop it first with \
             `orca-term agent stop {name}`"
        ),
        BusyReason::AnotherAgent { running, wanted } => format!(
            "worktree {name} at {path} is busy: its agent {running} is running; swap it for \
             {wanted} with `orca-term agent swap {name} {wanted}`, or stop it first with \
             `orca-term agent stop {name}`"
        ),
        BusyReason::InTurn => format!(
            "worktree {name} at {path} is busy: its agent is in a turn, working or waiting for \
             you; swap agents once the turn ends, or stop it first with `orca-term agent stop \
             {name}`"
        ),
        BusyReason::SessionInTurn => format!(
            "worktree {name} at {path} is busy: its agent is in a turn, working or waiting for \
             you; change its session once the turn ends, or stop it first with `orca-term agent \
             stop {name}`"
        ),
        BusyReason::AgentDidNotStop => format!(
            "worktree {name} at {path} is busy: its agent did not stop, so nothing was removed; \
             try again, or stop it with `orca-term agent stop {name}`"
        ),
        BusyReason::RebaseInTurn => format!(
            "worktree {name} at {path} is busy: its agent is in a turn, working or waiting for \
             you; rebase once the turn ends, or stop it first with `orca-term agent stop {name}` \
             (nothing was rebased)"
        ),
    }
}

/// The message of [`TypedError::Dirty`], saying the agent was stopped when it was.
fn dirty_message(name: &str, path: &Path, stopped_agent: bool) -> String {
    let path = path.display();
    let stopped = if stopped_agent {
        "; its agent was stopped, but the worktree was kept"
    } else {
        ""
    };
    format!(
        "worktree {name} at {path} has modified or untracked files{stopped}; pass --force to \
         remove it anyway, discarding its changes"
    )
}

/// The message of [`TypedError::SessionGone`]. The id is shown cut to one short line, since an
/// id that is not one claude makes may hold anything.
fn session_gone_message(name: &str, session: &str, reason: &GoneReason) -> String {
    let reason = match reason {
        GoneReason::TranscriptGone(path) => format!("its transcript {} is gone", path.display()),
        GoneReason::NoTranscript => "no transcript of it is recorded".to_owned(),
        GoneReason::InvalidId => "its id is not one an agent makes".to_owned(),
        GoneReason::NotListed => {
            "opencode no longer lists it in `opencode session list`".to_owned()
        }
    };
    let session: String = session
        .chars()
        .take(SHOWN_ID_CHARS)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!(
        "worktree {name} cannot resume its session {session}: {reason}; start a fresh session \
         with `orca-term agent start {name} --fresh`"
    )
}

/// The most characters of a session id an error message shows.
const SHOWN_ID_CHARS: usize = 128;

/// The message of [`TypedError::NoSandbox`]: whose sandbox, then the diagnosis `orca-term doctor`
/// shows for the sandbox, in the same words.
fn no_sandbox_message(agent: Agent, missing: &[String], hint: &str) -> String {
    format!(
        "{}'s sandbox is not available: {}; {DOCTOR_HINT}",
        agent.program(),
        sandbox::diagnosis(missing, hint)
    )
}

impl TypedError {
    /// How `--json` names the failure, in `error.kind`.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Busy { .. } => "busy",
            Self::NoSandbox { .. } => "no_sandbox",
            Self::Dirty { .. } | Self::RebaseDirty { .. } => "dirty",
            Self::SessionGone { .. } => "session_gone",
            Self::NoHolder { .. } | Self::ResolveNoHolder { .. } => "no_holder",
            Self::Prerequisite { .. }
            | Self::GhUnavailable { .. }
            | Self::MergeGhUnavailable { .. } => "prerequisite",
            Self::Untrusted { .. } => "untrusted",
            Self::NoCommits { .. } => "no_commits",
            Self::PrExists { .. } => "pr_exists",
            Self::PushFailed { .. } | Self::WorktreePushFailed { .. } => "push_failed",
            Self::NoOpenPr { .. } => "no_open_pr",
            Self::PrDraft { .. } => "pr_draft",
            Self::PrConflicting { .. } => "pr_conflicting",
            Self::NotApproved { .. } => "not_approved",
            Self::OperationInProgress { .. } => "operation_in_progress",
            Self::NoRemoteBase { .. } => "no_remote_base",
            Self::RebaseConflict { .. } => "rebase_conflict",
            Self::NoRebase { .. } => "no_rebase",
            Self::PushDiverged { .. } => "push_diverged",
            Self::NotARepository { .. } => "not_a_repository",
            Self::AmbiguousProject { .. } => "ambiguous_project",
        }
    }

    /// The process's exit code for the failure.
    pub(crate) fn exit_code(&self) -> u8 {
        match self {
            Self::Busy { .. } => 3,
            Self::NoSandbox { .. } => 5,
            Self::Dirty { .. } | Self::RebaseDirty { .. } => 6,
            Self::SessionGone { .. } => 4,
            Self::NoHolder { .. } | Self::ResolveNoHolder { .. } => 7,
            Self::Prerequisite { .. }
            | Self::GhUnavailable { .. }
            | Self::MergeGhUnavailable { .. } => 9,
            Self::Untrusted { .. } => 8,
            Self::NoCommits { .. } => 10,
            Self::PrExists { .. } => 11,
            Self::PushFailed { .. } | Self::WorktreePushFailed { .. } => 12,
            Self::NoOpenPr { .. } => 13,
            Self::PrDraft { .. } => 14,
            Self::PrConflicting { .. } => 15,
            Self::NotApproved { .. } => 16,
            Self::OperationInProgress { .. } => 17,
            Self::NoRemoteBase { .. } => 18,
            Self::RebaseConflict { .. } => 19,
            Self::NoRebase { .. } => 20,
            Self::PushDiverged { .. } => 21,
            Self::NotARepository { .. } => 22,
            Self::AmbiguousProject { .. } => 23,
        }
    }
}

/// The typed error inside `err`, at any depth of its context chain.
pub(crate) fn typed(err: &anyhow::Error) -> Option<&TypedError> {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<TypedError>())
}

/// The `error.kind` of `err`: its typed error's kind, or `other`.
pub(crate) fn kind(err: &anyhow::Error) -> &'static str {
    typed(err).map_or(OTHER_KIND, TypedError::kind)
}

/// The exit code of `err`: its typed error's code, or 1.
pub(crate) fn exit_code(err: &anyhow::Error) -> u8 {
    typed(err).map_or(FAILURE_EXIT, TypedError::exit_code)
}

#[cfg(test)]
mod tests {
    use anyhow::{Context as _, anyhow};

    use super::*;

    fn busy(reason: BusyReason) -> TypedError {
        TypedError::Busy {
            name: "fix".to_owned(),
            path: PathBuf::from("/w/repo/fix"),
            reason,
        }
    }

    #[test]
    fn a_busy_worktree_has_its_own_kind_and_exit_code() {
        let err = anyhow::Error::new(busy(BusyReason::Locked));
        assert_eq!(kind(&err), "busy");
        assert_eq!(exit_code(&err), 3);
        assert_eq!(
            err.to_string(),
            "worktree fix at /w/repo/fix is busy: its setup or another orca-term command is \
             running on it; try again once it finishes"
        );
    }

    #[test]
    fn a_worktree_whose_agent_runs_is_busy_and_names_agent_stop() {
        let err = anyhow::Error::new(busy(BusyReason::AgentRunning));
        assert_eq!(kind(&err), "busy");
        assert_eq!(exit_code(&err), 3);
        assert_eq!(
            err.to_string(),
            "worktree fix at /w/repo/fix is busy: its agent is running; stop it first with \
             `orca-term agent stop fix`"
        );
    }

    #[test]
    fn a_missing_sandbox_has_its_own_kind_and_exit_code() {
        let err = anyhow::Error::new(TypedError::NoSandbox {
            agent: Agent::ClaudeCode,
            missing: vec!["bwrap".to_owned(), "socat".to_owned()],
            hint: "install them".to_owned(),
        });
        assert_eq!(kind(&err), "no_sandbox");
        assert_eq!(exit_code(&err), 5);
        assert_eq!(
            err.to_string(),
            "claude's sandbox is not available: bwrap and socat not found on PATH; install them; \
             run orca-term doctor for the full report"
        );
        let err = anyhow::Error::new(TypedError::NoSandbox {
            agent: Agent::OpenCode,
            missing: vec!["bwrap".to_owned()],
            hint: "install it".to_owned(),
        });
        assert_eq!(kind(&err), "no_sandbox");
        assert!(
            err.to_string()
                .starts_with("opencode's sandbox is not available: bwrap not found on PATH"),
            "{err}"
        );
    }

    #[test]
    fn a_worktree_running_another_agent_is_busy_naming_it_and_the_swap() {
        let err = anyhow::Error::new(busy(BusyReason::AnotherAgent {
            running: Agent::OpenCode,
            wanted: Agent::ClaudeCode,
        }));
        assert_eq!(kind(&err), "busy");
        assert_eq!(exit_code(&err), 3);
        assert_eq!(
            err.to_string(),
            "worktree fix at /w/repo/fix is busy: its agent opencode is running; swap it for \
             claude-code with `orca-term agent swap fix claude-code`, or stop it first with \
             `orca-term agent stop fix`"
        );
    }

    #[test]
    fn an_agent_in_a_turn_is_busy_for_a_swap() {
        let err = anyhow::Error::new(busy(BusyReason::InTurn));
        assert_eq!(kind(&err), "busy");
        assert_eq!(exit_code(&err), 3);
        assert_eq!(
            err.to_string(),
            "worktree fix at /w/repo/fix is busy: its agent is in a turn, working or waiting for \
             you; swap agents once the turn ends, or stop it first with `orca-term agent stop fix`"
        );
    }

    #[test]
    fn an_agent_in_a_turn_is_busy_for_a_session_change() {
        let err = anyhow::Error::new(busy(BusyReason::SessionInTurn));
        assert_eq!(kind(&err), "busy");
        assert_eq!(exit_code(&err), 3);
        assert_eq!(
            err.to_string(),
            "worktree fix at /w/repo/fix is busy: its agent is in a turn, working or waiting for \
             you; change its session once the turn ends, or stop it first with `orca-term agent \
             stop fix`"
        );
    }

    #[test]
    fn a_worktree_with_changes_has_its_own_kind_and_exit_code() {
        let err = anyhow::Error::new(TypedError::Dirty {
            name: "wip".to_owned(),
            path: PathBuf::from("/w/repo/wip"),
            stopped_agent: false,
        });
        assert_eq!(kind(&err), "dirty");
        assert_eq!(exit_code(&err), 6);
        assert_eq!(
            err.to_string(),
            "worktree wip at /w/repo/wip has modified or untracked files; pass --force to remove \
             it anyway, discarding its changes"
        );
    }

    #[test]
    fn a_dirty_refusal_after_stopping_the_agent_says_it_stays_stopped() {
        let err = anyhow::Error::new(TypedError::Dirty {
            name: "wip".to_owned(),
            path: PathBuf::from("/w/repo/wip"),
            stopped_agent: true,
        });
        assert_eq!(kind(&err), "dirty");
        assert_eq!(exit_code(&err), 6);
        assert_eq!(
            err.to_string(),
            "worktree wip at /w/repo/wip has modified or untracked files; its agent was stopped, \
             but the worktree was kept; pass --force to remove it anyway, discarding its changes"
        );
    }

    #[test]
    fn an_agent_that_does_not_stop_for_a_removal_is_busy_and_nothing_is_removed() {
        let err = anyhow::Error::new(busy(BusyReason::AgentDidNotStop));
        assert_eq!(kind(&err), "busy");
        assert_eq!(exit_code(&err), 3);
        assert_eq!(
            err.to_string(),
            "worktree fix at /w/repo/fix is busy: its agent did not stop, so nothing was removed; \
             try again, or stop it with `orca-term agent stop fix`"
        );
    }

    fn gone(reason: GoneReason) -> anyhow::Error {
        anyhow::Error::new(TypedError::SessionGone {
            name: "fix".to_owned(),
            session: "0b983a87-4074-466e-b864-422ce6f063c8".to_owned(),
            reason,
        })
    }

    #[test]
    fn a_session_that_cannot_be_resumed_has_its_own_kind_and_exit_code() {
        let err = gone(GoneReason::TranscriptGone(PathBuf::from("/h/t/s.jsonl")));
        assert_eq!(kind(&err), "session_gone");
        assert_eq!(exit_code(&err), 4);
        assert_eq!(
            err.to_string(),
            "worktree fix cannot resume its session 0b983a87-4074-466e-b864-422ce6f063c8: its \
             transcript /h/t/s.jsonl is gone; start a fresh session with `orca-term agent start \
             fix --fresh`"
        );
        let err = gone(GoneReason::InvalidId).to_string();
        assert!(
            err.contains(": its id is not one an agent makes; "),
            "{err}"
        );
        let err = gone(GoneReason::NoTranscript).to_string();
        assert!(err.contains(": no transcript of it is recorded; "), "{err}");
        let err = gone(GoneReason::NotListed).to_string();
        assert!(
            err.contains(": opencode no longer lists it in `opencode session list`; "),
            "{err}"
        );
    }

    #[test]
    fn a_session_id_is_shown_on_one_short_line() {
        let err = anyhow::Error::new(TypedError::SessionGone {
            name: "fix".to_owned(),
            session: format!("a\nb{}", "x".repeat(500)),
            reason: GoneReason::InvalidId,
        })
        .to_string();
        assert!(!err.contains('\n'), "{err}");
        assert!(err.contains("session a bxxx"), "{err}");
        assert!(!err.contains(&"x".repeat(127)), "{err}");
    }

    #[test]
    fn a_worktree_with_no_holder_has_its_own_kind_and_exit_code() {
        let err = anyhow::Error::new(TypedError::NoHolder {
            name: "fix".to_owned(),
            path: PathBuf::from("/w/repo/fix"),
        });
        assert_eq!(kind(&err), "no_holder");
        assert_eq!(exit_code(&err), 7);
        assert_eq!(
            err.to_string(),
            "worktree fix at /w/repo/fix has no running agent to send notes to; start it with \
             `orca-term agent start fix`, then send again (the notes stay pending)"
        );
    }

    #[test]
    fn a_missing_prerequisite_has_its_own_kind_and_exit_code() {
        let err = anyhow::Error::new(TypedError::Prerequisite {
            problems: vec![
                "git 2.30.1: orca-term needs 2.38 or later".to_owned(),
                "claude: not found on PATH; install Claude Code".to_owned(),
            ],
        });
        assert_eq!(kind(&err), "prerequisite");
        assert_eq!(exit_code(&err), 9);
        assert_eq!(
            err.to_string(),
            "git 2.30.1: orca-term needs 2.38 or later; claude: not found on PATH; install Claude \
             Code; run orca-term doctor for the full report"
        );
    }

    #[test]
    fn a_setup_script_not_approved_has_its_own_kind_and_exit_code() {
        let err = anyhow::Error::new(TypedError::Untrusted {
            name: "fix".to_owned(),
            subject: Subject::Setup {
                script: "make\n".to_owned(),
            },
            file: PathBuf::from("/w/repo/fix/orca-term.yaml"),
        });
        assert_eq!(kind(&err), "untrusted");
        assert_eq!(exit_code(&err), 8);
        assert_eq!(
            err.to_string(),
            "the setup script of worktree fix, from /w/repo/fix/orca-term.yaml, is not approved \
             in this repository, so it did not run and the worktree is not ready; read it, then \
             run `orca-term worktree setup fix` in a terminal to be asked, or pass \
             --approve-setup"
        );
    }

    #[test]
    fn writable_paths_not_approved_are_untrusted_too_and_name_agent_start() {
        let err = anyhow::Error::new(TypedError::Untrusted {
            name: "fix".to_owned(),
            subject: Subject::Sandbox {
                writable: vec!["~".to_owned(), "../shared".to_owned()],
            },
            file: PathBuf::from("/w/repo/fix/orca-term.yaml"),
        });
        assert_eq!(kind(&err), "untrusted");
        assert_eq!(exit_code(&err), 8);
        assert_eq!(
            err.to_string(),
            "the sandbox's extra writable paths of worktree fix (~, ../shared), from \
             /w/repo/fix/orca-term.yaml, are not approved in this repository, so the agent did \
             not start; read them, then run `orca-term agent start fix` in a terminal to be \
             asked, or pass --approve-sandbox"
        );
    }

    #[test]
    fn each_refusal_of_pr_create_has_its_own_kind_and_exit_code() {
        let err = anyhow::Error::new(TypedError::NoCommits {
            name: "demo".to_owned(),
            branch: "demo".to_owned(),
            base: "main".to_owned(),
        });
        assert_eq!((kind(&err), exit_code(&err)), ("no_commits", 10));
        assert_eq!(
            err.to_string(),
            "worktree demo has no commits ahead of main on its branch demo, so there is nothing \
             to open a pull request with; commit your changes first (nothing was pushed)"
        );
        let err = anyhow::Error::new(TypedError::PrExists {
            name: "demo".to_owned(),
            number: 12,
            url: "https://github.com/o/r/pull/12".to_owned(),
        });
        assert_eq!((kind(&err), exit_code(&err)), ("pr_exists", 11));
        assert_eq!(
            err.to_string(),
            "worktree demo already has an open pull request, #12: \
             https://github.com/o/r/pull/12 (nothing was pushed)"
        );
        let err = anyhow::Error::new(TypedError::PushFailed {
            name: "demo".to_owned(),
            branch: "demo".to_owned(),
            reason: "! [rejected] demo -> demo (fetch first)".to_owned(),
        });
        assert_eq!((kind(&err), exit_code(&err)), ("push_failed", 12));
        assert_eq!(
            err.to_string(),
            "could not push branch demo of worktree demo to origin: ! [rejected] demo -> demo \
             (fetch first); no pull request was created"
        );
        let err = anyhow::Error::new(TypedError::GhUnavailable {
            name: "demo".to_owned(),
            reason: "GitHub CLI is unavailable".to_owned(),
        });
        assert_eq!((kind(&err), exit_code(&err)), ("prerequisite", 9));
        assert_eq!(
            err.to_string(),
            "cannot create a pull request for worktree demo: GitHub CLI is unavailable; install \
             the GitHub CLI (gh) and log in with `gh auth login`, then try again (nothing was \
             pushed)"
        );
    }

    /// Checks that `typed` has the kind `wanted_kind`, the exit code `code` and says `message`.
    fn assert_typed(typed: TypedError, wanted_kind: &str, code: u8, message: &str) {
        let err = anyhow::Error::new(typed);
        assert_eq!((kind(&err), exit_code(&err)), (wanted_kind, code));
        assert_eq!(err.to_string(), message);
    }

    #[test]
    fn no_open_pull_request_to_merge_has_its_own_kind_and_exit_code() {
        let no_open = |found| TypedError::NoOpenPr {
            name: "demo".to_owned(),
            found,
        };
        assert_typed(
            no_open(NotOpen::None),
            "no_open_pr",
            13,
            "worktree demo has no pull request to merge; create one first with `orca-term pr \
             create demo` (nothing was merged)",
        );
        assert_typed(
            no_open(NotOpen::Merged(12)),
            "no_open_pr",
            13,
            "PR #12 of worktree demo is merged already; there is nothing to merge",
        );
        assert_typed(
            no_open(NotOpen::Closed(12)),
            "no_open_pr",
            13,
            "PR #12 of worktree demo is closed; reopen it on GitHub first (nothing was merged)",
        );
    }

    #[test]
    fn each_other_refusal_of_pr_merge_has_its_own_kind_and_exit_code() {
        let name = || "demo".to_owned();
        let draft = TypedError::PrDraft {
            name: name(),
            number: 12,
        };
        assert_typed(
            draft,
            "pr_draft",
            14,
            "PR #12 of worktree demo is a draft; mark it ready for review first (nothing was \
             merged)",
        );
        let conflicting = TypedError::PrConflicting {
            name: name(),
            number: 12,
        };
        assert_typed(
            conflicting,
            "pr_conflicting",
            15,
            "PR #12 of worktree demo conflicts with its base branch on GitHub; update the branch \
             first (nothing was merged)",
        );
        let not_approved = |review| TypedError::NotApproved {
            name: name(),
            number: 12,
            review,
        };
        assert_typed(
            not_approved(Review::ChangesRequested),
            "not_approved",
            16,
            "PR #12 of worktree demo has changes requested in its review; address them and get it \
             approved first (nothing was merged)",
        );
        assert_typed(
            not_approved(Review::Required),
            "not_approved",
            16,
            "PR #12 of worktree demo needs an approving review first (nothing was merged)",
        );
        let gh = TypedError::MergeGhUnavailable {
            name: name(),
            reason: "GitHub CLI is unavailable".to_owned(),
        };
        assert_typed(
            gh,
            "prerequisite",
            9,
            "cannot merge the pull request of worktree demo: GitHub CLI is unavailable; install \
             the GitHub CLI (gh) and log in with `gh auth login`, then try again (nothing was \
             merged)",
        );
    }

    #[test]
    fn the_rebase_refusals_have_their_kinds_and_exit_codes() {
        let files = |n: usize| (1..=n).map(|i| format!("f{i}.txt")).collect::<Vec<_>>();
        assert_typed(
            busy(BusyReason::RebaseInTurn),
            "busy",
            3,
            "worktree fix at /w/repo/fix is busy: its agent is in a turn, working or waiting for \
             you; rebase once the turn ends, or stop it first with `orca-term agent stop fix` \
             (nothing was rebased)",
        );
        assert_typed(
            TypedError::RebaseDirty {
                name: "demo".to_owned(),
                path: PathBuf::from("/w/demo"),
                files: files(7),
            },
            "dirty",
            6,
            "worktree demo at /w/demo has changes not committed (f1.txt, f2.txt, f3.txt, f4.txt, \
             f5.txt and 2 more); commit or stash them first (nothing was rebased)",
        );
        assert_typed(
            TypedError::OperationInProgress {
                name: "demo".to_owned(),
                operation: "merge",
            },
            "operation_in_progress",
            17,
            "worktree demo has a merge in progress; finish or abort it first (nothing was \
             rebased)",
        );
        assert_typed(
            TypedError::OperationInProgress {
                name: "demo".to_owned(),
                operation: "rebase",
            },
            "operation_in_progress",
            17,
            "worktree demo has a rebase in progress already; send its conflicts to the agent \
             with `orca-term agent resolve demo`, or abort it with `orca-term worktree rebase \
             demo --abort` (nothing was rebased)",
        );
        assert_typed(
            TypedError::NoRemoteBase {
                name: "demo".to_owned(),
            },
            "no_remote_base",
            18,
            "worktree demo cannot be rebased: this repository has no default branch on an \
             `origin` remote to rebase onto, and a rebase never goes onto a local branch \
             (nothing was rebased)",
        );
    }

    #[test]
    fn a_rebase_conflict_names_its_files_and_the_ways_out() {
        assert_typed(
            TypedError::RebaseConflict {
                name: "demo".to_owned(),
                base: "origin/main".to_owned(),
                files: vec!["a.txt".to_owned()],
            },
            "rebase_conflict",
            19,
            "the rebase of worktree demo onto origin/main stopped on conflicts in a.txt; it stays \
             in progress: send them to the agent or abort the rebase, from the worktree menu or \
             with `orca-term agent resolve demo` or `orca-term worktree rebase demo --abort`",
        );
        let no_rebase = |asked| TypedError::NoRebase {
            name: "demo".to_owned(),
            asked,
        };
        assert_typed(
            no_rebase(RebaseAsk::Abort),
            "no_rebase",
            20,
            "worktree demo has no rebase in progress to abort",
        );
        assert_typed(
            no_rebase(RebaseAsk::Resolve),
            "no_rebase",
            20,
            "worktree demo has no rebase in progress, so there are no conflicts to resolve",
        );
        assert_typed(
            TypedError::ResolveNoHolder {
                name: "demo".to_owned(),
                path: PathBuf::from("/w/demo"),
            },
            "no_holder",
            7,
            "worktree demo at /w/demo has no running agent to resolve the conflicts; start it \
             with `orca-term agent start demo`, then try again (the rebase stays in progress)",
        );
    }

    #[test]
    fn the_push_refusals_have_their_kinds_and_exit_codes() {
        assert_typed(
            TypedError::PushDiverged {
                name: "demo".to_owned(),
                branch: "demo".to_owned(),
            },
            "push_diverged",
            21,
            "branch demo of worktree demo diverged from origin/demo (after a rebase?), so git \
             refused the push; `orca-term worktree push demo --force-with-lease` would replace \
             origin/demo with it (nothing was pushed)",
        );
        assert_typed(
            TypedError::WorktreePushFailed {
                name: "demo".to_owned(),
                branch: "demo".to_owned(),
                reason: "! [rejected] demo -> demo (stale info)".to_owned(),
            },
            "push_failed",
            12,
            "could not push branch demo of worktree demo to origin: ! [rejected] demo -> demo \
             (stale info)",
        );
    }

    #[test]
    fn the_project_refusals_have_their_kinds_and_exit_codes() {
        assert_typed(
            TypedError::NotARepository {
                path: PathBuf::from("/tmp/notes"),
            },
            "not_a_repository",
            22,
            "/tmp/notes is not inside a git repository, so it cannot be added as a project \
             (nothing was added)",
        );
        assert_typed(
            TypedError::AmbiguousProject {
                name: "app".to_owned(),
                candidates: vec![PathBuf::from("/a/app"), PathBuf::from("/b/app")],
            },
            "ambiguous_project",
            23,
            "several projects are named app: /a/app, /b/app; remove one by its path (nothing was \
             removed)",
        );
    }

    #[test]
    fn a_typed_error_is_found_under_context() {
        let err = Err::<(), _>(busy(BusyReason::Locked))
            .context("could not set up")
            .unwrap_err();
        assert_eq!(kind(&err), "busy");
        assert_eq!(exit_code(&err), 3);
    }

    #[test]
    fn any_other_failure_is_other_and_exits_1() {
        let err = anyhow!("git failed");
        assert_eq!(kind(&err), "other");
        assert_eq!(exit_code(&err), 1);
    }
}
