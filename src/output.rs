//! Edge: the only module that writes to stdout and stderr. It renders the text people read; the
//! JSON documents live in `output::json`.
//!
//! Reports and JSON documents go to stdout. Errors go to stderr, or to stdout as a JSON document
//! with `--json`. A command that may run a setup first says on stderr what it starts (`creating
//! worktree fix...`), and the setup script's output follows there live, through the writer
//! [`live_echo`] hands to the command; neither appears with `--json`.

mod json;

use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::path::PathBuf;
use std::process::ExitCode;

use crate::domain::agent::{AttachEnd, StartOutcome, Started, StopOutcome, Stopped, Swapped};
use crate::domain::blame::{Blame, LineAuthor, Source};
use crate::domain::detail::{Detail, Drift, SetupDetail, SetupProgress};
use crate::domain::error;
use crate::domain::fleet::{Fleet, Pruned, Worktree};
use crate::domain::history::{History, LoggedTurn, Progress, TurnDiff};
use crate::domain::include;
use crate::domain::note::{Listed, Noted, NotesSent, PendingNote, Unnoted};
use crate::domain::nvim::Address;
use crate::domain::operation::{Operation, RebaseStep};
use crate::domain::pr::{Checks, Found, Refreshed};
use crate::domain::prerequisite::Report;
use crate::domain::review::Reviewed;
use crate::domain::seen::Marked;
use crate::domain::session::{self, Discarded, Resumed, SessionList};
use crate::domain::setup::{SetupDone, SetupOutcome, SetupReport};
use crate::domain::trust::{Subject, Unapproved};
use crate::domain::turn::{Outcome, Phase};
use crate::domain::worktree::{BranchOutcome, Created, Removed, Target, WorktreeName};

/// How a command prints its result and its errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Text for a person.
    Text,
    /// One JSON document carrying `contract` and `version`, for the client (`--json`).
    Json,
}

impl Format {
    /// The format a command's `--json` flag asks for.
    pub(crate) fn from_json_flag(json: bool) -> Self {
        if json { Self::Json } else { Self::Text }
    }
}

/// Says on stderr that `worktree new` is creating the worktree `name`, before the setup's live
/// output, which may take minutes. Nothing with `--json`.
pub(crate) fn creating_worktree(name: &WorktreeName, format: Format) {
    progress(&format!("creating worktree {name}..."), format);
}

/// Says on stderr that `worktree setup` is running the setup of `target`, before its live output.
/// Nothing with `--json`.
pub(crate) fn running_setup(target: &Target, format: Format) {
    progress(&format!("running the setup of {target}..."), format);
}

fn progress(line: &str, format: Format) {
    if format == Format::Text {
        #[expect(clippy::print_stderr, reason = "this module owns the process's output")]
        {
            eprintln!("{line}");
        }
    }
}

/// Shows on stderr what a committed file asks for, `unapproved` (a setup script, or the sandbox's
/// writable paths), and the file, then asks `run this setup script? [y/N] ` or `widen the sandbox
/// with these writable paths? [y/N] `, leaving the cursor after the question for the answer.
pub(crate) fn ask_approval(unapproved: &Unapproved) {
    let mut stderr = io::stderr().lock();
    let _ = write!(stderr, "{}", render_approval(unapproved));
    let _ = stderr.flush();
}

fn render_approval(unapproved: &Unapproved) -> String {
    let (name, file) = (&unapproved.name, unapproved.file.display());
    let (intro, lines, question): (String, Vec<&str>, &str) = match &unapproved.subject {
        Subject::Setup { script } => (
            format!(
                "the setup script of worktree {name} comes from {file}, committed to the \
                 repository, and is not approved yet:"
            ),
            script.lines().collect(),
            "run this setup script? [y/N] ",
        ),
        Subject::Sandbox { writable } => (
            format!(
                "the sandbox's extra writable paths of worktree {name} come from {file}, \
                 committed to the repository, and are not approved yet:"
            ),
            writable.iter().map(String::as_str).collect(),
            "widen the sandbox with these writable paths? [y/N] ",
        ),
    };
    let mut text = format!("{intro}\n\n");
    for line in lines {
        let _ = writeln!(text, "    {line}");
    }
    text.push('\n');
    text.push_str(question);
    text
}

/// Where a setup script's output is echoed while it runs: stderr for a person, nowhere with
/// `--json`, whose stdout carries only the document.
pub(crate) fn live_echo(format: Format) -> Box<dyn io::Write> {
    match format {
        Format::Text => Box::new(io::stderr()),
        Format::Json => Box::new(io::sink()),
    }
}

/// Prints the fleet as one JSON document carrying `contract` and `version`.
pub(crate) fn fleet_json(fleet: &Fleet) -> anyhow::Result<()> {
    out(&json::fleet(fleet)?);
    Ok(())
}

/// Prints the fleet as an aligned table, one worktree per line.
pub(crate) fn fleet_table(fleet: &Fleet) {
    out(&render_fleet_table(fleet));
}

fn render_fleet_table(fleet: &Fleet) -> String {
    if fleet.worktrees.is_empty() {
        return format!("no worktrees in {}", fleet.primary.path.display());
    }
    let header = ["NAME", "BRANCH", "STATE", "PATH"].map(str::to_owned);
    let rows: Vec<[String; 4]> = fleet
        .worktrees
        .iter()
        .map(|worktree| {
            [
                worktree.name.clone(),
                worktree
                    .branch
                    .as_ref()
                    .map_or_else(|| "(detached)".to_owned(), ToString::to_string),
                state_cell(worktree),
                worktree.path.display().to_string(),
            ]
        })
        .collect();
    let mut lines: Vec<Vec<String>> = [header].into_iter().chain(rows).map(Vec::from).collect();
    // The agent, notes, denials and detail columns appear only when some worktree has something in
    // them.
    for (title, cells) in [
        (
            "AGENT",
            fleet.worktrees.iter().map(agent_cell).collect::<Vec<_>>(),
        ),
        ("NOTES", fleet.worktrees.iter().map(notes_cell).collect()),
        (
            "DENIALS",
            fleet.worktrees.iter().map(denials_cell).collect(),
        ),
        (
            "DETAIL",
            fleet
                .worktrees
                .iter()
                .map(|worktree| worktree.state.detail().unwrap_or_default())
                .collect(),
        ),
    ] {
        if cells.iter().any(|cell| !cell.is_empty()) {
            lines[0].push(title.to_owned());
            for (line, cell) in lines[1..].iter_mut().zip(cells) {
                line.push(cell);
            }
        }
    }
    render_columns(&lines)
}

/// The worktree's state as the table shows it, followed by `(unseen)` when its latest turn ended
/// after the person last reviewed it, or the person marked it so, and by the git operation it is
/// in the middle of: `finished (unseen)`, `idle, rebasing`.
fn state_cell(worktree: &Worktree) -> String {
    let mut cell = worktree.state.label().to_owned();
    if worktree.unseen {
        cell.push_str(" (unseen)");
    }
    if let Some(operation) = worktree.operation {
        let _ = write!(cell, ", {}", operation.verb());
    }
    cell
}

/// The detail's line for a git operation in progress, as the client's float says it too:
/// `merge in progress`, `rebase in progress (2/5)` when git records the rebase's step.
fn operation_line(operation: Operation, step: Option<RebaseStep>) -> String {
    match step {
        Some(step) => format!(
            "{} in progress ({}/{})",
            operation.name(),
            step.current,
            step.total
        ),
        None => format!("{} in progress", operation.name()),
    }
}

