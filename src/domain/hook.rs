//! Domain: what claude's hooks report, and how each report changes a worktree's live status
//! (`domain/turn.rs`). claude calls `orca-term hook claude` for every event registered in the
//! settings it is launched with (`domain/launch.rs`), with the event as a JSON payload on stdin.
//! opencode's plugin reports the same moments in lines of its own, which `hook/opencode.rs` reads
//! as the payload claude's hook of the same meaning carries (#35), so both agents' turns follow
//! the rules below.
//!
//! A turn opens on a submitted prompt and closes when the agent stops. claude sends nothing when
//! the person presses Esc, so a turn whose end was never seen is closed as interrupted by
//! whatever comes next: a new prompt, the end of the session, a new session. A prompt typed while
//! a turn runs is queued: claude reports it with the running turn's `prompt_id`, then either folds
//! it into that turn or runs it as a turn of its own, whose events carry a `prompt_id` no
//! submitted prompt had.
//!
//! Payloads from subagents (and claude's internal agents) carry `agent_id`; they never open or
//! close a turn, nor set the session's transcript.
//!
//! claude reports a tool call its permission settings denied only in `PostToolBatch`, the event
//! closing each batch of tool calls, by the text of the call's `tool_response` (there is no error
//! flag): [`is_denial`]. Each denial is recorded on the open turn its `prompt_id` names, a
//! subagent's too; it never changes the turn's phase or activity, so it is never a state. The
//! person's own refusal at a permission prompt is not a denial, nor is a command the sandbox
//! stopped (ADR 0007): that is a failed command. Hooks give a lower bound: when the person
//! refuses or presses Esc on one call of a batch, claude sends no `PostToolBatch` for it, and a
//! rule denial in the same batch is lost.

pub(crate) mod opencode;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::domain::turn::{self, ClosedTurn, Denial, OpenTurn, Outcome, Phase, Status, TurnRecord};

/// The variable naming the file descriptor of the pipe an agent writes its events to when its
/// hooks cannot run orca-term themselves: opencode's plugin, inside the sandbox that denies writes
/// to the admin directory (#35). The holder makes the pipe, hands its write end to the agent, and
/// runs `orca-term hook <agent> --admin <dir>` outside the sandbox for each line, the line on its
/// stdin; `hook/opencode.rs` reads the line as the payload claude's hook would carry.
pub(crate) const EVENTS_FD_VAR: &str = "ORCA_TERM_EVENTS_FD";

/// The events a hook is registered for, each with its matcher (`*`: every tool), in the order
/// the settings list them.
pub(crate) const REGISTERED: &[(&str, Option<&str>)] = &[
    ("SessionStart", None),
    ("UserPromptSubmit", None),
    ("PreToolUse", Some("*")),
    ("PermissionRequest", Some("*")),
    ("PostToolUse", Some("*")),
    ("PostToolUseFailure", Some("*")),
    ("PostToolBatch", None),
    ("Stop", None),
    ("StopFailure", None),
    ("SessionEnd", None),
];

/// How the prompt claude submits after compacting a session starts. It continues the turn in
/// progress, so it opens none.
const COMPACTION_PROMPT: &str = "This session is being continued from a previous ";

/// How claude's `tool_response` begins when a deny rule on a tool refused the call:
/// `Permission to use Bash with command FOO=bar ls has been denied.`
const DENIED_TOOL_START: &str = "Permission to use ";

/// What claude's `tool_response` says when a deny rule refused the call, anywhere in it:
/// `<tool_use_error>File is in a directory that is denied by your permission settings.</tool_use_error>`.
const DENIED_PHRASES: &[&str] = &[
    "denied by your permission settings",
    "deny rule in your permission settings",
];

/// The `source` values of `SessionStart` that begin a new process or session: a turn left open
/// before them was interrupted. `compact` happens mid-turn and closes nothing.
const NEW_SESSION_SOURCES: &[&str] = &["startup", "resume", "clear"];

/// What a hook's payload says, reduced to the fields the rules read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Payload {
    /// `hook_event_name`: `UserPromptSubmit`, `Stop`, ...
    pub(crate) event: String,
    pub(crate) session_id: Option<String>,
    /// The file claude keeps the session's transcript in, `transcript_path`.
    pub(crate) transcript_path: Option<String>,
    pub(crate) prompt_id: Option<String>,
    /// Set on the events of subagents and of claude's internal agents.
    pub(crate) agent_id: Option<String>,
    /// The prompt of `UserPromptSubmit`.
    pub(crate) prompt: Option<String>,
    /// Why `SessionStart` fired: `startup`, `resume`, `clear`, `compact`.
    pub(crate) source: Option<String>,
    pub(crate) tool_name: Option<String>,
    /// The tool's input, `null` when absent.
    pub(crate) tool_input: Value,
    /// What the agent said last, on `Stop` and `StopFailure`.
    pub(crate) last_message: Option<String>,
    /// The type of the API error on `StopFailure`.
    pub(crate) error: Option<String>,
    /// The calls of the batch `PostToolBatch` closes, each with what it returned.
    pub(crate) tool_calls: Vec<ToolCall>,
}

/// One tool call of a `PostToolBatch` payload, reduced to the fields the rules read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolCall {
    /// `tool_name`.
    pub(crate) name: Option<String>,
    /// `tool_input`, `null` when absent.
    pub(crate) input: Value,
    /// `tool_use_id`, the agent's id for the call.
    pub(crate) id: Option<String>,
    /// `tool_response`, what the call returned, when it is text; a tool that returned anything
    /// else (the `Agent` tool's list of blocks) was not denied.
    pub(crate) response: Option<String>,
}

impl ToolCall {
    /// The call in `value`, one entry of `tool_calls`, every field taken when it is there and has
    /// the expected type.
    fn parse(value: &Value) -> Self {
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        Self {
            name: text("tool_name"),
            input: value.get("tool_input").cloned().unwrap_or(Value::Null),
            id: text("tool_use_id"),
            response: text("tool_response"),
        }
    }

    /// The denial this call is, when its permission settings denied it: the tool and the call in
    /// one line, as the activity shows it.
    pub(crate) fn denial(&self) -> Option<Denial> {
        let response = self.response.as_deref()?;
        let tool = self.name.as_deref()?;
        is_denial(response).then(|| Denial {
            tool: tool.to_owned(),
            text: tool_preview(tool, &self.input),
            id: self.id.clone(),
        })
    }
}

