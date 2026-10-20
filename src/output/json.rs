//! Edge: the JSON documents `--json` prints. Domain types never derive `Serialize`: each document
//! is a view built here from them, so changing a domain type never silently changes the contract.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::domain::agent::{HolderStatus, StartOutcome, Started, StopOutcome, Stopped};
use crate::domain::blame::{Blame, Source};
use crate::domain::branch::Branch;
use crate::domain::contract::{CONTRACT, VERSION};
use crate::domain::detail::{Detail, SetupProgress};
use crate::domain::error::{self, TypedError};
use crate::domain::fleet::{Fleet, PrimaryCheckout, Worktree};
use crate::domain::history::{History, LoggedTurn, Progress, TurnDiff};
use crate::domain::launch::SessionId;
use crate::domain::note::{Anchor, Listed, Note, Noted, NotesSent, PendingNote, Unnoted};
use crate::domain::prerequisite::Report;
use crate::domain::review::Reviewed;
use crate::domain::seen::Marked;
use crate::domain::setup::{SetupDone, SetupOutcome, SetupReport};
use crate::domain::state::State;
use crate::domain::trust::Subject;
use crate::domain::turn::{Outcome, Phase};
use crate::domain::worktree::{BranchOutcome, Created, Removed};

/// Every document carries `contract` and `version` next to its own fields.
#[derive(Debug, Serialize)]
struct Envelope<'a, T: Serialize> {
    contract: u32,
    version: &'a str,
    #[serde(flatten)]
    body: T,
}

/// `worktree ls --json`.
#[derive(Debug, Serialize)]
struct FleetView<'a> {
    primary: PrimaryView<'a>,
    worktrees: Vec<WorktreeView<'a>>,
}

#[derive(Debug, Serialize)]
struct PrimaryView<'a> {
    path: &'a Path,
    branch: Option<&'a str>,
}

/// One worktree: `activity` is what its agent is doing or said last, beside a state from its
/// hooks (`working`, `waiting`, `finished`, `failed`, `interrupted`), else null; `agent` is its
/// recorded agent's name or null, `holder` whether a holder is alive, `unseen` whether its latest
/// turn ended after the person last reviewed it (or the person marked it so), `notes` how many
/// notes are pending on it, `denials` how many tool calls its agent was denied in its latest turn
/// (open or ended; never a state).
#[derive(Debug, Serialize)]
struct WorktreeView<'a> {
    name: &'a str,
    path: &'a Path,
    branch: Option<&'a str>,
    state: &'static str,
    detail: Option<String>,
    activity: Option<&'a str>,
    agent: Option<&'a str>,
    holder: bool,
    unseen: bool,
    notes: usize,
    denials: usize,
}

/// `worktree new --json`: the worktree as `worktree ls` shows it, and what was done to make it.
#[derive(Debug, Serialize)]
struct CreatedView<'a> {
    worktree: WorktreeView<'a>,
    start: String,
    copied: usize,
    base: &'a Path,
    setup: SetupView<'a>,
}

/// `worktree setup --json`: the worktree as `worktree ls` shows it, and what its setup did.
#[derive(Debug, Serialize)]
struct SetupDoneView<'a> {
    worktree: WorktreeView<'a>,
    setup: SetupView<'a>,
}

/// What a setup did: `status` is `none` (no script configured), `succeeded` or `failed`, and
/// `log` is the setup log, `null` when no script ran.
#[derive(Debug, Serialize)]
struct SetupView<'a> {
    status: &'static str,
    log: Option<&'a Path>,
}

/// `worktree rm --json`: the worktree removed, and what happened to its branch.
#[derive(Debug, Serialize)]
struct RemovedView<'a> {
    name: &'a str,
    path: &'a Path,
    branch: RemovedBranchView<'a>,
}

/// `status` is `deleted`, `kept` or `none` (the worktree was detached, so it had no branch);
/// `name` is the branch, null when there was none; `reason` says why it was deleted or kept.
#[derive(Debug, Serialize)]
struct RemovedBranchView<'a> {
    name: Option<&'a str>,
    status: &'static str,
    reason: Option<String>,
}

/// `agent start --json`: the worktree as `worktree ls` shows it, the holder, the sandbox's
/// extra writable paths with the config file that set them (`null` when nothing was launched),
/// and `resumed`, the id of the session the agent was started on with `--resume` (`null` for a
/// new session, or when nothing was launched).
#[derive(Debug, Serialize)]
struct StartedView<'a> {
    worktree: WorktreeView<'a>,
    agent: &'static str,
    holder: HolderView,
    writable: Option<WritableView<'a>>,
    resumed: Option<&'a str>,
}

/// `status` is `started` or `already_running`; `pid` is the holder's, when known.
#[derive(Debug, Serialize)]
struct HolderView {
    status: &'static str,
    pid: Option<u32>,
}

#[derive(Debug, Serialize)]
struct WritableView<'a> {
    paths: Vec<&'a Path>,
    source: Option<&'a str>,
}

/// `agent stop --json`: `status` is `stopped` or `not_running`.
#[derive(Debug, Serialize)]
struct StoppedView<'a> {
    worktree: WorktreeView<'a>,
    status: &'static str,
}

/// `worktree seen --json` and `worktree unseen --json`: the worktree, and whether it is unseen now.
#[derive(Debug, Serialize)]
struct MarkedView<'a> {
    name: &'a str,
    path: &'a Path,
    unseen: bool,
}