/// The worktree's agent and whether it runs, as the table shows it: `claude-code (running)`,
/// `claude-code (stopped)`, or nothing.
fn agent_cell(worktree: &Worktree) -> String {
    match (&worktree.agent, worktree.holder.is_alive()) {
        (None, _) => String::new(),
        (Some(agent), true) => format!("{agent} (running)"),
        (Some(agent), false) => format!("{agent} (stopped)"),
    }
}

/// How many notes are pending on the worktree, as the table shows it: the count, or nothing.
fn notes_cell(worktree: &Worktree) -> String {
    if worktree.notes == 0 {
        String::new()
    } else {
        worktree.notes.to_string()
    }
}

/// How many tool calls the worktree's latest turn had denied, as the table shows it: the count, or
/// nothing.
fn denials_cell(worktree: &Worktree) -> String {
    if worktree.denials == 0 {
        String::new()
    } else {
        worktree.denials.to_string()
    }
}

/// Lays `lines` out in columns two spaces apart. The last column is not padded.
fn render_columns(lines: &[Vec<String>]) -> String {
    let columns = lines.first().map_or(0, Vec::len);
    let widths: Vec<usize> = (0..columns)
        .map(|col| lines.iter().map(|line| line[col].len()).max().unwrap_or(0))
        .collect();
    let mut table = String::new();
    for line in lines {
        let mut text = String::new();
        for (col, cell) in line.iter().enumerate() {
            if col + 1 < columns {
                let _ = write!(text, "{cell:width$}  ", width = widths[col]);
            } else {
                text.push_str(cell);
            }
        }
        let _ = writeln!(table, "{}", text.trim_end());
    }
    table.truncate(table.trim_end().len());
    table
}

/// Prints what `worktree new` made, with the `base` it used and where that came from, and what
/// its setup did.
pub(crate) fn worktree_created(created: &Created, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&render_worktree_created(created)),
        Format::Json => out(&json::created(created)?),
    }
    Ok(())
}

fn render_worktree_created(created: &Created) -> String {
    let files = if created.copied == 1 { "file" } else { "files" };
    format!(
        "created worktree {}\n\
         path:   {}\n\
         branch: {} (from {})\n\
         copied: {} {files} from {}\n\
         base:   {}\n\
         {}",
        created.name,
        created.path.display(),
        created.branch,
        created.start,
        created.copied,
        include::FILE_NAME,
        created.base,
        render_setup(created.name.as_str(), &created.setup),
    )
}

/// Prints what `worktree setup` did.
pub(crate) fn worktree_set_up(done: &SetupDone, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&render_worktree_set_up(done)),
        Format::Json => out(&json::set_up(done)?),
    }
    Ok(())
}

fn render_worktree_set_up(done: &SetupDone) -> String {
    let readiness = if done.setup.outcome.failed() {
        "is not ready"
    } else {
        "is ready"
    };
    format!(
        "worktree {} {readiness}\n\
         path:   {}\n\
         {}",
        done.name,
        done.path.display(),
        render_setup(&done.name, &done.setup),
    )
}

/// The lines saying what the setup of the worktree `name` did: which script ran and from where,
/// where its log is and, when it failed, why the worktree is not ready and how to try again.
fn render_setup(name: &str, setup: &SetupReport) -> String {
    let from = setup
        .script
        .as_ref()
        .map(|script| format!(" (script {})", script.origin()))
        .unwrap_or_default();
    let log = setup
        .log
        .as_ref()
        .map(|log| format!("\nlog:    {}", log.display()))
        .unwrap_or_default();
    match setup.outcome {
        SetupOutcome::NoScript => {
            "setup:  nothing to run, no setup script is configured".to_owned()
        }
        SetupOutcome::Succeeded => format!("setup:  succeeded{from}{log}"),
        SetupOutcome::Failed(_) => format!(
            "setup:  failed{from}{log}\n\
             not ready: {}\n\
             fix the cause, then run `orca-term worktree setup {name}`",
            setup.state.detail().unwrap_or_default()
        ),
    }
}

/// Prints what `worktree rm` removed and what happened to the worktree's branch.
pub(crate) fn worktree_removed(removed: &Removed, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&render_worktree_removed(removed)),
        Format::Json => out(&json::removed(removed)?),
    }
    Ok(())
}

fn render_worktree_removed(removed: &Removed) -> String {
    let branch = match &removed.branch {
        BranchOutcome::DeletedMerged { branch, into } => {
            format!("deleted branch {branch} (merged into {into})")
        }
        BranchOutcome::DeletedForced { branch } => format!("deleted branch {branch} (--force)"),
        BranchOutcome::Kept { branch, reason } => format!("kept branch {branch}: {reason}"),
        BranchOutcome::Detached => "no branch to delete: the worktree was detached".to_owned(),
    };
    format!(
        "removed worktree {}\n\
         path:   {}\n\
         {branch}",
        removed.name,
        removed.path.display(),
    )
}

/// Prints what `worktree prune` cleaned, one line each: the worktrees by path, then the chains of
/// snapshots by ref; or that there was nothing to prune.
pub(crate) fn worktrees_pruned(pruned: &Pruned) {
    out(&render_worktrees_pruned(pruned));
}

fn render_worktrees_pruned(pruned: &Pruned) -> String {
    if pruned.worktrees.is_empty() && pruned.chains.is_empty() {
        return "nothing to prune".to_owned();
    }
    let worktrees = pruned
        .worktrees
        .iter()
        .map(|path| format!("pruned {}", path.display()));
    let chains = pruned.chains.iter().map(|name| format!("deleted {name}"));
    worktrees.chain(chains).collect::<Vec<_>>().join("\n")
}

/// Prints what `agent start` did and, in text, says on stderr which extra paths the sandbox may
/// write to and which config file set them.
pub(crate) fn agent_started(started: &Started, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => {
            if let Some(writable) = &started.writable {
                progress(&render_writable(writable), format);
            }
            out(&render_agent_started(started));
        }
        Format::Json => out(&json::started(started)?),
    }
    Ok(())
}

fn render_writable(writable: &crate::domain::config::Setting<Vec<PathBuf>>) -> String {
    if writable.value.is_empty() {
        return format!("sandbox: no extra writable paths ({})", writable.origin());
    }
    let paths: Vec<String> = writable
        .value
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    format!(
        "sandbox: extra writable paths {} ({})",
        paths.join(", "),
        writable.origin()
    )
}

fn render_agent_started(started: &Started) -> String {
    let worktree = &started.worktree;
    let (headline, pid) = match &started.outcome {
        StartOutcome::Started { pid, resumed } => (
            match resumed {
                Some(session) => format!(
                    "started agent {} in worktree {}, resuming session {session}",
                    started.agent, worktree.name
                ),
                None => format!(
                    "started agent {} in worktree {}",
                    started.agent, worktree.name
                ),
            },
            format!("pid {pid}"),
        ),
        StartOutcome::AlreadyRunning { pid } => (
            format!(
                "agent {} is already running in worktree {}",
                started.agent, worktree.name
            ),
            pid.map_or_else(|| "pid unknown".to_owned(), |pid| format!("pid {pid}")),
        ),
    };
    format!(
        "{headline}\n\
         path:   {}\n\
         holder: {pid}\n\
         attach: orca-term agent attach {}",
        worktree.path.display(),
        worktree.name,
    )
}