/// Whether a tool call's `tool_response`, `response`, says claude's permission settings denied
/// it: a deny rule on the tool (`Permission to use Bash with command ... has been denied.`) or on
/// a path (`... denied by your permission settings.`). Only the text tells: the match is on
/// claude's own words, kept honest by the recorded payloads in `tests/fixtures/claude-hooks/`.
/// The person's refusal (`The user doesn't want to proceed with this tool use.`) and a command
/// the sandbox stopped (`Operation not permitted`) are not denials.
pub(crate) fn is_denial(response: &str) -> bool {
    (response.starts_with(DENIED_TOOL_START) && response.contains(" denied"))
        || DENIED_PHRASES
            .iter()
            .any(|phrase| response.contains(phrase))
}

impl Payload {
    /// A payload of the event `event` that says nothing else, for the other fields to be filled.
    pub(crate) fn named(event: &str) -> Self {
        Self {
            event: event.to_owned(),
            session_id: None,
            transcript_path: None,
            prompt_id: None,
            agent_id: None,
            prompt: None,
            source: None,
            tool_name: None,
            tool_input: Value::Null,
            last_message: None,
            error: None,
            tool_calls: Vec::new(),
        }
    }

    /// The payload claude wrote to the hook's stdin, `bytes`. Only `hook_event_name` is
    /// required; every other field is taken when it is there and has the expected type.
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        let value: Value =
            serde_json::from_slice(bytes).context("the hook's payload is not JSON")?;
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        let Some(event) = text("hook_event_name") else {
            bail!("the hook's payload has no hook_event_name");
        };
        Ok(Self {
            event,
            session_id: text("session_id"),
            transcript_path: text("transcript_path"),
            prompt_id: text("prompt_id"),
            agent_id: text("agent_id"),
            prompt: text("prompt"),
            source: text("source"),
            tool_name: text("tool_name"),
            tool_input: value.get("tool_input").cloned().unwrap_or(Value::Null),
            last_message: text("last_assistant_message"),
            error: text("error"),
            tool_calls: value
                .get("tool_calls")
                .and_then(Value::as_array)
                .map(|calls| calls.iter().map(ToolCall::parse).collect())
                .unwrap_or_default(),
        })
    }
}

/// What a hook changes: the new status, and the lines to append to the turn log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Applied {
    pub(crate) status: Status,
    pub(crate) records: Vec<TurnRecord>,
}

/// Applies the event in `payload`, which a hook of `agent` received at `now`, to `status`. A turn
/// this event opens gets the number `next_number` and belongs to `agent`.
pub(crate) fn apply(
    status: Status,
    payload: &Payload,
    agent: &str,
    now: u64,
    next_number: u64,
) -> Applied {
    let mut step = Step {
        status,
        records: Vec::new(),
        agent,
        now,
        next_number,
    };
    step.follow_session(payload);
    if payload.agent_id.is_some() {
        step.subagent_event(payload);
    } else {
        step.main_event(payload);
    }
    step.status.updated_at = now;
    Applied {
        status: step.status,
        records: step.records,
    }
}

/// The status being changed by one event, with the log lines it produces.
#[derive(Debug)]
struct Step<'a> {
    status: Status,
    records: Vec<TurnRecord>,
    /// The agent whose hook this is: a turn it opens belongs to it.
    agent: &'a str,
    now: u64,
    next_number: u64,
}