/// `worktree review --json`: the worktree as `worktree ls` shows it, now seen; `base`, the full id
/// of the commit the review compares the working tree with; and `against`, the default branch
/// that base was taken from, as git names it (`main` or `origin/main`).
#[derive(Debug, Serialize)]
struct ReviewedView<'a> {
    worktree: WorktreeView<'a>,
    base: &'a str,
    against: String,
}

/// `worktree detail --json`: the worktree as `worktree ls` shows it; `detail`, why it is in its
/// state, whole (every line: a failed setup's reason, a broken worktree's, or a failed turn's
/// whole error), else null; `denials`, the tool calls its agent was denied in its latest turn, in
/// order; `setup`, its latest setup (null when it never ran one); `drift`, how far it is from the
/// local default branch (null when there is none, or the histories are unrelated).
#[derive(Debug, Serialize)]
struct DetailView<'a> {
    worktree: WorktreeView<'a>,
    detail: Option<&'a str>,
    denials: Vec<DenialView<'a>>,
    setup: Option<SetupDetailView<'a>>,
    drift: Option<DriftView<'a>>,
}

/// One denied tool call: `tool` (`Bash`) and `text`, the call in one line (`Bash: FOO=bar ls`).
#[derive(Debug, Serialize)]
struct DenialView<'a> {
    tool: &'a str,
    text: &'a str,
}

/// A worktree's latest setup: `status` is `running`, `interrupted`, `succeeded` or `failed`;
/// `log` is the setup log; `tail` its last lines, terminal control sequences taken out.
#[derive(Debug, Serialize)]
struct SetupDetailView<'a> {
    status: &'static str,
    log: &'a Path,
    tail: &'a [String],
}

/// `ahead`: the worktree's commits the local default branch `against` lacks; `behind`: the
/// branch's commits the worktree lacks.
#[derive(Debug, Serialize)]
struct DriftView<'a> {
    against: &'a str,
    ahead: u64,
    behind: u64,
}

/// `note add --json` and `note edit --json`: the worktree, and the note as it is now.
#[derive(Debug, Serialize)]
struct NotedView<'a> {
    name: &'a str,
    path: &'a Path,
    note: NoteView<'a>,
}

/// `note ls --json`: the worktree, and its pending notes in id order.
#[derive(Debug, Serialize)]
struct ListedView<'a> {
    name: &'a str,
    path: &'a Path,
    notes: Vec<NoteView<'a>>,
}

/// `note rm --json`: the worktree, and the id of the note removed.
#[derive(Debug, Serialize)]
struct UnnotedView<'a> {
    name: &'a str,
    path: &'a Path,
    id: u64,
}

/// `note send --json`: the worktree, the `prompt` pasted into its agent, the ids of the notes
/// `sent` in it (all consumed but those `kept`), and the ids of those `kept` pending because they
/// were edited while being sent.
#[derive(Debug, Serialize)]
struct SentView<'a> {
    name: &'a str,
    path: &'a Path,
    prompt: &'a str,
    sent: &'a [u64],
    kept: &'a [u64],
}

/// One pending note: `path` is relative to the worktree, `anchor` where it was attached in the
/// file as it was then, `current` where that is in the file now (the whole file when its lines
/// are gone), `rev` its revision (bumped by every edit), `created` when it was made, in
/// milliseconds since the Unix epoch.
#[derive(Debug, Serialize)]
struct NoteView<'a> {
    id: u64,
    path: &'a str,
    anchor: AnchorView,
    current: AnchorView,
    body: &'a str,
    rev: u64,
    created: u64,
}

/// An anchor: `{"scope":"line","line":3}`, `{"scope":"lines","start":3,"end":5}` or
/// `{"scope":"file"}`.
#[derive(Debug, Serialize)]
#[serde(tag = "scope", rename_all = "lowercase")]
enum AnchorView {
    Line { line: u32 },
    Lines { start: u32, end: u32 },
    File,
}

/// `turn ls --json`: the worktree, and its turns oldest first, the open one last.
#[derive(Debug, Serialize)]
struct TurnsView<'a> {
    name: &'a str,
    path: &'a Path,
    turns: Vec<TurnView<'a>>,
}

/// One turn: `number` in the worktree; `agent` (null for a turn logged before it was recorded);
/// `session`; `prompt`, its first line; `started_at` and `ended_at` in milliseconds since the Unix
/// epoch (null when unknown, `ended_at` while open); `outcome`, `finished`, `failed` or
/// `interrupted` once ended, else null; `phase`, `working` or `waiting` for the open turn, else
/// null; `end_observed` (null while open and for a turn ended before the snapshots); `begin` and
/// `end`, its snapshots' full commit ids (null when not taken); `denials`, how many tool calls its
/// agent was denied during it.
#[derive(Debug, Serialize)]
struct TurnView<'a> {
    number: u64,
    agent: Option<&'a str>,
    session: Option<&'a str>,
    prompt: &'a str,
    started_at: Option<u64>,
    ended_at: Option<u64>,
    outcome: Option<&'static str>,
    phase: Option<&'static str>,
    end_observed: Option<bool>,
    begin: Option<&'a str>,
    end: Option<&'a str>,
    denials: usize,
}

/// `turn diff --json`: the worktree, the `turn`, its `begin` and `end` snapshots, whether its end
/// was observed (when false the patch may hold edits made after the interruption), and the
/// `patch` git printed (invalid UTF-8 replaced).
#[derive(Debug, Serialize)]
struct TurnDiffView<'a> {
    name: &'a str,
    path: &'a Path,
    turn: u64,
    begin: &'a str,
    end: &'a str,
    end_observed: Option<bool>,
    patch: String,
}