/// Prints what `agent swap` did: in text, which agent was stopped (when one was), then what
/// starting the new one did, as `agent start` prints it.
pub(crate) fn agent_swapped(swapped: &Swapped, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => {
            if let Some(writable) = &swapped.started.writable {
                progress(&render_writable(writable), format);
            }
            out(&render_agent_swapped(swapped));
        }
        Format::Json => out(&json::swapped(swapped)?),
    }
    Ok(())
}

fn render_agent_swapped(swapped: &Swapped) -> String {
    let started = render_agent_started(&swapped.started);
    match (swapped.stopped, swapped.previous) {
        (true, Some(previous)) => format!(
            "stopped agent {previous} in worktree {}\n{started}",
            swapped.started.worktree.name
        ),
        _ => started,
    }
}

/// The characters of a session's first prompt `session ls` shows.
const SESSION_PROMPT_CHARS: usize = 60;

/// Prints a worktree's sessions (#41): as a table like `turn ls`'s, the most recently active
/// first, the current one marked `*`; or one JSON document.
pub(crate) fn sessions_listed(list: &SessionList, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&render_sessions_listed(list)),
        Format::Json => out(&json::sessions(list)?),
    }
    Ok(())
}

fn render_sessions_listed(list: &SessionList) -> String {
    if list.sessions.is_empty() {
        return format!("no sessions in worktree {}", list.worktree.name);
    }
    let header = [
        "",
        "SESSION",
        "AGENT",
        "TURNS",
        "LAST ACTIVITY",
        "FIRST PROMPT",
    ]
    .map(str::to_owned)
    .to_vec();
    let rows = list.sessions.iter().map(|session| {
        vec![
            if session.current { "*" } else { "" }.to_owned(),
            session.id.clone(),
            session
                .agent()
                .map_or_else(|| session.agent.clone(), |agent| agent.name().to_owned()),
            session.turns.to_string(),
            session
                .last_activity
                .map_or_else(|| "-".to_owned(), |then| session::age(list.now, then)),
            session
                .first_prompt
                .as_deref()
                .map_or_else(|| "-".to_owned(), cut_prompt),
        ]
    });
    render_columns(&[header].into_iter().chain(rows).collect::<Vec<_>>())
}

/// `prompt` on one line, cut to [`SESSION_PROMPT_CHARS`] characters with `...`.
fn cut_prompt(prompt: &str) -> String {
    let flat = prompt.replace('\n', " ");
    if flat.chars().count() <= SESSION_PROMPT_CHARS {
        return flat;
    }
    let kept: String = flat.chars().take(SESSION_PROMPT_CHARS - 3).collect();
    format!("{kept}...")
}

/// Prints what `session resume` did (#41): in text, that the session was already current when its
/// agent was running on it, then what `agent swap` prints.
pub(crate) fn session_resumed(resumed: &Resumed, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => {
            if let Some(writable) = &resumed.swapped.started.writable {
                progress(&render_writable(writable), format);
            }
            out(&render_session_resumed(resumed));
        }
        Format::Json => out(&json::resumed(resumed)?),
    }
    Ok(())
}

fn render_session_resumed(resumed: &Resumed) -> String {
    let swapped = render_agent_swapped(&resumed.swapped);
    match resumed.swapped.started.outcome {
        StartOutcome::AlreadyRunning { .. } => format!(
            "session {} is already current in worktree {}\n{swapped}",
            resumed.session, resumed.swapped.started.worktree.name
        ),
        StartOutcome::Started { .. } => swapped,
    }
}

/// Prints what `session new` did (#41): the session discarded and how to bring it back, then what
/// starting the agent again on a new session did, or that its next start begins one.
pub(crate) fn session_discarded(discarded: &Discarded, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => {
            if let Some(writable) = discarded.started.as_ref().and_then(|s| s.writable.as_ref()) {
                progress(&render_writable(writable), format);
            }
            out(&render_session_discarded(discarded));
        }
        Format::Json => out(&json::discarded(discarded)?),
    }
    Ok(())
}

fn render_session_discarded(discarded: &Discarded) -> String {
    let name = &discarded.worktree.name;
    let headline = match &discarded.session {
        Some(id) => format!(
            "discarded session {id} of worktree {name}; `orca-term session resume {name} {id}` \
             brings it back"
        ),
        None => format!("worktree {name} had no current session"),
    };
    match &discarded.started {
        Some(started) => format!("{headline}\n{}", render_agent_started(started)),
        None => format!("{headline}\nthe next start of its agent begins a new session"),
    }
}

/// Prints what `agent stop` did.
pub(crate) fn agent_stopped(stopped: &Stopped, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&render_agent_stopped(stopped)),
        Format::Json => out(&json::stopped(stopped)?),
    }
    Ok(())
}

fn render_agent_stopped(stopped: &Stopped) -> String {
    let name = &stopped.worktree.name;
    match stopped.outcome {
        StopOutcome::Stopped => format!("stopped the agent of worktree {name}"),
        StopOutcome::NotRunning => format!("no agent is running in worktree {name}"),
    }
}

/// Prints what `worktree seen` or `worktree unseen` left: one line, `fix: seen` or `fix: unseen`.
pub(crate) fn worktree_marked(marked: &Marked, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&render_worktree_marked(marked)),
        Format::Json => out(&json::marked(marked)?),
    }
    Ok(())
}

fn render_worktree_marked(marked: &Marked) -> String {
    let mark = if marked.unseen { "unseen" } else { "seen" };
    format!("{}: {mark}", marked.name)
}

/// Prints what `pr refresh` found, one line per worktree:
///
/// ```text
/// fix: PR #12 open, checks pending
/// docs: no PR
/// spike: PR unavailable: GitHub CLI is unavailable
/// ```
pub(crate) fn prs_refreshed(refreshed: &Refreshed, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text if refreshed.worktrees.is_empty() => {}
        Format::Text => out(&render_prs_refreshed(refreshed)),
        Format::Json => out(&json::refreshed(refreshed)?),
    }
    Ok(())
}

fn render_prs_refreshed(refreshed: &Refreshed) -> String {
    let lines: Vec<String> = refreshed
        .worktrees
        .iter()
        .map(|worktree| format!("{}: {}", worktree.name, pr_text(&worktree.facts.found)))
        .collect();
    lines.join("\n")
}

/// What was found about a pull request, in a few words: `PR #12 open, checks passing`, `no PR`
/// or `PR unavailable: <reason>`.
fn pr_text(found: &Found) -> String {
    match found {
        Found::Pr(pr) => {
            let checks = match pr.checks {
                Checks::None => "no checks".to_owned(),
                checks => format!("checks {}", checks.name()),
            };
            format!("PR #{} {}, {checks}", pr.number, pr.state.name())
        }
        Found::NoPr => "no PR".to_owned(),
        Found::Unavailable(reason) => format!("PR unavailable: {reason}"),
    }
}

/// Prints what `worktree detail` found, as a block of lines: the worktree and its state, then
/// the whole detail indented, the denials of the latest turn, the setup with the end of its log,
/// and the drift, each part only when it has something to say:
///
/// ```text
/// fix: failed
///   API Error: 400 ...
/// denied 1 tool call in the latest turn:
///   Bash: FOO=bar ls
/// setup succeeded (log /r/.git/worktrees/fix/orca-term/setup.log):
///   done
/// 2 ahead of main, 1 behind
/// ```
pub(crate) fn worktree_detail(detail: &Detail, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&render_worktree_detail(detail)),
        Format::Json => out(&json::detail(detail)?),
    }
    Ok(())
}