impl Step<'_> {
    /// Records the session `payload` names as the current one, belonging to the agent whose hook
    /// this is, and the transcript it names when it comes from the main agent: a subagent's
    /// payload is not trusted to name the session's own transcript. A session that changes
    /// forgets the previous session's transcript, so the transcript recorded always belongs to the
    /// current session.
    fn follow_session(&mut self, payload: &Payload) {
        if let Some(session) = &payload.session_id {
            if self.status.session.as_ref() != Some(session) {
                self.status.transcript = None;
            }
            self.status.session = Some(session.clone());
            self.status.agent = Some(self.agent.to_owned());
        }
        if payload.agent_id.is_none()
            && let Some(path) = &payload.transcript_path
        {
            self.status.transcript = Some(PathBuf::from(path));
        }
    }

    /// An event of the main agent.
    fn main_event(&mut self, payload: &Payload) {
        match payload.event.as_str() {
            "UserPromptSubmit" => self.prompt_submitted(payload),
            "PreToolUse" | "PermissionRequest" | "PostToolUse" | "PostToolUseFailure" | "Stop"
            | "StopFailure" => {
                if self.follow_prompt(payload.prompt_id.as_deref()) {
                    self.turn_event(payload);
                }
            }
            "SessionStart" => {
                let source = payload.source.as_deref().unwrap_or_default();
                if NEW_SESSION_SOURCES.contains(&source) {
                    self.close(Outcome::Interrupted, None, None);
                }
            }
            "SessionEnd" => self.close(Outcome::Interrupted, None, None),
            "PostToolBatch" => self.batch_ended(payload),
            _ => {}
        }
    }

    /// An event of a subagent: only a permission request changes the phase, to waiting, and the
    /// end of a subagent's tool changes it back to working. Its denials count on the open turn
    /// when its `prompt_id` is that turn's.
    fn subagent_event(&mut self, payload: &Payload) {
        match payload.event.as_str() {
            "PermissionRequest" => self.wait(payload),
            "PostToolUse" | "PostToolUseFailure" => self.work(),
            "PostToolBatch" => {
                let same_turn = self.status.turn.as_ref().is_some_and(|turn| {
                    payload.prompt_id.is_some() && turn.prompt_id == payload.prompt_id
                });
                if same_turn {
                    self.record_denials(payload);
                }
            }
            _ => {}
        }
    }

    /// `PostToolBatch` of the main agent: an event of the turn its `prompt_id` names, which
    /// records the batch's denials there.
    fn batch_ended(&mut self, payload: &Payload) {
        if self.follow_prompt(payload.prompt_id.as_deref()) {
            self.record_denials(payload);
        }
    }

    /// Records on the open turn every call of the batch in `payload` that was denied. The phase
    /// and the activity stay as they were.
    fn record_denials(&mut self, payload: &Payload) {
        let Some(turn) = self.status.turn.as_mut() else {
            return;
        };
        for denial in payload.tool_calls.iter().filter_map(ToolCall::denial) {
            turn.record_denial(denial);
        }
    }

    /// `UserPromptSubmit`: a queued prompt when it carries the open turn's `prompt_id`, else a
    /// new turn, closing the open one as interrupted.
    fn prompt_submitted(&mut self, payload: &Payload) {
        let prompt = payload.prompt.as_deref().unwrap_or_default();
        if prompt.starts_with(COMPACTION_PROMPT) {
            return;
        }
        let queued =
            self.status.turn.as_ref().is_some_and(|turn| {
                payload.prompt_id.is_some() && turn.prompt_id == payload.prompt_id
            });
        if queued {
            self.status.queued = Some(prompt_line(prompt));
            return;
        }
        self.close(Outcome::Interrupted, None, None);
        self.status.queued = None;
        self.open(payload.prompt_id.clone(), prompt_line(prompt));
    }

    /// Makes the open turn the one `prompt_id` belongs to. A `prompt_id` no turn has means a
    /// queued prompt became a turn of its own: the open turn is closed as interrupted and one is
    /// opened for it, named after the queued prompt. With no turn open, the `prompt_id` of the
    /// latest closed turn means a late or repeated event of a turn that already ended: it is
    /// ignored. Returns whether a turn is open.
    fn follow_prompt(&mut self, prompt_id: Option<&str>) -> bool {
        let open_id = self
            .status
            .turn
            .as_ref()
            .map(|turn| turn.prompt_id.as_deref());
        let closed_id = self
            .status
            .latest
            .as_ref()
            .and_then(|turn| turn.prompt_id.as_deref());
        match (prompt_id, open_id) {
            (Some(id), Some(Some(open))) if id == open => true,
            (Some(id), None) if closed_id == Some(id) => false,
            (Some(id), _) => {
                let prompt = self.status.queued.take().unwrap_or_default();
                self.close(Outcome::Interrupted, None, None);
                self.open(Some(id.to_owned()), prompt);
                true
            }
            (None, open) => open.is_some(),
        }
    }

    /// An event of the open turn.
    fn turn_event(&mut self, payload: &Payload) {
        match payload.event.as_str() {
            "Stop" => {
                let detail = payload.last_message.as_deref().map(turn::one_line);
                self.close(Outcome::Finished, detail, None);
            }
            "StopFailure" => {
                let detail = payload.last_message.as_deref().map(turn::one_line);
                self.close(Outcome::Failed, detail, payload.error.clone());
                // The whole error, which the one-line detail cuts, for the detail float.
                if let Some(latest) = self.status.latest.as_mut() {
                    latest.message = payload
                        .last_message
                        .as_deref()
                        .map(turn::message)
                        .filter(|text| !text.is_empty());
                }
            }
            "PreToolUse" => self.run_tool(payload),
            "PermissionRequest" => self.wait(payload),
            _ => self.work(),
        }
    }

    /// `PreToolUse`: the open turn works, on the tool it names.
    fn run_tool(&mut self, payload: &Payload) {
        let Some(turn) = self.status.turn.as_mut() else {
            return;
        };
        turn.phase = Phase::Working;
        if let Some(tool) = &payload.tool_name {
            turn.activity = tool_preview(tool, &payload.tool_input);
        }
    }

    /// `PermissionRequest`: the open turn waits for the person, for what the request names.
    fn wait(&mut self, payload: &Payload) {
        let Some(turn) = self.status.turn.as_mut() else {
            return;
        };
        turn.phase = Phase::Waiting;
        if let Some(activity) = waiting_activity(payload) {
            turn.activity = activity;
        }
    }

    /// The end of a tool: the open turn works again, its activity kept.
    fn work(&mut self) {
        if let Some(turn) = self.status.turn.as_mut() {
            turn.phase = Phase::Working;
        }
    }

    /// Opens a turn for `prompt_id`, whose prompt is `prompt`, and logs its start.
    fn open(&mut self, prompt_id: Option<String>, prompt: String) {
        let turn = OpenTurn {
            number: self.next_number,
            session: self.status.session.clone(),
            prompt_id,
            activity: prompt.clone(),
            prompt,
            started_at: self.now,
            phase: Phase::Working,
            denials: Vec::new(),
            agent: self.agent.to_owned(),
            begin: None,
        };
        self.next_number += 1;
        self.records.push(TurnRecord::start(&turn));
        self.status.turn = Some(turn);
    }

    /// Closes the open turn, if any, as `outcome`, and logs its end. Its end was observed only
    /// when the agent said it stopped (finished or failed); an interrupted turn's was not.
    fn close(&mut self, outcome: Outcome, detail: Option<String>, error: Option<String>) {
        let Some(turn) = self.status.turn.take() else {
            return;
        };
        let closed = ClosedTurn {
            number: turn.number,
            session: turn.session,
            prompt_id: turn.prompt_id,
            prompt: turn.prompt,
            outcome,
            detail: detail.filter(|text| !text.is_empty()),
            error,
            ended_at: self.now,
            denials: turn.denials,
            message: None,
            begin: turn.begin,
            end: None,
            end_observed: outcome != Outcome::Interrupted,
        };
        self.records.push(TurnRecord::end(&closed));
        self.status.latest = Some(closed);
    }
}

/// What the agent waits for, on a `PermissionRequest`: the question it asks with
/// `AskUserQuestion`, else the tool it wants to run.
fn waiting_activity(payload: &Payload) -> Option<String> {
    let tool = payload.tool_name.as_deref()?;
    let question = payload
        .tool_input
        .pointer("/questions/0/question")
        .and_then(Value::as_str)
        .map(turn::one_line)
        .filter(|question| !question.is_empty());
    match question {
        Some(question) if tool == "AskUserQuestion" => Some(question),
        _ => Some(tool_preview(tool, &payload.tool_input)),
    }
}

/// A tool call in one line, `Tool: input`, with the input field shown for each tool as Orca
/// shows it; just the tool's name for any other tool, or when the field is missing.
pub(crate) fn tool_preview(tool: &str, input: &Value) -> String {
    let field = match tool {
        "Bash" => Some("command"),
        "Read" | "Edit" | "Write" | "MultiEdit" | "NotebookEdit" => Some("file_path"),
        "WebFetch" => Some("url"),
        "WebSearch" => Some("query"),
        "Grep" | "Glob" => Some("pattern"),
        _ => None,
    };
    let shown = field
        .and_then(|field| input.get(field))
        .and_then(Value::as_str)
        .map(turn::one_line)
        .filter(|text| !text.is_empty());
    match shown {
        Some(text) => turn::cap(&format!("{tool}: {text}")),
        None => turn::cap(tool),
    }
}