/// `turn blame --json`: the worktree, the `file` (relative to its root), and each of its lines on
/// disk now.
#[derive(Debug, Serialize)]
struct BlameView<'a> {
    name: &'a str,
    path: &'a Path,
    file: &'a str,
    lines: Vec<LineAuthorView<'a>>,
}

/// One line: its number `line` from 1; `source`, `turn` (a turn wrote it), `between_turns` (no
/// turn: the person between turns, or the files before the first turn) or `since_snapshot`
/// (changed since the latest snapshot); `turn` and `agent`, the turn that wrote it and its agent
/// (null otherwise); `snapshot`, the snapshot git found the line in (null for `since_snapshot`);
/// `text`.
#[derive(Debug, Serialize)]
struct LineAuthorView<'a> {
    line: usize,
    source: &'static str,
    turn: Option<u64>,
    agent: Option<&'a str>,
    snapshot: Option<&'a str>,
    text: &'a str,
}

/// `nvim register --json` and `nvim unregister --json`: nothing beyond `contract` and `version`.
#[derive(Debug, Serialize)]
struct DoneView {}

/// Any command's failure with `--json`: `kind` is `other` unless the failure is typed.
#[derive(Debug, Serialize)]
struct ErrorView<'a> {
    error: ErrorBody<'a>,
}

/// An error: its `kind` and `message`; `untrusted` also carries `what` was not approved
/// (`setup` or `sandbox`), the setup `script` or the `writable` paths as written, and the
/// committed file they come from (`source`), so the client can show them.
#[derive(Debug, Serialize)]
struct ErrorBody<'a> {
    kind: &'static str,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    what: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    script: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    writable: Option<&'a [String]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    source: Option<&'a Path>,
}

/// `doctor --json`: every check in order, and, when one is missing or too old, the `prerequisite`
/// error other commands would print for it.
#[derive(Debug, Serialize)]
struct DoctorView<'a> {
    checks: Vec<CheckView<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ErrorBody<'a>>,
}

/// One check: `status` is `ok`, `warn`, `missing`, `too_old` or `info`; `found` what was found
/// (such as a version) and `need` what orca-term needs (such as the oldest version), each null
/// when there is nothing to say; `message` the report's line after the status.
#[derive(Debug, Serialize)]
struct CheckView<'a> {
    name: &'static str,
    status: &'static str,
    found: Option<&'a str>,
    need: Option<&'a str>,
    message: &'a str,
}

/// Wraps `body` in the envelope and renders it.
fn document<T: Serialize>(body: T) -> serde_json::Result<String> {
    serde_json::to_string(&Envelope {
        contract: CONTRACT,
        version: VERSION,
        body,
    })
}

/// The fleet as one JSON document.
pub(crate) fn fleet(fleet: &Fleet) -> serde_json::Result<String> {
    document(FleetView {
        primary: primary_view(&fleet.primary),
        worktrees: fleet.worktrees.iter().map(worktree_view).collect(),
    })
}

/// What `worktree new` made, as one JSON document.
pub(crate) fn created(created: &Created) -> serde_json::Result<String> {
    let worktree = set_up_worktree(
        created.name.as_str(),
        &created.path,
        Some(&created.branch),
        &created.setup,
    );
    document(CreatedView {
        worktree: worktree_view(&worktree),
        start: created.start.to_string(),
        copied: created.copied,
        base: &created.base.value,
        setup: setup_view(&created.setup),
    })
}

/// What `worktree setup` did, as one JSON document.
pub(crate) fn set_up(done: &SetupDone) -> serde_json::Result<String> {
    let worktree = set_up_worktree(&done.name, &done.path, done.branch.as_ref(), &done.setup);
    document(SetupDoneView {
        worktree: worktree_view(&worktree),
        setup: setup_view(&done.setup),
    })
}

/// The worktree `name` at `path` on `branch` as its setup, `setup`, left it, as `worktree ls`
/// shows it. A setup runs only while no holder is alive.
fn set_up_worktree(
    name: &str,
    path: &Path,
    branch: Option<&Branch>,
    setup: &SetupReport,
) -> Worktree {
    Worktree {
        name: name.to_owned(),
        path: path.to_owned(),
        branch: branch.cloned(),
        state: setup.state.clone(),
        activity: setup.activity.clone(),
        agent: setup.agent.clone(),
        holder: HolderStatus::Gone,
        unseen: setup.unseen,
        notes: setup.notes,
        denials: setup.denials,
    }
}

/// What `worktree rm` removed, as one JSON document.
pub(crate) fn removed(removed: &Removed) -> serde_json::Result<String> {
    document(RemovedView {
        name: &removed.name,
        path: &removed.path,
        branch: removed_branch_view(&removed.branch),
    })
}

fn removed_branch_view(outcome: &BranchOutcome) -> RemovedBranchView<'_> {
    let (name, status, reason) = match outcome {
        BranchOutcome::DeletedMerged { branch, into } => {
            (Some(branch), "deleted", Some(format!("merged into {into}")))
        }
        BranchOutcome::DeletedForced { branch } => {
            (Some(branch), "deleted", Some("--force".to_owned()))
        }
        BranchOutcome::Kept { branch, reason } => (Some(branch), "kept", Some(reason.to_string())),
        BranchOutcome::Detached => (None, "none", None),
    };
    RemovedBranchView {
        name: name.map(Branch::as_str),
        status,
        reason,
    }
}