fn render_worktree_detail(detail: &Detail) -> String {
    let mut text = format!(
        "{}: {}\n",
        detail.worktree.name,
        state_cell(&detail.worktree)
    );
    for line in detail.why.iter().flat_map(|whole| whole.lines()) {
        let _ = writeln!(text, "  {line}");
    }
    if let Some(operation) = detail.worktree.operation {
        let _ = writeln!(text, "{}", operation_line(operation, detail.operation_step));
    }
    if !detail.denials.is_empty() {
        let count = detail.denials.len();
        let calls = if count == 1 { "call" } else { "calls" };
        let _ = writeln!(text, "denied {count} tool {calls} in the latest turn:");
        for denial in &detail.denials {
            let _ = writeln!(text, "  {}", denial.text);
        }
    }
    if let Some(setup) = &detail.setup {
        text.push_str(&render_setup_detail(setup));
    }
    text.push_str(&render_drift(detail.drift.as_ref()));
    text
}

/// The setup's line, `setup failed (exit 1) (log <path>):`, and the end of its log, indented.
fn render_setup_detail(setup: &SetupDetail) -> String {
    let progress = match setup.progress {
        SetupProgress::Running => "setup running".to_owned(),
        SetupProgress::Interrupted => "setup interrupted".to_owned(),
        SetupProgress::Succeeded => "setup succeeded".to_owned(),
        SetupProgress::Failed(exit) => exit.to_string(),
        SetupProgress::NotApproved => "setup not approved".to_owned(),
    };
    let mut text = format!("{progress} (log {})", setup.log.display());
    text.push_str(if setup.tail.is_empty() { "\n" } else { ":\n" });
    for line in &setup.tail {
        let _ = writeln!(text, "  {line}");
    }
    text
}

/// The drift's line: `2 ahead of main, 1 behind`, or why there is none.
fn render_drift(drift: Option<&Drift>) -> String {
    match drift {
        Some(drift) => format!(
            "{} ahead of {}, {} behind",
            drift.ahead, drift.against, drift.behind
        ),
        None => {
            "no drift to show: no local default branch, or a history unrelated to it".to_owned()
        }
    }
}

/// Prints what `worktree review` found: one line naming the default branch and the base's short
/// id, `fix: review against main (1a2b3c4)`.
pub(crate) fn worktree_reviewed(reviewed: &Reviewed, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&render_worktree_reviewed(reviewed)),
        Format::Json => out(&json::reviewed(reviewed)?),
    }
    Ok(())
}

fn render_worktree_reviewed(reviewed: &Reviewed) -> String {
    format!(
        "{}: review against {} ({})",
        reviewed.worktree.name,
        reviewed.against,
        reviewed.base.short()
    )
}

/// Prints what `note add` or `note edit` left: one line naming the note, its file and its lines,
/// `fix: note 1 on src/app.rs, line 3`.
pub(crate) fn noted(noted: &Noted, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&format!(
            "{}: note {}",
            noted.name,
            render_note_place(&noted.note)
        )),
        Format::Json => out(&json::noted(noted)?),
    }
    Ok(())
}

/// Prints what `note ls` found: one line per pending note, its id, file, lines and what it says.
pub(crate) fn notes_listed(listed: &Listed, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&render_notes_listed(listed)),
        Format::Json => out(&json::listed(listed)?),
    }
    Ok(())
}

fn render_notes_listed(listed: &Listed) -> String {
    if listed.notes.is_empty() {
        return format!("no pending notes in worktree {}", listed.name);
    }
    let header = ["ID", "FILE", "LINES", "NOTE"].map(str::to_owned).to_vec();
    let rows = listed.notes.iter().map(|pending| {
        vec![
            pending.note.id.to_string(),
            pending.note.path.to_string(),
            render_anchor(pending),
            pending.note.body.replace('\n', " "),
        ]
    });
    render_columns(&[header].into_iter().chain(rows).collect::<Vec<_>>())
}

/// `1 on src/app.rs, line 3`: the note's id, file and lines.
fn render_note_place(pending: &PendingNote) -> String {
    format!(
        "{} on {}, {}",
        pending.note.id,
        pending.note.path,
        render_anchor(pending)
    )
}

/// The note's lines as made, followed by where they are now when they moved: `line 3`,
/// `line 3 (now line 5)`, `lines 2-4 (now the whole file)`.
fn render_anchor(pending: &PendingNote) -> String {
    let made = pending.note.anchor;
    if pending.current == made {
        made.to_string()
    } else {
        format!("{made} (now {})", pending.current)
    }
}

/// Prints what `note rm` removed: one line, `fix: removed note 1`.
pub(crate) fn unnoted(unnoted: &Unnoted, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&format!(
            "{}: removed note {}",
            unnoted.name, unnoted.note.id
        )),
        Format::Json => out(&json::unnoted(unnoted)?),
    }
    Ok(())
}

/// Prints what `note send` pasted: the prompt, then one line naming the notes sent and those kept
/// because they were edited while being sent.
pub(crate) fn notes_sent(sent: &NotesSent, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&render_notes_sent(sent)),
        Format::Json => out(&json::sent(sent)?),
    }
    Ok(())
}

fn render_notes_sent(sent: &NotesSent) -> String {
    let ids = |ids: &[u64]| {
        ids.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut text = format!(
        "{}\n\n{}: sent notes {}",
        sent.prompt,
        sent.name,
        ids(&sent.sent)
    );
    if !sent.kept.is_empty() {
        let _ = write!(
            text,
            "; kept {} pending, edited while being sent",
            ids(&sent.kept)
        );
    }
    text
}

/// Prints that the Neovim at `address` is registered: one line, or with `--json` a document
/// carrying only `contract` and `version`.
pub(crate) fn nvim_registered(address: &Address, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&format!("registered Neovim {address}")),
        Format::Json => out(&json::done()?),
    }
    Ok(())
}

/// Prints that the Neovim at `address` is no longer registered, as [`nvim_registered`] does.
pub(crate) fn nvim_unregistered(address: &Address, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&format!("unregistered Neovim {address}")),
        Format::Json => out(&json::done()?),
    }
    Ok(())
}

/// Says on stderr how `agent attach` ended, once the terminal is the person's again.
pub(crate) fn attach_ended(end: &AttachEnd) {
    let line = match end {
        AttachEnd::Detached => "[detached]".to_owned(),
        AttachEnd::AgentEnded(text) => format!("[agent ended: {text}]"),
    };
    progress(&line, Format::Text);
}

/// Prints the doctor's report: one line per check, or one JSON document. When a check is missing
/// or too old, the whole report is printed first, then the `prerequisite` error as any command
/// prints it (on stderr as text, inside the same document with `--json`), and the exit code is
/// that error's.
pub(crate) fn doctor(report: &Report, format: Format) -> anyhow::Result<ExitCode> {
    let refusal = report.refusal().map(anyhow::Error::new);
    match format {
        Format::Text => {
            out(&render_doctor(report));
            if let Some(err) = &refusal {
                return Ok(error(err, Format::Text));
            }
        }
        Format::Json => out(&json::doctor(report, refusal.as_ref())?),
    }
    Ok(refusal.map_or(ExitCode::SUCCESS, |err| {
        ExitCode::from(error::exit_code(&err))
    }))
}