/// A prompt in one line: its first line that is neither blank nor one of the
/// `<pasted_content ...>` lines claude wraps pasted text in.
pub(crate) fn prompt_line(prompt: &str) -> String {
    let line = prompt
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !is_paste_wrapper(line));
    turn::cap(line.unwrap_or_default())
}

fn is_paste_wrapper(line: &str) -> bool {
    line.starts_with("<pasted_content") || line.starts_with("</pasted_content")
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::domain::agent::HolderStatus;
    use crate::domain::state;
    use crate::domain::turn::MAX_TEXT_CHARS;

    /// A payload of the main agent in session `s1`.
    fn event(name: &str, prompt_id: Option<&str>) -> Payload {
        Payload {
            event: name.to_owned(),
            session_id: Some("s1".to_owned()),
            transcript_path: None,
            prompt_id: prompt_id.map(str::to_owned),
            agent_id: None,
            prompt: None,
            source: None,
            tool_name: None,
            tool_input: Value::Null,
            last_message: None,
            error: None,
            tool_calls: Vec::new(),
        }
    }

    fn submit(prompt_id: &str, prompt: &str) -> Payload {
        Payload {
            prompt: Some(prompt.to_owned()),
            ..event("UserPromptSubmit", Some(prompt_id))
        }
    }

    fn tool(name: &str, prompt_id: &str, tool: &str, input: Value) -> Payload {
        Payload {
            tool_name: Some(tool.to_owned()),
            tool_input: input,
            ..event(name, Some(prompt_id))
        }
    }

    fn stop(prompt_id: &str, message: &str) -> Payload {
        Payload {
            last_message: Some(message.to_owned()),
            ..event("Stop", Some(prompt_id))
        }
    }

    fn session_start(session: &str, source: &str) -> Payload {
        Payload {
            session_id: Some(session.to_owned()),
            source: Some(source.to_owned()),
            ..event("SessionStart", None)
        }
    }

    /// Applies `events` in order from an empty status, numbering turns from 1, and returns the
    /// status and every log line, with the time of each event its index.
    fn replay(events: &[Payload]) -> (Status, Vec<TurnRecord>) {
        let mut status = Status::default();
        let mut log = Vec::new();
        for (at, payload) in (1u64..).zip(events) {
            let next = turn::next_turn_number(log.last().map(|r: &TurnRecord| r.turn), &status);
            let applied = apply(status, payload, "claude", at, next);
            status = applied.status;
            log.extend(applied.records);
        }
        (status, log)
    }

    /// Each log line as (turn, outcome), `None` for a start.
    fn summary(log: &[TurnRecord]) -> Vec<(u64, Option<Outcome>)> {
        log.iter().map(|r| (r.turn, r.outcome)).collect()
    }

    #[test]
    fn a_payload_is_read_leniently_but_needs_its_event() {
        let payload = Payload::parse(
            br#"{"hook_event_name":"Stop","session_id":"s","prompt_id":7,"last_assistant_message":"ok","extra":[1]}"#,
        )
        .unwrap();
        assert_eq!(payload.event, "Stop");
        assert_eq!(payload.session_id.as_deref(), Some("s"));
        assert_eq!(
            payload.prompt_id, None,
            "a field of another type is left out"
        );
        assert_eq!(payload.last_message.as_deref(), Some("ok"));
        assert!(Payload::parse(b"").is_err());
        assert!(Payload::parse(b"{not json").is_err());
        assert!(Payload::parse(br#"{"session_id":"s"}"#).is_err());
    }

    #[test]
    fn a_prompt_opens_a_turn_and_stop_finishes_it() {
        let (status, log) = replay(&[
            session_start("s1", "startup"),
            submit("p1", "fix the login\nplease"),
            stop("p1", "\nAll done.\nSecond line."),
        ]);
        assert_eq!(status.session.as_deref(), Some("s1"));
        assert_eq!(status.turn, None);
        let latest = status.latest.unwrap();
        assert_eq!(latest.number, 1);
        assert_eq!(latest.outcome, Outcome::Finished);
        assert_eq!(latest.prompt, "fix the login");
        assert_eq!(latest.detail.as_deref(), Some("All done."));
        assert_eq!(status.updated_at, 3);
        assert_eq!(summary(&log), [(1, None), (1, Some(Outcome::Finished))]);
        assert_eq!(log[0].prompt.as_deref(), Some("fix the login"));
    }

    #[test]
    fn stop_failure_fails_the_turn_with_the_error_type() {
        let (status, _) = replay(&[
            submit("p1", "go"),
            Payload {
                last_message: Some("API Error: 400 bad".to_owned()),
                error: Some("unknown".to_owned()),
                ..event("StopFailure", Some("p1"))
            },
        ]);
        let latest = status.latest.unwrap();
        assert_eq!(latest.outcome, Outcome::Failed);
        assert_eq!(latest.detail.as_deref(), Some("API Error: 400 bad"));
        assert_eq!(latest.error.as_deref(), Some("unknown"));
    }

    #[test]
    fn a_new_prompt_while_a_turn_is_open_interrupts_it() {
        let (status, log) = replay(&[submit("p1", "slow"), submit("p2", "again")]);
        assert_eq!(
            summary(&log),
            [(1, None), (1, Some(Outcome::Interrupted)), (2, None)]
        );
        let turn = status.turn.unwrap();
        assert_eq!((turn.number, turn.prompt.as_str()), (2, "again"));
        assert_eq!(status.latest.unwrap().detail, None);
    }

    #[test]
    fn a_queued_prompt_folded_into_the_running_turn_makes_one_turn() {
        let (status, log) = replay(&[
            submit("p1", "sleep"),
            tool("PreToolUse", "p1", "Bash", json!({"command": "sleep 6"})),
            submit("p1", "hello queued"),
            tool("PostToolUse", "p1", "Bash", json!({"command": "sleep 6"})),
            stop("p1", "done"),
        ]);
        assert_eq!(summary(&log), [(1, None), (1, Some(Outcome::Finished))]);
        assert_eq!(status.latest.unwrap().prompt, "sleep");
    }

    #[test]
    fn a_queued_prompt_run_as_its_own_turn_gets_the_queued_text() {
        let (status, log) = replay(&[
            submit("p1", "slow"),
            submit("p1", "hello queued"),
            stop("p1", "slow answer"),
            stop("p2", "ok"),
        ]);
        assert_eq!(
            summary(&log),
            [
                (1, None),
                (1, Some(Outcome::Finished)),
                (2, None),
                (2, Some(Outcome::Finished))
            ]
        );
        assert_eq!(log[2].prompt.as_deref(), Some("hello queued"));
        assert_eq!(status.queued, None, "the queued prompt was used");
        assert_eq!(status.latest.unwrap().detail.as_deref(), Some("ok"));
    }

    #[test]
    fn a_queued_prompt_then_esc_interrupts_the_first_turn() {
        let (_, log) = replay(&[
            submit("p1", "slow"),
            submit("p1", "hello queued"),
            stop("p2", "ok"),
        ]);
        assert_eq!(
            summary(&log),
            [
                (1, None),
                (1, Some(Outcome::Interrupted)),
                (2, None),
                (2, Some(Outcome::Finished))
            ]
        );
        assert_eq!(log[2].prompt.as_deref(), Some("hello queued"));
    }

    #[test]
    fn any_turn_event_with_an_unknown_prompt_id_opens_a_turn() {
        for name in [
            "PreToolUse",
            "PermissionRequest",
            "PostToolUse",
            "PostToolUseFailure",
        ] {
            let (status, log) = replay(&[
                submit("p1", "first"),
                tool(name, "p2", "Bash", json!({"command": "ls"})),
            ]);
            assert_eq!(
                summary(&log),
                [(1, None), (1, Some(Outcome::Interrupted)), (2, None)],
                "{name}"
            );
            assert_eq!(
                status.turn.unwrap().prompt,
                "",
                "{name}: nothing was queued"
            );
        }
        let (status, log) = replay(&[tool("PreToolUse", "p9", "Bash", json!({"command": "ls"}))]);
        assert_eq!(summary(&log), [(1, None)], "no turn was open");
        assert_eq!(status.turn.unwrap().activity, "Bash: ls");
    }

    #[test]
    fn events_of_a_turn_that_already_ended_are_ignored() {
        let late_tool = Payload {
            tool_name: Some("Bash".to_owned()),
            ..event("PostToolUse", Some("p1"))
        };
        for late in [late_tool, stop("p1", "again")] {
            let (status, log) = replay(&[submit("p1", "go"), stop("p1", "ok"), late.clone()]);
            assert_eq!(
                summary(&log),
                [(1, None), (1, Some(Outcome::Finished))],
                "{}",
                late.event
            );
            assert_eq!(status.turn, None, "{}", late.event);
            let latest = status.latest.unwrap();
            assert_eq!(latest.outcome, Outcome::Finished);
            assert_eq!(latest.detail.as_deref(), Some("ok"));
        }
    }

    #[test]
    fn a_stop_with_no_turn_and_no_prompt_id_is_ignored() {
        let (status, log) = replay(&[event("Stop", None), event("StopFailure", None)]);
        assert!(log.is_empty());
        assert_eq!(status.latest, None);
        assert_eq!(status.session.as_deref(), Some("s1"));
    }

    #[test]
    fn tools_set_the_activity_and_permission_requests_wait() {
        let bash = json!({"command": "FOO=bar ls"});
        let (status, _) = replay(&[
            submit("p1", "perm"),
            tool("PreToolUse", "p1", "Bash", bash.clone()),
        ]);
        let turn = status.turn.unwrap();
        assert_eq!(
            (turn.phase, turn.activity.as_str()),
            (Phase::Working, "Bash: FOO=bar ls")
        );

        let (status, _) = replay(&[
            submit("p1", "perm"),
            tool("PreToolUse", "p1", "Bash", bash.clone()),
            tool("PermissionRequest", "p1", "Bash", bash.clone()),
        ]);
        let turn = status.turn.unwrap();
        assert_eq!(
            (turn.phase, turn.activity.as_str()),
            (Phase::Waiting, "Bash: FOO=bar ls")
        );

        for after in ["PostToolUse", "PostToolUseFailure"] {
            let (status, _) = replay(&[
                submit("p1", "perm"),
                tool("PermissionRequest", "p1", "Bash", bash.clone()),
                Payload {
                    tool_name: Some("Bash".to_owned()),
                    ..event(after, Some("p1"))
                },
            ]);
            let turn = status.turn.unwrap();
            assert_eq!(turn.phase, Phase::Working, "{after}");
            assert_eq!(
                turn.activity, "Bash: FOO=bar ls",
                "{after} keeps the activity"
            );
        }
    }

    #[test]
    fn a_question_waits_with_the_question_as_the_activity() {
        let ask =
            json!({"questions": [{"question": "Which colour?\nPick one", "header": "Colour"}]});
        let (status, _) = replay(&[
            submit("p1", "ask"),
            tool("PreToolUse", "p1", "AskUserQuestion", ask.clone()),
        ]);
        assert_eq!(status.turn.unwrap().activity, "AskUserQuestion");
        let (status, _) = replay(&[
            submit("p1", "ask"),
            tool("PermissionRequest", "p1", "AskUserQuestion", ask),
        ]);
        let turn = status.turn.unwrap();
        assert_eq!(
            (turn.phase, turn.activity.as_str()),
            (Phase::Waiting, "Which colour?")
        );
    }

    #[test]
    fn session_start_closes_an_open_turn_unless_it_compacts() {
        for source in ["startup", "resume", "clear"] {
            let (status, log) = replay(&[submit("p1", "go"), session_start("s2", source)]);
            assert_eq!(status.session.as_deref(), Some("s2"), "{source}");
            assert_eq!(status.turn, None, "{source}");
            let latest = status.latest.unwrap();
            assert_eq!(latest.outcome, Outcome::Interrupted, "{source}");
            assert_eq!(
                latest.session.as_deref(),
                Some("s1"),
                "the turn keeps its session"
            );
            assert_eq!(log.len(), 2);
        }
        let (status, log) = replay(&[submit("p1", "go"), session_start("s1", "compact")]);
        assert!(status.turn.is_some());
        assert_eq!(log.len(), 1);
    }

    #[test]
    fn session_end_interrupts_the_open_turn() {
        let (status, log) = replay(&[submit("p1", "go"), event("SessionEnd", Some("p1"))]);
        assert_eq!(status.latest.unwrap().outcome, Outcome::Interrupted);
        assert_eq!(log.len(), 2);
        let (status, log) = replay(&[
            submit("p1", "go"),
            stop("p1", "ok"),
            event("SessionEnd", Some("p-exit")),
        ]);
        assert_eq!(
            status.latest.unwrap().outcome,
            Outcome::Finished,
            "/exit carries a new prompt_id and closes nothing"
        );
        assert_eq!(log.len(), 2);
    }

    #[test]
    fn only_a_turn_the_agent_said_it_stopped_has_its_end_observed() {
        let observed = |events: &[Payload]| {
            let (status, log) = replay(events);
            let latest = status.latest.unwrap();
            let end = log
                .iter()
                .rfind(|r| r.kind == turn::RecordKind::End)
                .unwrap();
            assert_eq!(end.end_observed, Some(latest.end_observed));
            latest.end_observed
        };
        assert!(observed(&[submit("p1", "go"), stop("p1", "ok")]));
        let failure = Payload {
            error: Some("unknown".to_owned()),
            ..event("StopFailure", Some("p1"))
        };
        assert!(observed(&[submit("p1", "go"), failure]));
        assert!(!observed(&[submit("p1", "go"), submit("p2", "again")]));
        assert!(!observed(&[submit("p1", "go"), event("SessionEnd", None)]));
        assert!(!observed(&[
            submit("p1", "go"),
            session_start("s2", "startup")
        ]));
    }

    #[test]
    fn a_turn_belongs_to_the_agent_whose_hook_opened_it() {
        let (status, log) = replay(&[submit("p1", "go")]);
        assert_eq!(status.turn.unwrap().agent, "claude");
        assert_eq!(log[0].agent.as_deref(), Some("claude"));
        assert_eq!(
            (log[0].begin.as_deref(), log[0].end.as_deref()),
            (None, None)
        );
    }

    #[test]
    fn subagents_never_open_or_close_a_turn_but_may_wait() {
        let sub = |payload: Payload| Payload {
            agent_id: Some("a1".to_owned()),
            ..payload
        };
        let (status, log) = replay(&[
            sub(submit("p1", "background report")),
            sub(stop("p1", "sub done")),
            sub(tool("PreToolUse", "p1", "Bash", json!({"command": "ls"}))),
        ]);
        assert!(log.is_empty());
        assert_eq!((status.turn, status.latest), (None, None));

        let (status, log) = replay(&[
            submit("p1", "go"),
            sub(tool("PreToolUse", "p2", "Bash", json!({"command": "ls"}))),
            sub(stop("p2", "sub done")),
            sub(tool(
                "PermissionRequest",
                "p2",
                "WebFetch",
                json!({"url": "https://x.example/"}),
            )),
        ]);
        assert_eq!(log.len(), 1, "only the main turn's start");
        let turn = status.turn.unwrap();
        assert_eq!(turn.number, 1);
        assert_eq!(turn.phase, Phase::Waiting);
        assert_eq!(turn.activity, "WebFetch: https://x.example/");

        let (status, _) = replay(&[
            submit("p1", "go"),
            sub(tool(
                "PermissionRequest",
                "p2",
                "Bash",
                json!({"command": "ls"}),
            )),
            sub(tool("PostToolUse", "p2", "Bash", json!({"command": "ls"}))),
        ]);
        assert_eq!(status.turn.unwrap().phase, Phase::Working);
    }

    #[test]
    fn the_compaction_prompt_opens_no_turn() {
        let (status, log) = replay(&[
            submit("p1", "go"),
            submit(
                "p2",
                "This session is being continued from a previous one that ran out of context.",
            ),
        ]);
        assert_eq!(log.len(), 1);
        assert_eq!(status.turn.unwrap().prompt_id.as_deref(), Some("p1"));
    }

    #[test]
    fn every_event_with_a_session_updates_the_current_session() {
        let (status, _) = replay(&[Payload {
            session_id: Some("s9".to_owned()),
            ..event("Notification", None)
        }]);
        assert_eq!(status.session.as_deref(), Some("s9"));
    }

    /// A payload of session `session` naming the transcript `path`.
    fn with_transcript(payload: Payload, session: &str, path: &str) -> Payload {
        Payload {
            session_id: Some(session.to_owned()),
            transcript_path: Some(path.to_owned()),
            ..payload
        }
    }

    #[test]
    fn the_main_agents_payloads_record_the_sessions_transcript() {
        let (status, _) = replay(&[with_transcript(
            session_start("s1", "startup"),
            "s1",
            "/t/s1.jsonl",
        )]);
        assert_eq!(status.transcript, Some(PathBuf::from("/t/s1.jsonl")));
        assert_eq!(
            Payload::parse(br#"{"hook_event_name":"Stop","transcript_path":"/t/s.jsonl"}"#)
                .unwrap()
                .transcript_path
                .as_deref(),
            Some("/t/s.jsonl")
        );
    }

    #[test]
    fn a_subagents_payload_never_sets_the_transcript() {
        let subagent = Payload {
            agent_id: Some("a1".to_owned()),
            ..with_transcript(event("PostToolUse", Some("p1")), "s1", "/t/agent-a1.jsonl")
        };
        let (status, _) = replay(&[
            with_transcript(session_start("s1", "startup"), "s1", "/t/s1.jsonl"),
            subagent.clone(),
        ]);
        assert_eq!(status.transcript, Some(PathBuf::from("/t/s1.jsonl")));
        let (status, _) = replay(&[subagent]);
        assert_eq!(status.transcript, None);
    }

    #[test]
    fn a_new_session_forgets_the_previous_transcript() {
        let (status, _) = replay(&[
            with_transcript(session_start("s1", "startup"), "s1", "/t/s1.jsonl"),
            session_start("s2", "clear"),
        ]);
        assert_eq!(status.session.as_deref(), Some("s2"));
        assert_eq!(status.transcript, None, "s1's transcript is not s2's");
        let (status, _) = replay(&[
            with_transcript(session_start("s1", "startup"), "s1", "/t/s1.jsonl"),
            submit("p1", "go"),
        ]);
        assert_eq!(
            status.transcript,
            Some(PathBuf::from("/t/s1.jsonl")),
            "a payload of the same session without a path keeps it"
        );
    }

    #[test]
    fn tool_previews_show_the_field_each_tool_is_known_by() {
        let cases = [
            (
                "Bash",
                json!({"command": "npm test\nmore"}),
                "Bash: npm test",
            ),
            ("Read", json!({"file_path": "/a/b.rs"}), "Read: /a/b.rs"),
            (
                "Edit",
                json!({"file_path": "src/main.rs"}),
                "Edit: src/main.rs",
            ),
            ("Write", json!({"file_path": "x"}), "Write: x"),
            ("MultiEdit", json!({"file_path": "y"}), "MultiEdit: y"),
            (
                "NotebookEdit",
                json!({"file_path": "n.ipynb"}),
                "NotebookEdit: n.ipynb",
            ),
            (
                "WebFetch",
                json!({"url": "https://example.org/"}),
                "WebFetch: https://example.org/",
            ),
            (
                "WebSearch",
                json!({"query": "rust flock"}),
                "WebSearch: rust flock",
            ),
            ("Grep", json!({"pattern": "fn main"}), "Grep: fn main"),
            ("Glob", json!({"pattern": "**/*.rs"}), "Glob: **/*.rs"),
            ("TodoWrite", json!({"todos": []}), "TodoWrite"),
            ("Bash", json!({}), "Bash"),
            ("Bash", json!({"command": 7}), "Bash"),
        ];
        for (tool, input, preview) in cases {
            assert_eq!(tool_preview(tool, &input), preview);
        }
        let long = tool_preview("Bash", &json!({"command": "x".repeat(1000)}));
        assert_eq!(long.chars().count(), MAX_TEXT_CHARS);
    }

    /// The recordings of claude 2.1.303 around denied tool calls (`tests/fixtures/claude-hooks/`),
    /// by name, read at compile time.
    const DENY_FIXTURES: &[(&str, &str)] = &[
        (
            "tui-deny",
            include_str!("../../tests/fixtures/claude-hooks/tui-deny-2.1.303.log"),
        ),
        (
            "tui-deny-esc",
            include_str!("../../tests/fixtures/claude-hooks/tui-deny-esc-2.1.303.log"),
        ),
        (
            "tui-deny-mixed-no",
            include_str!("../../tests/fixtures/claude-hooks/tui-deny-mixed-no-2.1.303.log"),
        ),
        (
            "tui-deny-multi",
            include_str!("../../tests/fixtures/claude-hooks/tui-deny-multi-2.1.303.log"),
        ),
        (
            "tui-deny-rule",
            include_str!("../../tests/fixtures/claude-hooks/tui-deny-rule-2.1.303.log"),
        ),
        (
            "tui-deny-rule-bash",
            include_str!("../../tests/fixtures/claude-hooks/tui-deny-rule-bash-2.1.303.log"),
        ),
        (
            "tui-deny-sub",
            include_str!("../../tests/fixtures/claude-hooks/tui-deny-sub-2.1.303.log"),
        ),
        (
            "tui-deny-tell",
            include_str!("../../tests/fixtures/claude-hooks/tui-deny-tell-2.1.303.log"),
        ),
        (
            "tui-sandbox",
            include_str!("../../tests/fixtures/claude-hooks/tui-sandbox-2.1.303.log"),
        ),
    ];

    /// The payloads of a recording, one per line (the event, a space, the payload).
    fn recorded(text: &str) -> Vec<Payload> {
        text.lines()
            .filter(|line| !line.is_empty())
            .map(|line| Payload::parse(line.split_once(' ').unwrap().1.as_bytes()).unwrap())
            .collect()
    }

    fn recording(name: &str) -> Vec<Payload> {
        let (_, text) = DENY_FIXTURES.iter().find(|(n, _)| *n == name).unwrap();
        recorded(text)
    }

    /// The denials of each ended turn in `log`, as (turn, texts).
    fn denials_by_turn(log: &[TurnRecord]) -> Vec<(u64, Vec<String>)> {
        log.iter()
            .filter(|record| record.outcome.is_some())
            .map(|record| {
                let texts = record.denials.iter().map(|d| d.text.clone()).collect();
                (record.turn, texts)
            })
            .collect()
    }

    #[test]
    fn a_denial_is_recognised_by_claudes_words_only() {
        for denied in [
            "Permission to use Bash with command FOO=bar ls has been denied.",
            "<tool_use_error>File is in a directory that is denied by your permission settings.</tool_use_error>",
            "Permission to use Write has been denied because of a deny rule in your permission settings.",
            "blocked: matches a deny rule in your permission settings",
        ] {
            assert!(is_denial(denied), "{denied}");
        }
        for not_denied in [
            "The user doesn't want to proceed with this tool use. The tool use was rejected.",
            "Exit code 1\ntouch: /x/from-bash.txt: Operation not permitted",
            "Permission to use Bash was granted.",
            "the file says: access denied",
            "",
        ] {
            assert!(!is_denial(not_denied), "{not_denied}");
        }
    }

    #[test]
    fn each_recorded_batch_gives_the_denials_claude_reported() {
        let mut by_fixture = Vec::new();
        for (name, text) in DENY_FIXTURES {
            let denials: Vec<String> = recorded(text)
                .iter()
                .flat_map(|payload| payload.tool_calls.iter().filter_map(ToolCall::denial))
                .map(|denial| format!("{}|{}", denial.tool, denial.text))
                .collect();
            by_fixture.push((*name, denials));
        }
        let write = |file: &str| format!("Write|Write: {{{{denied}}}}/{file}");
        assert_eq!(
            by_fixture,
            [
                ("tui-deny", vec![]),
                ("tui-deny-esc", vec![write("a.txt")]),
                ("tui-deny-mixed-no", vec![]),
                (
                    "tui-deny-multi",
                    vec![write("a.txt"), write("b.txt"), write("c.txt")]
                ),
                ("tui-deny-rule", vec![write("from-write.txt")]),
                (
                    "tui-deny-rule-bash",
                    vec!["Bash|Bash: FOO=bar ls".to_owned()]
                ),
                ("tui-deny-sub", vec![write("from-write.txt")]),
                ("tui-deny-tell", vec![],),
                ("tui-sandbox", vec![]),
            ]
        );
    }

    #[test]
    fn denials_are_recorded_on_the_turn_their_prompt_id_names() {
        let (status, log) = replay(&recording("tui-deny-multi"));
        assert_eq!(
            denials_by_turn(&log),
            [(
                1,
                vec![
                    "Write: {{denied}}/a.txt".to_owned(),
                    "Write: {{denied}}/b.txt".to_owned(),
                    "Write: {{denied}}/c.txt".to_owned()
                ]
            )]
        );
        assert_eq!(status.latest_denials().len(), 3);

        // A denial, then Esc: it stays on the interrupted turn; the next turn has none.
        let (status, log) = replay(&recording("tui-deny-esc"));
        assert_eq!(
            denials_by_turn(&log),
            [(1, vec!["Write: {{denied}}/a.txt".to_owned()]), (2, vec![])]
        );
        assert!(status.latest_denials().is_empty());

        // A subagent's denial counts on the main turn; the notification's turn has none.
        let (status, log) = replay(&recording("tui-deny-sub"));
        assert_eq!(
            denials_by_turn(&log),
            [
                (1, vec!["Write: {{denied}}/from-write.txt".to_owned()]),
                (2, vec![])
            ]
        );
        assert!(status.latest_denials().is_empty());

        for name in [
            "tui-deny",
            "tui-deny-tell",
            "tui-deny-mixed-no",
            "tui-sandbox",
        ] {
            let (status, log) = replay(&recording(name));
            assert!(
                denials_by_turn(&log)
                    .iter()
                    .all(|(_, texts)| texts.is_empty()),
                "{name}"
            );
            assert!(status.latest_denials().is_empty(), "{name}");
        }
    }

    #[test]
    fn denials_show_while_the_turn_is_open() {
        let events = recording("tui-deny-rule-bash");
        let (status, _) = replay(&events[..4]);
        let turn = status.turn.unwrap();
        assert_eq!(turn.denials.len(), 1);
        assert_eq!(turn.denials[0].text, "Bash: FOO=bar ls");
        assert_eq!(
            turn.denials[0].id.as_deref(),
            Some("toolu_msg_1792444455766_0")
        );
    }

    /// The state and activity a worktree with an agent shows, with its holder alive and gone,
    /// after each event of `events` but `PostToolBatch`.
    fn states_shown(events: &[Payload]) -> Vec<(state::State, Option<String>)> {
        let mut shown = Vec::new();
        let mut status = Status::default();
        let mut last = None;
        for (at, payload) in (1u64..).zip(events) {
            let next = turn::next_turn_number(last, &status);
            let applied = apply(status, payload, "claude", at, next);
            status = applied.status;
            last = applied.records.last().map(|r| r.turn).or(last);
            if payload.event == "PostToolBatch" {
                continue;
            }
            for holder in [HolderStatus::Alive, HolderStatus::Gone] {
                shown.push(state::ready(Some("claude-code"), Some(&status), holder));
            }
        }
        shown
    }

    #[test]
    fn denials_never_change_the_state() {
        for (name, text) in DENY_FIXTURES {
            let all = recorded(text);
            let without: Vec<Payload> = all
                .iter()
                .filter(|payload| payload.event != "PostToolBatch")
                .cloned()
                .collect();
            assert_eq!(states_shown(&all), states_shown(&without), "{name}");
        }
    }

    /// A `PostToolBatch` of `prompt_id` whose only call was denied, with the id `id`.
    fn denied_batch(prompt_id: &str, id: &str) -> Payload {
        Payload {
            tool_calls: vec![ToolCall {
                name: Some("Bash".to_owned()),
                input: json!({"command": "rm -rf build"}),
                id: Some(id.to_owned()),
                response: Some(
                    "Permission to use Bash with command rm -rf build has been denied.".to_owned(),
                ),
            }],
            ..event("PostToolBatch", Some(prompt_id))
        }
    }

    #[test]
    fn a_batch_outside_an_open_turn_is_ignored_and_a_call_counts_once() {
        let (status, log) = replay(&[
            submit("p1", "go"),
            denied_batch("p1", "t1"),
            denied_batch("p1", "t1"),
            stop("p1", "ok"),
            denied_batch("p1", "t2"),
        ]);
        assert_eq!(
            denials_by_turn(&log),
            [(1, vec!["Bash: rm -rf build".to_owned()])],
            "the same call reported twice counts once; a late batch changes nothing"
        );
        assert_eq!(status.latest_denials().len(), 1);

        let sub = |payload: Payload| Payload {
            agent_id: Some("a1".to_owned()),
            ..payload
        };
        let (status, log) = replay(&[sub(denied_batch("p1", "t1"))]);
        assert!(log.is_empty(), "a subagent's batch opens no turn");
        assert_eq!(status.turn, None);
        let (status, _) = replay(&[submit("p1", "go"), sub(denied_batch("p0", "t1"))]);
        assert!(
            status.latest_denials().is_empty(),
            "a subagent's batch of another prompt is not this turn's"
        );
        let (status, _) = replay(&[submit("p1", "go"), sub(denied_batch("p1", "t1"))]);
        assert_eq!(status.latest_denials().len(), 1);
    }

    #[test]
    fn a_failed_turn_keeps_its_whole_message() {
        let (status, log) = replay(&[
            submit("p1", "go"),
            Payload {
                last_message: Some("API Error: 400\n{\"error\":\"bad\"}".to_owned()),
                error: Some("unknown".to_owned()),
                ..event("StopFailure", Some("p1"))
            },
        ]);
        let latest = status.latest.unwrap();
        assert_eq!(latest.detail.as_deref(), Some("API Error: 400"));
        assert_eq!(
            latest.message.as_deref(),
            Some("API Error: 400\n{\"error\":\"bad\"}")
        );
        let (status, _) = replay(&[submit("p1", "go"), stop("p1", "done\nmore")]);
        assert_eq!(
            status.latest.unwrap().message,
            None,
            "only a failure keeps it"
        );
        assert_eq!(log[1].detail.as_deref(), Some("API Error: 400"));
    }

    #[test]
    fn a_payload_reads_its_tool_calls_leniently() {
        let payload = Payload::parse(
            br#"{"hook_event_name":"PostToolBatch","tool_calls":[{"tool_name":"Agent","tool_response":[{"type":"text"}]},{"tool_name":7},"x"]}"#,
        )
        .unwrap();
        assert_eq!(payload.tool_calls.len(), 3);
        assert_eq!(payload.tool_calls[0].response, None);
        assert_eq!(payload.tool_calls[1].name, None);
        assert!(
            payload
                .tool_calls
                .iter()
                .all(|call| call.denial().is_none())
        );
        let none = Payload::parse(br#"{"hook_event_name":"Stop"}"#).unwrap();
        assert!(none.tool_calls.is_empty());
    }

    #[test]
    fn a_prompt_line_skips_the_paste_wrappers() {
        assert_eq!(
            prompt_line("<pasted_content id=\"1\">\n\n  the pasted text\n</pasted_content>\nask"),
            "the pasted text"
        );
        assert_eq!(prompt_line("\n\nhello there\n"), "hello there");
        assert_eq!(prompt_line("<pasted_content>\n</pasted_content>"), "");
    }
}