/// What `agent start` did, as one JSON document.
pub(crate) fn started(started: &Started) -> serde_json::Result<String> {
    let (status, pid, resumed) = match &started.outcome {
        StartOutcome::Started { pid, resumed } => (
            "started",
            Some(*pid),
            resumed.as_ref().map(SessionId::as_str),
        ),
        StartOutcome::AlreadyRunning { pid } => ("already_running", *pid, None),
    };
    document(StartedView {
        worktree: worktree_view(&started.worktree),
        agent: started.agent.name(),
        holder: HolderView { status, pid },
        writable: started.writable.as_ref().map(|writable| WritableView {
            paths: writable.value.iter().map(PathBuf::as_path).collect(),
            source: writable.source.as_deref(),
        }),
        resumed,
    })
}

/// What `agent stop` did, as one JSON document.
pub(crate) fn stopped(stopped: &Stopped) -> serde_json::Result<String> {
    document(StoppedView {
        worktree: worktree_view(&stopped.worktree),
        status: match stopped.outcome {
            StopOutcome::Stopped => "stopped",
            StopOutcome::NotRunning => "not_running",
        },
    })
}

/// What `worktree seen` or `worktree unseen` left, as one JSON document.
pub(crate) fn marked(marked: &Marked) -> serde_json::Result<String> {
    document(MarkedView {
        name: &marked.name,
        path: &marked.path,
        unseen: marked.unseen,
    })
}

/// What `worktree review` found, as one JSON document.
pub(crate) fn reviewed(reviewed: &Reviewed) -> serde_json::Result<String> {
    document(ReviewedView {
        worktree: worktree_view(&reviewed.worktree),
        base: reviewed.base.as_str(),
        against: reviewed.against.to_string(),
    })
}

/// What `worktree detail` found, as one JSON document.
pub(crate) fn detail(detail: &Detail) -> serde_json::Result<String> {
    document(DetailView {
        worktree: worktree_view(&detail.worktree),
        detail: detail.why.as_deref(),
        denials: detail
            .denials
            .iter()
            .map(|denial| DenialView {
                tool: &denial.tool,
                text: &denial.text,
            })
            .collect(),
        setup: detail.setup.as_ref().map(|setup| SetupDetailView {
            status: setup_progress_name(setup.progress),
            log: &setup.log,
            tail: &setup.tail,
        }),
        drift: detail.drift.as_ref().map(|drift| DriftView {
            against: drift.against.as_str(),
            ahead: drift.ahead,
            behind: drift.behind,
        }),
    })
}

/// How the contract names where a setup is.
fn setup_progress_name(progress: SetupProgress) -> &'static str {
    match progress {
        SetupProgress::Running => "running",
        SetupProgress::Interrupted => "interrupted",
        SetupProgress::Succeeded => "succeeded",
        SetupProgress::Failed(_) => "failed",
        SetupProgress::NotApproved => "not_approved",
    }
}

/// What `note add` or `note edit` left, as one JSON document.
pub(crate) fn noted(noted: &Noted) -> serde_json::Result<String> {
    document(NotedView {
        name: &noted.name,
        path: &noted.path,
        note: note_view(&noted.note),
    })
}

/// What `note ls` found, as one JSON document.
pub(crate) fn listed(listed: &Listed) -> serde_json::Result<String> {
    document(ListedView {
        name: &listed.name,
        path: &listed.path,
        notes: listed.notes.iter().map(note_view).collect(),
    })
}

/// The turns of a worktree, as one JSON document.
pub(crate) fn turns(history: &History) -> serde_json::Result<String> {
    document(TurnsView {
        name: &history.name,
        path: &history.path,
        turns: history.turns.iter().map(turn_view).collect(),
    })
}

fn turn_view(turn: &LoggedTurn) -> TurnView<'_> {
    let (outcome, phase) = match turn.progress {
        Progress::Ended(outcome) => (Some(outcome_name(outcome)), None),
        Progress::Open(phase) => (None, Some(phase_name(phase))),
        Progress::Unended => (None, None),
    };
    TurnView {
        number: turn.number,
        agent: turn.agent.as_deref(),
        session: turn.session.as_deref(),
        prompt: &turn.prompt,
        started_at: turn.started_at,
        ended_at: turn.ended_at,
        outcome,
        phase,
        end_observed: turn.end_observed,
        begin: turn.begin.as_deref(),
        end: turn.end.as_deref(),
        denials: turn.denials,
    }
}

/// How the contract names how a turn ended.
fn outcome_name(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Finished => "finished",
        Outcome::Failed => "failed",
        Outcome::Interrupted => "interrupted",
    }
}

/// How the contract names what the open turn's agent is doing.
fn phase_name(phase: Phase) -> &'static str {
    match phase {
        Phase::Working => "working",
        Phase::Waiting => "waiting",
    }
}

/// What a turn changed, as one JSON document.
pub(crate) fn turn_diff(diff: &TurnDiff) -> serde_json::Result<String> {
    document(TurnDiffView {
        name: &diff.name,
        path: &diff.path,
        turn: diff.turn,
        begin: &diff.bounds.begin,
        end: &diff.bounds.end,
        end_observed: diff.end_observed,
        patch: String::from_utf8_lossy(&diff.patch).into_owned(),
    })
}

/// Who wrote each line of a file, as one JSON document.
pub(crate) fn blame(blame: &Blame) -> serde_json::Result<String> {
    document(BlameView {
        name: &blame.name,
        path: &blame.path,
        file: &blame.file,
        lines: blame
            .lines
            .iter()
            .map(|author| {
                let (source, turn, agent) = match &author.source {
                    Source::Turn { number, agent } => ("turn", Some(*number), agent.as_deref()),
                    Source::BetweenTurns => ("between_turns", None, None),
                    Source::SinceSnapshot => ("since_snapshot", None, None),
                };
                LineAuthorView {
                    line: author.line,
                    source,
                    turn,
                    agent,
                    snapshot: author.snapshot.as_deref(),
                    text: &author.text,
                }
            })
            .collect(),
    })
}