fn render_doctor(report: &Report) -> String {
    report
        .checks
        .iter()
        .map(|check| format!("{:<8} {}", check.status.label(), check.message))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Prints an error, with its causes: to stderr as text, or to stdout as a JSON document carrying
/// its `kind`. Returns the exit code the error maps to: its own for a typed error, 1 otherwise.
pub fn error(err: &anyhow::Error, format: Format) -> ExitCode {
    let document = match format {
        Format::Json => json::error(err).ok(),
        Format::Text => None,
    };
    match document {
        Some(document) => out(&document),
        None => {
            #[expect(clippy::print_stderr, reason = "this module owns the process's output")]
            {
                eprintln!("orca-term: {err:#}");
            }
        }
    }
    ExitCode::from(error::exit_code(err))
}

/// The characters of a snapshot's id a table shows.
const SHORT_ID: usize = 7;

/// The width of the turn column of `turn blame`.
const BLAME_TURN_WIDTH: usize = 4;

/// What `turn blame` shows for a line written between turns, and for one changed since the latest
/// snapshot.
const BETWEEN_TURNS_MARK: &str = "-";
const SINCE_SNAPSHOT_MARK: &str = "·";

/// Prints the turns of a worktree: a table, oldest first, or one JSON document.
pub(crate) fn turns_listed(history: &History, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&render_turns_listed(history)),
        Format::Json => out(&json::turns(history)?),
    }
    Ok(())
}

fn render_turns_listed(history: &History) -> String {
    if history.turns.is_empty() {
        return format!("no turns in worktree {}", history.name);
    }
    let header = ["TURN", "OUTCOME", "AGENT", "BEGIN", "END", "PROMPT"]
        .map(str::to_owned)
        .to_vec();
    let rows = history.turns.iter().map(|turn| {
        vec![
            turn.number.to_string(),
            progress_cell(turn),
            turn.agent.clone().unwrap_or_else(|| "-".to_owned()),
            short_id(turn.begin.as_deref()),
            short_id(turn.end.as_deref()),
            turn.prompt.replace('\n', " "),
        ]
    });
    render_columns(&[header].into_iter().chain(rows).collect::<Vec<_>>())
}

/// Where a turn is, as `turn ls` shows it: its outcome, `working` or `waiting` while open, `-`
/// when its end was never logged.
fn progress_cell(turn: &LoggedTurn) -> String {
    let label = match turn.progress {
        Progress::Ended(Outcome::Finished) => "finished",
        Progress::Ended(Outcome::Failed) => "failed",
        Progress::Ended(Outcome::Interrupted) => "interrupted",
        Progress::Open(Phase::Working) => "working",
        Progress::Open(Phase::Waiting) => "waiting",
        Progress::Unended => "-",
    };
    label.to_owned()
}

/// A snapshot's id cut to its first characters, `-` when there is none.
fn short_id(id: Option<&str>) -> String {
    id.map_or_else(|| "-".to_owned(), |id| id.chars().take(SHORT_ID).collect())
}

/// Prints what a turn changed: the patch exactly as git printed it on stdout, and on stderr a
/// warning when its end was not observed, or that it changed nothing; or one JSON document.
pub(crate) fn turn_diff(diff: &TurnDiff, format: Format) -> anyhow::Result<()> {
    if format == Format::Json {
        out(&json::turn_diff(diff)?);
        return Ok(());
    }
    if diff.end_observed == Some(false) {
        progress(&end_not_observed(diff.turn), format);
    }
    if diff.patch.is_empty() {
        progress(&format!("turn {} changed no file", diff.turn), format);
        return Ok(());
    }
    io::stdout().lock().write_all(&diff.patch)?;
    Ok(())
}

/// The warning `turn diff` gives for a turn whose end was not observed.
fn end_not_observed(turn: u64) -> String {
    format!(
        "turn {turn}: its end was not observed (it was interrupted), so this diff may include \
         edits made after the interruption"
    )
}

/// Prints who wrote each line of a file: one line per line of the file, or one JSON document.
pub(crate) fn turn_blame(blame: &Blame, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&render_turn_blame(blame)),
        Format::Json => out(&json::blame(blame)?),
    }
    Ok(())
}

fn render_turn_blame(blame: &Blame) -> String {
    if blame.lines.is_empty() {
        return format!("{} is empty", blame.file);
    }
    let width = blame.lines.len().to_string().len().max(BLAME_TURN_WIDTH);
    let mut text = String::new();
    for author in &blame.lines {
        let _ = writeln!(
            text,
            "{:>BLAME_TURN_WIDTH$}  {:>width$}  {}",
            blame_mark(author),
            author.line,
            author.text
        );
    }
    text.truncate(text.trim_end_matches('\n').len());
    text
}

/// Who wrote a line, as `turn blame` shows it: the turn's number, `-` for no turn, `·` for a line
/// changed since the latest snapshot.
fn blame_mark(author: &LineAuthor) -> String {
    match &author.source {
        Source::Turn { number, .. } => number.to_string(),
        Source::BetweenTurns => BETWEEN_TURNS_MARK.to_owned(),
        Source::SinceSnapshot => SINCE_SNAPSHOT_MARK.to_owned(),
    }
}

fn out(text: &str) {
    #[expect(clippy::print_stdout, reason = "this module owns the process's output")]
    {
        println!("{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::agent::{Agent, HolderStatus};
    use crate::domain::branch::{Branch, StartPoint};
    use crate::domain::config::Setting;
    use crate::domain::fleet::PrimaryCheckout;
    use crate::domain::launch::SessionId;
    use crate::domain::note::{Anchor, LineRange, Note, NotePath};
    use crate::domain::review::Commit;
    use crate::domain::setup::Exit;
    use crate::domain::state::{BrokenReason, NotReadyReason, State};
    use crate::domain::turn::Denial;
    use crate::domain::worktree::KeptReason;

    fn no_setup() -> SetupReport {
        SetupReport {
            outcome: SetupOutcome::NoScript,
            script: None,
            log: None,
            state: State::NoAgent,
            activity: None,
            agent: None,
            unseen: false,
            notes: 0,
            denials: 0,
            operation: None,
        }
    }

    fn worktree(name: &str, state: State) -> Worktree {
        Worktree {
            name: name.to_owned(),
            path: PathBuf::from(format!("/l/{name}")),
            branch: Some(Branch::new(name)),
            state,
            activity: None,
            agent: None,
            holder: HolderStatus::Gone,
            unseen: false,
            notes: 0,
            denials: 0,
            operation: None,
            pr: None,
        }
    }

    #[test]
    fn a_refresh_says_what_it_found_for_each_worktree_on_its_own_line() {
        use crate::domain::pr::{PrFacts, PrState, PullRequest, RefreshedWorktree};
        let refreshed = |name: &str, found: Found| RefreshedWorktree {
            name: name.to_owned(),
            path: PathBuf::from(format!("/l/{name}")),
            facts: PrFacts {
                fetched_at: 1,
                found,
            },
        };
        let pr = |state, checks| {
            Found::Pr(PullRequest {
                number: 12,
                title: "Fix".to_owned(),
                state,
                url: "https://github.com/o/r/pull/12".to_owned(),
                checks,
            })
        };
        let all = Refreshed {
            worktrees: vec![
                refreshed("fix", pr(PrState::Open, Checks::Pending)),
                refreshed("old", pr(PrState::Merged, Checks::None)),
                refreshed("docs", Found::NoPr),
                refreshed(
                    "spike",
                    Found::Unavailable("GitHub CLI is unavailable".to_owned()),
                ),
            ],
        };
        assert_eq!(
            render_prs_refreshed(&all),
            "fix: PR #12 open, checks pending\nold: PR #12 merged, no checks\ndocs: no PR\n\
             spike: PR unavailable: GitHub CLI is unavailable"
        );
    }

    fn failed_setup() -> SetupReport {
        SetupReport {
            outcome: SetupOutcome::Failed(Exit::Code(1)),
            script: Some(Setting {
                value: "make".to_owned(),
                source: Some("orca-term.yaml".to_owned()),
                overridden: Vec::new(),
            }),
            log: Some(PathBuf::from("/a/setup.log")),
            state: State::NotReady(NotReadyReason::Failed {
                exit: Exit::Code(1),
                last_line: Some("boom".to_owned()),
            }),
            activity: None,
            agent: None,
            unseen: false,
            notes: 0,
            denials: 0,
            operation: None,
        }
    }

    fn fleet(worktrees: Vec<Worktree>) -> Fleet {
        Fleet {
            primary: PrimaryCheckout {
                path: PathBuf::from("/repo"),
                branch: Some(Branch::new("main")),
            },
            worktrees,
        }
    }

    #[test]
    fn an_empty_fleet_says_so() {
        assert_eq!(render_fleet_table(&fleet(vec![])), "no worktrees in /repo");
    }

    #[test]
    fn the_detail_column_appears_only_when_a_worktree_has_one() {
        assert_eq!(
            render_fleet_table(&fleet(vec![worktree("a", State::NoAgent)])),
            "NAME  BRANCH  STATE     PATH\n\
             a     a       no agent  /l/a"
        );
        assert_eq!(
            render_fleet_table(&fleet(vec![
                worktree("a", State::SettingUp),
                worktree("bb", failed_setup().state),
            ])),
            "NAME  BRANCH  STATE       PATH   DETAIL\n\
             a     a       setting up  /l/a\n\
             bb    bb      not ready   /l/bb  setup failed (exit 1): boom"
        );
    }

    #[test]
    fn the_agent_column_says_which_agent_and_whether_it_runs() {
        let mut running = worktree("a", State::Idle);
        running.agent = Some("claude-code".to_owned());
        running.holder = HolderStatus::Alive;
        let mut stopped = worktree("b", State::Idle);
        stopped.agent = Some("claude-code".to_owned());
        assert_eq!(
            render_fleet_table(&fleet(vec![
                running,
                stopped,
                worktree("c", State::NoAgent)
            ])),
            "NAME  BRANCH  STATE     PATH  AGENT\n\
             a     a       idle      /l/a  claude-code (running)\n\
             b     b       idle      /l/b  claude-code (stopped)\n\
             c     c       no agent  /l/c"
        );
    }

    #[test]
    fn the_notes_column_counts_the_pending_notes_when_a_worktree_has_some() {
        let mut noted = worktree("a", State::NoAgent);
        noted.notes = 2;
        assert_eq!(
            render_fleet_table(&fleet(vec![noted, worktree("b", State::NoAgent)])),
            "NAME  BRANCH  STATE     PATH  NOTES\n\
             a     a       no agent  /l/a  2\n\
             b     b       no agent  /l/b"
        );
    }

    #[test]
    fn the_denials_column_counts_the_latest_turns_denials_when_a_worktree_has_some() {
        let mut denied = worktree("a", State::NoAgent);
        denied.denials = 3;
        let mut noted = worktree("b", State::NoAgent);
        noted.notes = 1;
        assert_eq!(
            render_fleet_table(&fleet(vec![denied, noted])),
            "NAME  BRANCH  STATE     PATH  NOTES  DENIALS\n\
             a     a       no agent  /l/a         3\n\
             b     b       no agent  /l/b  1"
        );
    }

    fn pending(id: u64, anchor: Anchor, current: Anchor, body: &str) -> PendingNote {
        PendingNote {
            note: Note {
                id,
                path: NotePath::parse("src/app.rs").unwrap(),
                anchor,
                body: body.to_owned(),
                rev: 1,
                created: 0,
                copy: None,
            },
            current,
        }
    }

    #[test]
    fn the_notes_are_listed_with_where_their_lines_are_now() {
        let listed = |notes| Listed {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
            notes,
        };
        assert_eq!(
            render_notes_listed(&listed(vec![])),
            "no pending notes in worktree fix"
        );
        let range = Anchor::Lines(LineRange::new(2, 4).unwrap());
        assert_eq!(
            render_notes_listed(&listed(vec![
                pending(1, Anchor::Line(3), Anchor::Line(5), "why?"),
                pending(2, range, Anchor::File, "two\nlines"),
                pending(3, Anchor::File, Anchor::File, "all"),
            ])),
            "ID  FILE        LINES                           NOTE\n\
             1   src/app.rs  line 3 (now line 5)             why?\n\
             2   src/app.rs  lines 2-4 (now the whole file)  two lines\n\
             3   src/app.rs  the whole file                  all"
        );
    }

    #[test]
    fn a_send_prints_the_prompt_then_the_notes_sent_and_kept() {
        let sent = |kept| NotesSent {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
            prompt: "File: a\nLine: 1\nUser comment: \"x\"".to_owned(),
            sent: vec![1, 2],
            kept,
        };
        assert_eq!(
            render_notes_sent(&sent(vec![])),
            "File: a\nLine: 1\nUser comment: \"x\"\n\nfix: sent notes 1, 2"
        );
        assert!(
            render_notes_sent(&sent(vec![2]))
                .ends_with("fix: sent notes 1, 2; kept 2 pending, edited while being sent")
        );
    }

    #[test]
    fn the_table_names_the_agent_states_without_their_activity() {
        let with = |name: &str, state| {
            let mut worktree = worktree(name, state);
            worktree.agent = Some("claude-code".to_owned());
            worktree.activity = Some("Bash: npm test".to_owned());
            worktree
        };
        let table = render_fleet_table(&fleet(vec![
            with("a", State::Working),
            with("b", State::Waiting),
            with("c", State::Finished),
            with("d", State::Failed),
            with("e", State::Interrupted),
        ]));
        for state in ["working", "waiting", "finished", "failed", "interrupted"] {
            assert!(table.contains(state), "{table}");
        }
        assert!(!table.contains("npm test"), "{table}");
    }

    #[test]
    fn the_table_marks_an_unseen_worktree_beside_its_state() {
        let with = |name: &str, state, unseen| {
            let mut worktree = worktree(name, state);
            worktree.agent = Some("claude-code".to_owned());
            worktree.unseen = unseen;
            worktree
        };
        assert_eq!(
            render_fleet_table(&fleet(vec![
                with("a", State::Finished, true),
                with("b", State::Finished, false),
                with("c", State::Idle, true),
            ])),
            "NAME  BRANCH  STATE              PATH  AGENT\n\
             a     a       finished (unseen)  /l/a  claude-code (stopped)\n\
             b     b       finished           /l/b  claude-code (stopped)\n\
             c     c       idle (unseen)      /l/c  claude-code (stopped)"
        );
    }

    #[test]
    fn seen_and_unseen_say_what_the_worktree_is_now() {
        let marked = |unseen| Marked {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
            unseen,
        };
        assert_eq!(render_worktree_marked(&marked(false)), "fix: seen");
        assert_eq!(render_worktree_marked(&marked(true)), "fix: unseen");
    }

    #[test]
    fn review_names_the_default_branch_and_the_short_base() {
        let reviewed = |against| Reviewed {
            worktree: worktree("demo", State::NoAgent),
            base: Commit::parse("1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b").unwrap(),
            against,
        };
        assert_eq!(
            render_worktree_reviewed(&reviewed(StartPoint::Local(Branch::new("main")))),
            "demo: review against main (1a2b3c4)"
        );
        assert_eq!(
            render_worktree_reviewed(&reviewed(StartPoint::Remote(Branch::new("trunk")))),
            "demo: review against origin/trunk (1a2b3c4)"
        );
    }

    #[test]
    fn the_detail_is_a_block_of_the_parts_with_something_to_say() {
        let full = Detail {
            worktree: Worktree {
                unseen: true,
                operation: Some(Operation::Rebase),
                ..worktree("fix", State::Failed)
            },
            why: Some("API Error: 400\n{\"x\":1}".to_owned()),
            denials: vec![Denial {
                tool: "Bash".to_owned(),
                text: "Bash: FOO=bar ls".to_owned(),
                id: None,
            }],
            setup: Some(SetupDetail {
                progress: SetupProgress::Failed(Exit::Code(2)),
                log: PathBuf::from("/a/setup.log"),
                tail: vec!["installing".to_owned(), "error: boom".to_owned()],
            }),
            drift: Some(Drift {
                against: Branch::new("main"),
                ahead: 2,
                behind: 1,
            }),
            operation_step: Some(RebaseStep {
                current: 2,
                total: 5,
            }),
        };
        assert_eq!(
            render_worktree_detail(&full),
            "fix: failed (unseen), rebasing\n  API Error: 400\n  {\"x\":1}\n\
             rebase in progress (2/5)\n\
             denied 1 tool call in the latest turn:\n  Bash: FOO=bar ls\n\
             setup failed (exit 2) (log /a/setup.log):\n  installing\n  error: boom\n\
             2 ahead of main, 1 behind"
        );
        let bare = Detail {
            worktree: worktree("docs", State::NoAgent),
            why: None,
            denials: Vec::new(),
            setup: Some(SetupDetail {
                progress: SetupProgress::Succeeded,
                log: PathBuf::from("/a/setup.log"),
                tail: Vec::new(),
            }),
            drift: None,
            operation_step: None,
        };
        assert_eq!(
            render_worktree_detail(&bare),
            "docs: no agent\nsetup succeeded (log /a/setup.log)\n\
             no drift to show: no local default branch, or a history unrelated to it"
        );
    }

    #[test]
    fn columns_align() {
        let table = render_fleet_table(&fleet(vec![Worktree {
            branch: None,
            ..worktree("docs", State::Broken(BrokenReason::DirectoryMissing))
        }]));
        assert_eq!(
            table,
            "NAME  BRANCH      STATE   PATH     DETAIL\n\
             docs  (detached)  broken  /l/docs  directory missing; run orca-term worktree prune"
        );
        let table = render_fleet_table(&fleet(vec![
            worktree("fix-login", State::NoAgent),
            Worktree {
                branch: None,
                ..worktree("docs", State::Broken(BrokenReason::DirectoryMissing))
            },
        ]));
        assert_eq!(
            table,
            "NAME       BRANCH      STATE     PATH          DETAIL\n\
             fix-login  fix-login   no agent  /l/fix-login\n\
             docs       (detached)  broken    /l/docs       directory missing; run orca-term worktree prune"
        );
    }

    #[test]
    fn the_state_column_names_a_git_operation_in_progress() {
        let table = render_fleet_table(&fleet(vec![
            Worktree {
                operation: Some(Operation::Rebase),
                ..worktree("fix", State::Idle)
            },
            Worktree {
                unseen: true,
                operation: Some(Operation::CherryPick),
                ..worktree("docs", State::Finished)
            },
            worktree("api", State::NoAgent),
        ]));
        assert_eq!(
            table,
            "NAME  BRANCH  STATE                              PATH\n\
             fix   fix     idle, rebasing                     /l/fix\n\
             docs  docs    finished (unseen), cherry-picking  /l/docs\n\
             api   api     no agent                           /l/api"
        );
    }

    #[test]
    fn the_operation_line_names_it_with_a_rebases_step() {
        let step = Some(RebaseStep {
            current: 2,
            total: 5,
        });
        assert_eq!(
            operation_line(Operation::Rebase, step),
            "rebase in progress (2/5)"
        );
        assert_eq!(
            operation_line(Operation::Rebase, None),
            "rebase in progress"
        );
        assert_eq!(operation_line(Operation::Merge, None), "merge in progress");
        assert_eq!(
            operation_line(Operation::CherryPick, None),
            "cherry-pick in progress"
        );
        assert_eq!(
            operation_line(Operation::Revert, None),
            "revert in progress"
        );
    }

    #[test]
    fn worktree_new_names_the_base_and_its_source() {
        let text = render_worktree_created(&Created {
            name: "fix".parse().unwrap(),
            path: PathBuf::from("/l/repo/fix"),
            branch: Branch::new("fix"),
            start: StartPoint::Local(Branch::new("main")),
            copied: 1,
            base: Setting {
                value: PathBuf::from("/l"),
                source: Some(".git/orca-term.yaml".to_owned()),
                overridden: vec!["orca-term.yaml".to_owned()],
            },
            setup: no_setup(),
        });
        assert_eq!(
            text,
            "created worktree fix\n\
             path:   /l/repo/fix\n\
             branch: fix (from main)\n\
             copied: 1 file from .worktreeinclude\n\
             base:   /l (from .git/orca-term.yaml, overriding orca-term.yaml)\n\
             setup:  nothing to run, no setup script is configured"
        );
    }

    #[test]
    fn a_failed_setup_says_where_the_log_is_and_how_to_try_again() {
        assert_eq!(
            render_setup("fix", &failed_setup()),
            "setup:  failed (script from orca-term.yaml)\n\
             log:    /a/setup.log\n\
             not ready: setup failed (exit 1): boom\n\
             fix the cause, then run `orca-term worktree setup fix`"
        );
        let done = SetupDone {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
            branch: None,
            setup: failed_setup(),
        };
        assert!(
            render_worktree_set_up(&done)
                .starts_with("worktree fix is not ready\npath:   /l/fix\n")
        );
    }

    #[test]
    fn a_successful_setup_names_its_script_and_its_log() {
        let setup = SetupReport {
            outcome: SetupOutcome::Succeeded,
            log: Some(PathBuf::from("/a/setup.log")),
            state: State::NoAgent,
            ..failed_setup()
        };
        assert_eq!(
            render_setup("fix", &setup),
            "setup:  succeeded (script from orca-term.yaml)\nlog:    /a/setup.log"
        );
    }

    #[test]
    fn agent_start_says_what_it_started_and_how_to_attach() {
        let mut fix = worktree("fix", State::Idle);
        fix.agent = Some("claude-code".to_owned());
        fix.holder = HolderStatus::Alive;
        let started = Started {
            worktree: fix,
            agent: Agent::ClaudeCode,
            outcome: StartOutcome::Started {
                pid: 4242,
                resumed: None,
            },
            writable: Some(Setting {
                value: vec![PathBuf::from("/home/me/.cache/pnpm")],
                source: Some(".git/orca-term.yaml".to_owned()),
                overridden: vec!["orca-term.yaml".to_owned()],
            }),
        };
        assert_eq!(
            render_agent_started(&started),
            "started agent claude-code in worktree fix\n\
             path:   /l/fix\n\
             holder: pid 4242\n\
             attach: orca-term agent attach fix"
        );
        assert_eq!(
            render_writable(started.writable.as_ref().unwrap()),
            "sandbox: extra writable paths /home/me/.cache/pnpm (from .git/orca-term.yaml, \
             overriding orca-term.yaml)"
        );
        assert_eq!(
            render_writable(&Setting {
                value: Vec::new(),
                source: None,
                overridden: Vec::new()
            }),
            "sandbox: no extra writable paths (built-in default)"
        );
        let resumed = Started {
            outcome: StartOutcome::Started {
                pid: 4242,
                resumed: SessionId::parse("0b983a87-4074-466e-b864-422ce6f063c8"),
            },
            ..started.clone()
        };
        assert!(render_agent_started(&resumed).starts_with(
            "started agent claude-code in worktree fix, resuming session \
             0b983a87-4074-466e-b864-422ce6f063c8\npath:   /l/fix\n"
        ));
        let again = Started {
            outcome: StartOutcome::AlreadyRunning { pid: None },
            writable: None,
            ..started
        };
        assert!(
            render_agent_started(&again)
                .starts_with("agent claude-code is already running in worktree fix\n")
        );
        assert!(render_agent_started(&again).contains("holder: pid unknown\n"));
    }

    #[test]
    fn agent_swap_says_which_agent_it_stopped_then_what_it_started() {
        let mut fix = worktree("fix", State::Idle);
        fix.agent = Some("opencode".to_owned());
        fix.holder = HolderStatus::Alive;
        let started = Started {
            worktree: fix,
            agent: Agent::OpenCode,
            outcome: StartOutcome::Started {
                pid: 4243,
                resumed: None,
            },
            writable: None,
        };
        let swapped = Swapped {
            previous: Some(Agent::ClaudeCode),
            stopped: true,
            started: started.clone(),
        };
        assert_eq!(
            render_agent_swapped(&swapped),
            "stopped agent claude-code in worktree fix\n\
             started agent opencode in worktree fix\n\
             path:   /l/fix\n\
             holder: pid 4243\n\
             attach: orca-term agent attach fix"
        );
        let nothing_running = Swapped {
            previous: Some(Agent::ClaudeCode),
            stopped: false,
            started,
        };
        assert!(
            render_agent_swapped(&nothing_running)
                .starts_with("started agent opencode in worktree fix\n")
        );
    }

    #[test]
    fn session_ls_is_a_table_marking_the_current_session() {
        let minute = 60 * 1000;
        let session = |id: &str, agent: &str| crate::domain::session::Session {
            id: id.to_owned(),
            agent: agent.to_owned(),
            first_prompt: None,
            turns: 0,
            last_activity: None,
            current: false,
            transcript: None,
        };
        let mut current = session("ses_1", "opencode");
        current.current = true;
        current.last_activity = Some(10 * minute);
        let mut old = session("s-1", "claude");
        old.turns = 12;
        old.last_activity = Some(0);
        old.first_prompt = Some(format!("fix the login\nand {}", "x".repeat(80)));
        let list = SessionList {
            worktree: worktree("fix", State::Idle),
            sessions: vec![current, old, session("a-1", "aider")],
            now: 15 * minute,
        };
        assert_eq!(
            render_sessions_listed(&list),
            [
                "   SESSION  AGENT        TURNS  LAST ACTIVITY  FIRST PROMPT".to_owned(),
                "*  ses_1    opencode     0      5 min ago      -".to_owned(),
                format!(
                    "   s-1      claude-code  12     15 min ago     fix the login and {}...",
                    "x".repeat(39)
                ),
                "   a-1      aider        0      -              -".to_owned(),
            ]
            .join("\n")
        );
        let empty = SessionList {
            sessions: Vec::new(),
            ..list
        };
        assert_eq!(
            render_sessions_listed(&empty),
            "no sessions in worktree fix"
        );
    }

    #[test]
    fn session_resume_and_new_say_what_they_did() {
        let started = Started {
            worktree: worktree("fix", State::Idle),
            agent: Agent::ClaudeCode,
            outcome: StartOutcome::AlreadyRunning { pid: Some(7) },
            writable: None,
        };
        let resumed = Resumed {
            session: "s-1".to_owned(),
            swapped: Swapped {
                previous: Some(Agent::ClaudeCode),
                stopped: false,
                started: started.clone(),
            },
        };
        assert!(
            render_session_resumed(&resumed).starts_with(
                "session s-1 is already current in worktree fix\nagent claude-code is already \
                 running in worktree fix\n"
            ),
            "{}",
            render_session_resumed(&resumed)
        );
        let discarded = Discarded {
            worktree: worktree("fix", State::Idle),
            session: Some("s-1".to_owned()),
            started: None,
        };
        assert_eq!(
            render_session_discarded(&discarded),
            "discarded session s-1 of worktree fix; `orca-term session resume fix s-1` brings it \
             back\nthe next start of its agent begins a new session"
        );
        let restarted = Discarded {
            session: None,
            started: Some(Started {
                outcome: StartOutcome::Started {
                    pid: 8,
                    resumed: None,
                },
                ..started
            }),
            ..discarded
        };
        assert!(
            render_session_discarded(&restarted).starts_with(
                "worktree fix had no current session\nstarted agent claude-code in worktree fix\n"
            ),
            "{}",
            render_session_discarded(&restarted)
        );
    }

    #[test]
    fn agent_stop_says_whether_there_was_anything_to_stop() {
        let stopped = |outcome| Stopped {
            worktree: worktree("fix", State::Idle),
            outcome,
        };
        assert_eq!(
            render_agent_stopped(&stopped(StopOutcome::Stopped)),
            "stopped the agent of worktree fix"
        );
        assert_eq!(
            render_agent_stopped(&stopped(StopOutcome::NotRunning)),
            "no agent is running in worktree fix"
        );
    }

    #[test]
    fn an_error_maps_to_its_exit_code() {
        use crate::domain::error::{BusyReason, TypedError};
        let busy = anyhow::Error::new(TypedError::Busy {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
            reason: BusyReason::Locked,
        });
        assert_eq!(error(&busy, Format::Json), ExitCode::from(3));
        assert_eq!(
            error(&anyhow::anyhow!("x"), Format::Text),
            ExitCode::FAILURE
        );
    }

    #[test]
    fn worktree_rm_says_what_happened_to_the_branch() {
        let removed = |branch| Removed {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/repo/fix"),
            branch,
        };
        assert_eq!(
            render_worktree_removed(&removed(BranchOutcome::Kept {
                branch: Branch::new("fix"),
                reason: KeptReason::NotMerged {
                    into: StartPoint::Local(Branch::new("main")),
                },
            })),
            "removed worktree fix\npath:   /l/repo/fix\nkept branch fix: not merged into main"
        );
        assert_eq!(
            render_worktree_removed(&removed(BranchOutcome::DeletedMerged {
                branch: Branch::new("fix"),
                into: StartPoint::Local(Branch::new("main")),
            })),
            "removed worktree fix\npath:   /l/repo/fix\ndeleted branch fix (merged into main)"
        );
    }

    #[test]
    fn worktree_prune_lists_paths_and_chains_or_says_nothing() {
        assert_eq!(
            render_worktrees_pruned(&Pruned::default()),
            "nothing to prune"
        );
        let worktrees = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        assert_eq!(
            render_worktrees_pruned(&Pruned {
                worktrees: worktrees.clone(),
                chains: Vec::new(),
            }),
            "pruned /a\npruned /b"
        );
        assert_eq!(
            render_worktrees_pruned(&Pruned {
                worktrees,
                chains: vec!["refs/orca-term/worktrees/a".to_owned()],
            }),
            "pruned /a\npruned /b\ndeleted refs/orca-term/worktrees/a"
        );
        assert_eq!(
            render_worktrees_pruned(&Pruned {
                worktrees: Vec::new(),
                chains: vec!["refs/orca-term/worktrees/a".to_owned()],
            }),
            "deleted refs/orca-term/worktrees/a"
        );
    }
}