/// What `note rm` removed, as one JSON document.
pub(crate) fn unnoted(unnoted: &Unnoted) -> serde_json::Result<String> {
    document(UnnotedView {
        name: &unnoted.name,
        path: &unnoted.path,
        id: unnoted.note.id,
    })
}

/// What `note send` pasted, as one JSON document.
pub(crate) fn sent(sent: &NotesSent) -> serde_json::Result<String> {
    document(SentView {
        name: &sent.name,
        path: &sent.path,
        prompt: &sent.prompt,
        sent: &sent.sent,
        kept: &sent.kept,
    })
}

fn note_view(pending: &PendingNote) -> NoteView<'_> {
    let note: &Note = &pending.note;
    NoteView {
        id: note.id,
        path: note.path.as_str(),
        anchor: anchor_view(note.anchor),
        current: anchor_view(pending.current),
        body: &note.body,
        rev: note.rev,
        created: note.created,
    }
}

fn anchor_view(anchor: Anchor) -> AnchorView {
    match anchor {
        Anchor::Line(line) => AnchorView::Line { line },
        Anchor::Lines(range) => AnchorView::Lines {
            start: range.start(),
            end: range.end(),
        },
        Anchor::File => AnchorView::File,
    }
}

/// A command that did its job and has nothing to report, as one JSON document.
pub(crate) fn done() -> serde_json::Result<String> {
    document(DoneView {})
}

/// A failure as one JSON document, with the kind the client tells failures apart by.
pub(crate) fn error(err: &anyhow::Error) -> serde_json::Result<String> {
    document(ErrorView {
        error: error_body(err),
    })
}

/// The error object of `err`: its `kind` and `message`, and for `untrusted` `what` was not
/// approved, the setup `script` or the `writable` paths, and their `source`.
fn error_body(err: &anyhow::Error) -> ErrorBody<'_> {
    let mut body = ErrorBody {
        kind: error::kind(err),
        message: format!("{err:#}"),
        what: None,
        script: None,
        writable: None,
        source: None,
    };
    if let Some(TypedError::Untrusted { subject, file, .. }) = error::typed(err) {
        body.what = Some(subject.kind().name());
        body.source = Some(file.as_path());
        match subject {
            Subject::Setup { script } => body.script = Some(script.as_str()),
            Subject::Sandbox { writable } => body.writable = Some(writable.as_slice()),
        }
    }
    body
}

/// The doctor's report as one JSON document, with `refusal` as its `error` when there is one.
pub(crate) fn doctor(
    report: &Report,
    refusal: Option<&anyhow::Error>,
) -> serde_json::Result<String> {
    document(DoctorView {
        checks: report
            .checks
            .iter()
            .map(|check| CheckView {
                name: check.name,
                status: check.status.name(),
                found: check.found.as_deref(),
                need: check.need.as_deref(),
                message: &check.message,
            })
            .collect(),
        error: refusal.map(error_body),
    })
}

fn primary_view(primary: &PrimaryCheckout) -> PrimaryView<'_> {
    PrimaryView {
        path: &primary.path,
        branch: primary.branch.as_ref().map(Branch::as_str),
    }
}

fn worktree_view(worktree: &Worktree) -> WorktreeView<'_> {
    WorktreeView {
        name: &worktree.name,
        path: &worktree.path,
        branch: worktree.branch.as_ref().map(Branch::as_str),
        state: state_name(&worktree.state),
        detail: worktree.state.detail(),
        activity: worktree.activity.as_deref(),
        agent: worktree.agent.as_deref(),
        holder: worktree.holder.is_alive(),
        unseen: worktree.unseen,
        notes: worktree.notes,
        denials: worktree.denials,
    }
}

/// How the contract names a state.
fn state_name(state: &State) -> &'static str {
    match state {
        State::NoAgent => "no_agent",
        State::Idle => "idle",
        State::Working => "working",
        State::Waiting => "waiting",
        State::Finished => "finished",
        State::Failed => "failed",
        State::Interrupted => "interrupted",
        State::SettingUp => "setting_up",
        State::NotReady(_) => "not_ready",
        State::Broken(_) => "broken",
    }
}

fn setup_view(setup: &SetupReport) -> SetupView<'_> {
    SetupView {
        status: match setup.outcome {
            SetupOutcome::NoScript => "none",
            SetupOutcome::Succeeded => "succeeded",
            SetupOutcome::Failed(_) => "failed",
        },
        log: setup.log.as_deref(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::domain::agent::Agent;
    use crate::domain::branch::StartPoint;
    use crate::domain::config::Setting;
    use crate::domain::detail::{Drift, SetupDetail};
    use crate::domain::error::{BusyReason, TypedError};
    use crate::domain::fleet::WorktreeEntry;
    use crate::domain::note::{LineRange, NotePath};
    use crate::domain::review::Commit;
    use crate::domain::setup::Exit;
    use crate::domain::state::{BrokenReason, NotReadyReason};
    use crate::domain::turn::Denial;
    use crate::domain::worktree::KeptReason;

    #[test]
    fn the_fleet_document_keeps_its_shape() {
        let mut listed = Fleet::from_entries(vec![
            WorktreeEntry {
                path: PathBuf::from("/repo"),
                branch: Some(Branch::new("main")),
                prunable: None,
            },
            WorktreeEntry {
                path: PathBuf::from("/l/docs"),
                branch: None,
                prunable: Some("gone".to_owned()),
            },
        ])
        .unwrap();
        listed.worktrees[0].state = State::Broken(BrokenReason::DirectoryMissing);
        assert_eq!(
            fleet(&listed).unwrap(),
            format!(
                r#"{{"contract":1,"version":"{VERSION}","primary":{{"path":"/repo","branch":"main"}},"worktrees":[{{"name":"docs","path":"/l/docs","branch":null,"state":"broken","detail":"directory missing; run orca-term worktree prune","activity":null,"agent":null,"holder":false,"unseen":false,"notes":0,"denials":0}}]}}"#
            )
        );
        listed.worktrees[0].unseen = true;
        let doc: serde_json::Value = serde_json::from_str(&fleet(&listed).unwrap()).unwrap();
        assert_eq!(doc["worktrees"][0]["unseen"], true);
    }

    #[test]
    fn setup_states_are_named_in_snake_case_with_their_detail() {
        let failed = State::NotReady(NotReadyReason::Failed {
            exit: Exit::Code(1),
            last_line: Some("boom".to_owned()),
        });
        let worktree = Worktree {
            state: failed,
            ..idle_worktree()
        };
        let view = worktree_view(&worktree);
        assert_eq!(view.state, "not_ready");
        assert_eq!(view.detail.as_deref(), Some("setup failed (exit 1): boom"));
        assert_eq!(state_name(&State::SettingUp), "setting_up");
        assert_eq!(state_name(&State::Idle), "idle");
    }

    #[test]
    fn the_agent_states_are_named_and_carry_their_activity() {
        for (state, name) in [
            (State::Working, "working"),
            (State::Waiting, "waiting"),
            (State::Finished, "finished"),
            (State::Failed, "failed"),
            (State::Interrupted, "interrupted"),
        ] {
            assert_eq!(state_name(&state), name);
        }
        let worktree = Worktree {
            state: State::Working,
            activity: Some("Bash: npm test".to_owned()),
            ..idle_worktree()
        };
        let mut fleet = Fleet::from_entries(vec![WorktreeEntry {
            path: PathBuf::from("/repo"),
            branch: Some(Branch::new("main")),
            prunable: None,
        }])
        .unwrap();
        fleet.worktrees.push(worktree);
        assert_eq!(
            super::fleet(&fleet).unwrap(),
            format!(
                r#"{{"contract":1,"version":"{VERSION}","primary":{{"path":"/repo","branch":"main"}},"worktrees":[{{"name":"fix","path":"/l/fix","branch":"fix","state":"working","detail":null,"activity":"Bash: npm test","agent":"claude-code","holder":true,"unseen":false,"notes":0,"denials":0}}]}}"#
            )
        );
    }

    fn idle_worktree() -> Worktree {
        Worktree {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
            branch: Some(Branch::new("fix")),
            state: State::Idle,
            activity: None,
            agent: Some("claude-code".to_owned()),
            holder: HolderStatus::Alive,
            unseen: false,
            notes: 0,
            denials: 0,
        }
    }

    #[test]
    fn the_start_document_keeps_its_shape() {
        let doc = started(&Started {
            worktree: idle_worktree(),
            agent: Agent::ClaudeCode,
            outcome: StartOutcome::Started {
                pid: 4242,
                resumed: None,
            },
            writable: Some(Setting {
                value: vec![PathBuf::from("/home/me/.cache")],
                source: Some(".git/orca-term.yaml".to_owned()),
                overridden: Vec::new(),
            }),
        })
        .unwrap();
        assert_eq!(
            doc,
            format!(
                r#"{{"contract":1,"version":"{VERSION}","worktree":{{"name":"fix","path":"/l/fix","branch":"fix","state":"idle","detail":null,"activity":null,"agent":"claude-code","holder":true,"unseen":false,"notes":0,"denials":0}},"agent":"claude-code","holder":{{"status":"started","pid":4242}},"writable":{{"paths":["/home/me/.cache"],"source":".git/orca-term.yaml"}},"resumed":null}}"#
            )
        );
        let doc: serde_json::Value = serde_json::from_str(
            &started(&Started {
                worktree: idle_worktree(),
                agent: Agent::ClaudeCode,
                outcome: StartOutcome::AlreadyRunning { pid: None },
                writable: None,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(doc["holder"]["status"], "already_running");
        assert_eq!(doc["holder"]["pid"], serde_json::Value::Null);
        assert_eq!(doc["writable"], serde_json::Value::Null);
        assert_eq!(doc["resumed"], serde_json::Value::Null);
        let doc: serde_json::Value = serde_json::from_str(
            &started(&Started {
                worktree: idle_worktree(),
                agent: Agent::ClaudeCode,
                outcome: StartOutcome::Started {
                    pid: 7,
                    resumed: SessionId::parse("0b983a87-4074-466e-b864-422ce6f063c8"),
                },
                writable: None,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(doc["resumed"], "0b983a87-4074-466e-b864-422ce6f063c8");
    }

    #[test]
    fn the_stop_document_keeps_its_shape() {
        let doc: serde_json::Value = serde_json::from_str(
            &stopped(&Stopped {
                worktree: idle_worktree(),
                outcome: StopOutcome::NotRunning,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(doc["status"], "not_running");
        assert_eq!(doc["worktree"]["state"], "idle");
        assert_eq!(doc["worktree"]["agent"], "claude-code");
    }

    #[test]
    fn the_setup_document_keeps_its_shape() {
        let done = SetupDone {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
            branch: Some(Branch::new("fix")),
            setup: SetupReport {
                outcome: SetupOutcome::Succeeded,
                script: None,
                log: Some(PathBuf::from("/a/setup.log")),
                state: State::NoAgent,
                activity: None,
                agent: None,
                unseen: true,
                notes: 0,
                denials: 0,
            },
        };
        assert_eq!(
            set_up(&done).unwrap(),
            format!(
                r#"{{"contract":1,"version":"{VERSION}","worktree":{{"name":"fix","path":"/l/fix","branch":"fix","state":"no_agent","detail":null,"activity":null,"agent":null,"holder":false,"unseen":true,"notes":0,"denials":0}},"setup":{{"status":"succeeded","log":"/a/setup.log"}}}}"#
            )
        );
    }

    #[test]
    fn the_seen_and_unseen_documents_keep_their_shape() {
        let doc = marked(&Marked {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
            unseen: true,
        })
        .unwrap();
        assert_eq!(
            doc,
            format!(
                r#"{{"contract":1,"version":"{VERSION}","name":"fix","path":"/l/fix","unseen":true}}"#
            )
        );
    }

    #[test]
    fn the_review_document_keeps_its_shape() {
        let doc = reviewed(&Reviewed {
            worktree: Worktree {
                agent: None,
                holder: HolderStatus::Gone,
                state: State::NoAgent,
                ..idle_worktree()
            },
            base: Commit::parse("1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b").unwrap(),
            against: StartPoint::Remote(Branch::new("main")),
        })
        .unwrap();
        assert_eq!(
            doc,
            format!(
                r#"{{"contract":1,"version":"{VERSION}","worktree":{{"name":"fix","path":"/l/fix","branch":"fix","state":"no_agent","detail":null,"activity":null,"agent":null,"holder":false,"unseen":false,"notes":0,"denials":0}},"base":"1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b","against":"origin/main"}}"#
            )
        );
    }

    fn pending_note(anchor: Anchor, current: Anchor) -> PendingNote {
        PendingNote {
            note: Note {
                id: 1,
                path: NotePath::parse("src/app.rs").unwrap(),
                anchor,
                body: "why \"this\"?".to_owned(),
                rev: 2,
                created: 1_700_000_000_000,
                copy: None,
            },
            current,
        }
    }

    #[test]
    fn the_note_documents_keep_their_shape() {
        let doc = noted(&Noted {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
            note: pending_note(Anchor::Line(3), Anchor::Line(5)),
        })
        .unwrap();
        assert_eq!(
            doc,
            format!(
                r#"{{"contract":1,"version":"{VERSION}","name":"fix","path":"/l/fix","note":{{"id":1,"path":"src/app.rs","anchor":{{"scope":"line","line":3}},"current":{{"scope":"line","line":5}},"body":"why \"this\"?","rev":2,"created":1700000000000}}}}"#
            )
        );
        let range = Anchor::Lines(LineRange::new(3, 5).unwrap());
        let doc: serde_json::Value = serde_json::from_str(
            &listed(&Listed {
                name: "fix".to_owned(),
                path: PathBuf::from("/l/fix"),
                notes: vec![pending_note(range, Anchor::File)],
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            doc["notes"][0]["anchor"],
            serde_json::json!({"scope": "lines", "start": 3, "end": 5})
        );
        assert_eq!(
            doc["notes"][0]["current"],
            serde_json::json!({"scope": "file"})
        );
        let doc = unnoted(&Unnoted {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
            note: pending_note(Anchor::File, Anchor::File).note,
        })
        .unwrap();
        assert_eq!(
            doc,
            format!(
                r#"{{"contract":1,"version":"{VERSION}","name":"fix","path":"/l/fix","id":1}}"#
            )
        );
    }

    #[test]
    fn the_send_document_keeps_its_shape() {
        let doc = sent(&NotesSent {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
            prompt: "File: a\nScope: file\nUser comment: \"x\"".to_owned(),
            sent: vec![1, 3],
            kept: vec![3],
        })
        .unwrap();
        assert_eq!(
            doc,
            format!(
                r#"{{"contract":1,"version":"{VERSION}","name":"fix","path":"/l/fix","prompt":"File: a\nScope: file\nUser comment: \"x\"","sent":[1,3],"kept":[3]}}"#
            )
        );
    }

    #[test]
    fn the_detail_document_keeps_its_shape() {
        let doc = detail(&Detail {
            worktree: Worktree {
                state: State::Failed,
                activity: Some("API Error: 400".to_owned()),
                denials: 1,
                ..idle_worktree()
            },
            why: Some("API Error: 400\n{}".to_owned()),
            denials: vec![Denial {
                tool: "Bash".to_owned(),
                text: "Bash: FOO=bar ls".to_owned(),
                id: Some("toolu_1".to_owned()),
            }],
            setup: Some(SetupDetail {
                progress: SetupProgress::Failed(Exit::Code(1)),
                log: PathBuf::from("/a/setup.log"),
                tail: vec!["installing".to_owned(), "error: boom".to_owned()],
            }),
            drift: Some(Drift {
                against: Branch::new("main"),
                ahead: 2,
                behind: 1,
            }),
        })
        .unwrap();
        assert_eq!(
            doc,
            format!(
                r#"{{"contract":1,"version":"{VERSION}","worktree":{{"name":"fix","path":"/l/fix","branch":"fix","state":"failed","detail":null,"activity":"API Error: 400","agent":"claude-code","holder":true,"unseen":false,"notes":0,"denials":1}},"detail":"API Error: 400\n{{}}","denials":[{{"tool":"Bash","text":"Bash: FOO=bar ls"}}],"setup":{{"status":"failed","log":"/a/setup.log","tail":["installing","error: boom"]}},"drift":{{"against":"main","ahead":2,"behind":1}}}}"#
            )
        );
        let bare: serde_json::Value = serde_json::from_str(
            &detail(&Detail {
                worktree: idle_worktree(),
                why: None,
                denials: Vec::new(),
                setup: None,
                drift: None,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(bare["detail"], serde_json::Value::Null);
        assert_eq!(bare["denials"], serde_json::json!([]));
        assert_eq!(bare["setup"], serde_json::Value::Null);
        assert_eq!(bare["drift"], serde_json::Value::Null);
        for (progress, name) in [
            (SetupProgress::Running, "running"),
            (SetupProgress::Interrupted, "interrupted"),
            (SetupProgress::Succeeded, "succeeded"),
            (SetupProgress::Failed(Exit::Signal(9)), "failed"),
            (SetupProgress::NotApproved, "not_approved"),
        ] {
            assert_eq!(setup_progress_name(progress), name);
        }
    }

    #[test]
    fn a_worktree_carries_its_denials_count() {
        let worktree = Worktree {
            denials: 4,
            ..idle_worktree()
        };
        assert_eq!(worktree_view(&worktree).denials, 4);
    }

    #[test]
    fn a_worktree_carries_its_pending_notes_count() {
        let worktree = Worktree {
            notes: 2,
            ..idle_worktree()
        };
        assert_eq!(worktree_view(&worktree).notes, 2);
    }

    #[test]
    fn the_removal_document_keeps_its_shape() {
        let doc = removed(&Removed {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
            branch: BranchOutcome::Kept {
                branch: Branch::new("fix"),
                reason: KeptReason::DefaultBranch,
            },
        })
        .unwrap();
        assert_eq!(
            doc,
            format!(
                r#"{{"contract":1,"version":"{VERSION}","name":"fix","path":"/l/fix","branch":{{"name":"fix","status":"kept","reason":"it is the default branch"}}}}"#
            )
        );
        let detached: serde_json::Value = serde_json::from_str(
            &removed(&Removed {
                name: "spike".to_owned(),
                path: PathBuf::from("/l/spike"),
                branch: BranchOutcome::Detached,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(
            detached["branch"],
            serde_json::json!({"name": null, "status": "none", "reason": null})
        );
        let outcome = BranchOutcome::DeletedForced {
            branch: Branch::new("fix"),
        };
        let forced = removed_branch_view(&outcome);
        assert_eq!(
            (forced.status, forced.reason.as_deref()),
            ("deleted", Some("--force"))
        );
    }

    #[test]
    fn an_error_carries_its_kind_and_message() {
        let busy = anyhow::Error::new(TypedError::Busy {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
            reason: BusyReason::Locked,
        });
        let doc: serde_json::Value = serde_json::from_str(&error(&busy).unwrap()).unwrap();
        assert_eq!(doc["contract"], 1);
        assert_eq!(doc["error"]["kind"], "busy");
        assert!(
            doc["error"]["message"]
                .as_str()
                .unwrap()
                .contains("is busy")
        );
        let other: serde_json::Value =
            serde_json::from_str(&error(&anyhow::anyhow!("git failed")).unwrap()).unwrap();
        assert_eq!(other["error"]["kind"], "other");
        assert_eq!(other["error"]["message"], "git failed");
        assert!(other["error"].get("script").is_none());
        assert!(other["error"].get("what").is_none());
        assert!(other["error"].get("source").is_none());
    }

    #[test]
    fn an_untrusted_sandbox_carries_the_writable_paths_and_their_source() {
        let err = anyhow::Error::new(TypedError::Untrusted {
            name: "fix".to_owned(),
            subject: Subject::Sandbox {
                writable: vec!["~/.cache".to_owned(), "../shared".to_owned()],
            },
            file: PathBuf::from("/l/fix/orca-term.yaml"),
        });
        let doc: serde_json::Value = serde_json::from_str(&error(&err).unwrap()).unwrap();
        assert_eq!(doc["error"]["kind"], "untrusted");
        assert_eq!(doc["error"]["what"], "sandbox");
        assert_eq!(
            doc["error"]["writable"],
            serde_json::json!(["~/.cache", "../shared"])
        );
        assert_eq!(doc["error"]["source"], "/l/fix/orca-term.yaml");
        assert!(doc["error"].get("script").is_none());
    }

    #[test]
    fn an_untrusted_error_carries_the_script_and_its_source() {
        let err = anyhow::Error::new(TypedError::Untrusted {
            name: "fix".to_owned(),
            subject: Subject::Setup {
                script: "make\necho done\n".to_owned(),
            },
            file: PathBuf::from("/l/fix/orca-term.yaml"),
        });
        let doc: serde_json::Value = serde_json::from_str(&error(&err).unwrap()).unwrap();
        assert_eq!(doc["error"]["kind"], "untrusted");
        assert_eq!(doc["error"]["what"], "setup");
        assert!(doc["error"].get("writable").is_none());
        assert_eq!(doc["error"]["script"], "make\necho done\n");
        assert_eq!(doc["error"]["source"], "/l/fix/orca-term.yaml");
        assert!(
            doc["error"]["message"]
                .as_str()
                .unwrap()
                .contains("is not approved in this repository")
        );
    }
}
