//! Domain: a worktree's turns as the agent's hooks report them (ADR 0009), kept in its admin
//! directory beside `worktree.json`, in files only the hook writes:
//!
//! - `status.json`, the live status: the current session, the open turn, the latest closed turn.
//!   The hook replaces it whole on every run (write and rename). The session commands also
//!   replace it, to move the current session while no agent runs (#41, `domain/session.rs`).
//! - `turns.jsonl`, the turn log: one line when a turn starts and one when it ends, only ever
//!   appended to.
//!
//! Both carry `schema`, as every state file does (ADR 0008). Reading tolerates: a file this core
//! cannot use reads as absent, so `worktree ls` never fails on it. The hook refuses to write over
//! a schema newer than this core supports.
//!
//! Times are Unix milliseconds. Turns are numbered across the whole worktree, never per session.
//!
//! A turn also keeps the tool calls the agent was denied during it (`Denial`, from the
//! `PostToolBatch` hook, `domain/hook.rs`): counted on the turn, never a state (ADR 0007).
//!
//! A turn is bounded by two snapshots of the worktree's tree (ADR 0006, `domain/snapshot.rs`):
//! the turn records the ids of its `begin` and `end` snapshot commits, whether its end was
//! observed, and its agent; the status records the tip of the worktree's chain of snapshots.
//! All of it was added within schema 1, so a file written before reads as none.

use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::domain::agent::Agent;

/// The newest schema of `status.json` this core reads and writes.
pub(crate) const STATUS_SCHEMA: u64 = 1;

/// The newest schema of a `turns.jsonl` line this core reads and writes.
pub(crate) const TURNS_SCHEMA: u64 = 1;

/// The most characters a stored text (a prompt, an activity, a detail) keeps.
pub(crate) const MAX_TEXT_CHARS: usize = 300;

/// The most characters a failed turn's whole error message keeps: far more than an API error
/// says, only a bound on `status.json`.
pub(crate) const MAX_MESSAGE_CHARS: usize = 4000;

/// The most denials a turn keeps; any more are dropped, so `status.json` and a line of the turn
/// log stay small.
pub(crate) const MAX_DENIALS: usize = 100;

/// A tool call the agent's permission settings denied it, as a `PostToolBatch` hook reported it
/// (`domain/hook.rs`). It is counted on its turn and never makes a state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Denial {
    /// The tool that was denied: `Bash`, `Write`, ...
    pub(crate) tool: String,
    /// The call in one line, as the activity shows a tool: `Bash: FOO=bar ls`.
    pub(crate) text: String,
    /// The agent's id for the call (`tool_use_id`), which tells a call reported twice apart. Kept
    /// in `status.json` only, never in the turn log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) id: Option<String>,
}

/// What the agent is doing in its open turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Phase {
    /// The agent is working: thinking, writing, or running a tool.
    Working,
    /// The agent waits for the person: a permission dialog or a question.
    Waiting,
}

/// How a turn ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Outcome {
    /// The agent stopped on its own.
    Finished,
    /// The agent stopped on an error of its API.
    Failed,
    /// The turn's end was never seen: the person pressed Esc and sent another prompt, the session
    /// ended, or the agent died.
    Interrupted,
}

/// The turn in progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct OpenTurn {
    /// The turn's number in the worktree.
    pub(crate) number: u64,
    /// The session the turn belongs to.
    pub(crate) session: Option<String>,
    /// The agent's id for the prompt that started the turn.
    pub(crate) prompt_id: Option<String>,
    /// The first line of that prompt.
    pub(crate) prompt: String,
    pub(crate) started_at: u64,
    pub(crate) phase: Phase,
    /// What the agent is doing, in one line: the prompt, then each tool it runs.
    pub(crate) activity: String,
    /// The tool calls denied so far in the turn, in the order reported. Added within schema 1: a
    /// file written before it reads as none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) denials: Vec<Denial>,
    /// The agent the turn belongs to (`claude`), the one whose hook opened it. Added within schema
    /// 1: a file written before it reads as empty.
    #[serde(default)]
    pub(crate) agent: String,
    /// The snapshot commit the turn began on, `None` when it could not be taken. Added within
    /// schema 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) begin: Option<String>,
}

impl OpenTurn {
    /// Records `denial` on the turn, unless the turn already holds a denial of the same call (by
    /// its id) or as many denials as it keeps.
    pub(crate) fn record_denial(&mut self, denial: Denial) {
        let known = denial.id.is_some() && self.denials.iter().any(|seen| seen.id == denial.id);
        if !known && self.denials.len() < MAX_DENIALS {
            self.denials.push(denial);
        }
    }
}

/// A turn that ended.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ClosedTurn {
    pub(crate) number: u64,
    pub(crate) session: Option<String>,
    pub(crate) prompt_id: Option<String>,
    pub(crate) prompt: String,
    pub(crate) outcome: Outcome,
    /// What the agent said last, in one line, when the end was seen.
    pub(crate) detail: Option<String>,
    /// The type of the API error a failed turn ended on (`rate_limit`, `unknown`, ...).
    pub(crate) error: Option<String>,
    pub(crate) ended_at: u64,
    /// The tool calls denied during the turn, in the order reported. Added within schema 1.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) denials: Vec<Denial>,
    /// What the agent said last on a failed turn, whole (every line, up to
    /// [`MAX_MESSAGE_CHARS`]): the error `detail` shows in one line. Kept in `status.json` only.
    /// Added within schema 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) message: Option<String>,
    /// The snapshot commit the turn began on, when it was taken. Added within schema 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) begin: Option<String>,
    /// The snapshot commit the turn ended on, when it was taken. Added within schema 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) end: Option<String>,
    /// Whether the turn's end was seen (`Stop` or `StopFailure`). A turn closed any other way
    /// (interrupted) ends on the snapshot taken by whatever told the core it was over, so its diff
    /// may hold edits made after the interruption. Added within schema 1: a turn recorded before
    /// it reads as not observed, and it has no end snapshot either.
    #[serde(default)]
    pub(crate) end_observed: bool,
}

/// The live status of a worktree's agent, as `status.json` records it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Status {
    /// The current session: the agent's id for it, from the latest hook that named one.
    #[serde(default)]
    pub(crate) session: Option<String>,
    /// The current session's transcript, the file claude keeps it in
    /// (`~/.claude/projects/<dir>/<session>.jsonl`), from the latest hook of the main agent that
    /// named one. Added within schema 1: a file written before it reads as `None`.
    #[serde(default)]
    pub(crate) transcript: Option<PathBuf>,
    /// The agent whose hooks named the current session (`claude`, `opencode`), so a session is
    /// only ever resumed by the agent it belongs to (#35). Added within schema 1: a file written
    /// before it reads as `None`, which is claude's, the only agent then.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent: Option<String>,
    /// The turn in progress, if any.
    #[serde(default)]
    pub(crate) turn: Option<OpenTurn>,
    /// A prompt the person typed while a turn ran, remembered until it runs as a turn of its own
    /// or a new prompt starts a turn.
    #[serde(default)]
    pub(crate) queued: Option<String>,
    /// The latest turn that ended.
    #[serde(default)]
    pub(crate) latest: Option<ClosedTurn>,
    /// The tip of the worktree's chain of snapshots: the latest snapshot commit the hooks took
    /// (`domain/snapshot.rs`). Added within schema 1: a file written before it reads as none.
    #[serde(default)]
    pub(crate) snapshot: Option<String>,
    #[serde(default)]
    pub(crate) updated_at: u64,
}

impl Status {
    /// The agent the current session belongs to, as its hooks name it: the one recorded, else
    /// claude, whose hooks were the only ones before the agent was recorded.
    pub(crate) fn session_agent(&self) -> &str {
        self.agent.as_deref().unwrap_or(Agent::ClaudeCode.program())
    }

    /// The denials of the worktree's latest turn: the open one, else the latest that ended,
    /// whatever its outcome or session. None before any turn.
    pub(crate) fn latest_denials(&self) -> &[Denial] {
        match (&self.turn, &self.latest) {
            (Some(open), _) => &open.denials,
            (None, Some(closed)) => &closed.denials,
            (None, None) => &[],
        }
    }
}

/// `status.json` as this core writes it: the status with its schema.
#[derive(Debug, Serialize)]
struct StatusFile<'a> {
    schema: u64,
    #[serde(flatten)]
    status: &'a Status,
}

/// Only the schema of a state file, read before anything else.
#[derive(Debug, Deserialize)]
struct SchemaOnly {
    #[serde(default)]
    schema: Option<u64>,
}

/// What reading `status.json` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StatusRead {
    /// No file: no hook has reported yet.
    Missing,
    /// A status this core can use.
    Usable(Box<Status>),
    /// A file whose schema is newer than this core supports.
    NewerSchema(u64),
    /// A file that is not a status, for the reason given.
    Unreadable(String),
}

impl StatusRead {
    /// The status, when there is one this core can use.
    pub(crate) fn usable(self) -> Option<Status> {
        match self {
            Self::Usable(status) => Some(*status),
            Self::Missing | Self::NewerSchema(_) | Self::Unreadable(_) => None,
        }
    }
}

/// What `status.json`, whose content is `bytes` (`None` when it does not exist), holds.
pub(crate) fn read_status(bytes: Option<&[u8]>) -> StatusRead {
    let Some(bytes) = bytes else {
        return StatusRead::Missing;
    };
    match serde_json::from_slice::<SchemaOnly>(bytes) {
        Ok(SchemaOnly {
            schema: Some(schema),
        }) if schema > STATUS_SCHEMA => return StatusRead::NewerSchema(schema),
        Ok(_) => {}
        Err(err) => return StatusRead::Unreadable(err.to_string()),
    }
    match serde_json::from_slice::<Status>(bytes) {
        Ok(status) => StatusRead::Usable(Box::new(status)),
        Err(err) => StatusRead::Unreadable(err.to_string()),
    }
}

/// The bytes written to `status.json`.
pub(crate) fn status_bytes(status: &Status) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&StatusFile {
        schema: STATUS_SCHEMA,
        status,
    })?)
}

/// Whether a line of the turn log says a turn started or ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecordKind {
    Start,
    End,
}

/// One line of `turns.jsonl`. A start carries the prompt, the agent and the begin snapshot; an end
/// carries the outcome, the detail, the error, the denials (without their ids), both snapshots and
/// whether the end was observed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct TurnRecord {
    schema: u64,
    pub(crate) kind: RecordKind,
    pub(crate) turn: u64,
    pub(crate) session: Option<String>,
    pub(crate) prompt_id: Option<String>,
    /// When the turn started or ended.
    pub(crate) at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) prompt: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) outcome: Option<Outcome>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) detail: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) error: Option<String>,
    /// Added within schema 1: a line written before it reads as none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) denials: Vec<Denial>,
    /// The turn's agent, on a start. Added within schema 1, as are the fields below.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) agent: Option<String>,
    /// The snapshot commit the turn began on, when it was taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) begin: Option<String>,
    /// The snapshot commit the turn ended on, on an end, when it was taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) end: Option<String>,
    /// Whether the turn's end was observed, on an end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) end_observed: Option<bool>,
    /// The transcript claude keeps the turn's session in, on a start, when its hooks named one
    /// (#41): what `session resume` checks before it resumes a session that is no longer the
    /// current one. Added within schema 1: a line written before it reads as none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) transcript: Option<PathBuf>,
}

impl TurnRecord {
    /// The line that says `turn` started.
    pub(crate) fn start(turn: &OpenTurn) -> Self {
        Self {
            schema: TURNS_SCHEMA,
            kind: RecordKind::Start,
            turn: turn.number,
            session: turn.session.clone(),
            prompt_id: turn.prompt_id.clone(),
            at: turn.started_at,
            prompt: Some(turn.prompt.clone()),
            outcome: None,
            detail: None,
            error: None,
            denials: Vec::new(),
            agent: Some(turn.agent.clone()).filter(|agent| !agent.is_empty()),
            begin: turn.begin.clone(),
            end: None,
            end_observed: None,
            transcript: None,
        }
    }

    /// The same line, recording `transcript` as its session's transcript.
    pub(crate) fn with_transcript(self, transcript: Option<PathBuf>) -> Self {
        Self { transcript, ..self }
    }

    /// The line that says `turn` ended.
    pub(crate) fn end(turn: &ClosedTurn) -> Self {
        Self {
            schema: TURNS_SCHEMA,
            kind: RecordKind::End,
            turn: turn.number,
            session: turn.session.clone(),
            prompt_id: turn.prompt_id.clone(),
            at: turn.ended_at,
            prompt: None,
            outcome: Some(turn.outcome),
            detail: turn.detail.clone(),
            error: turn.error.clone(),
            denials: turn
                .denials
                .iter()
                .map(|denial| Denial {
                    id: None,
                    ..denial.clone()
                })
                .collect(),
            agent: None,
            begin: turn.begin.clone(),
            end: turn.end.clone(),
            end_observed: Some(turn.end_observed),
            transcript: None,
        }
    }

    /// The record as one line of the log, newline included.
    pub(crate) fn to_line(&self) -> Result<Vec<u8>> {
        let mut line = serde_json::to_vec(self)?;
        line.push(b'\n');
        Ok(line)
    }
}

/// A line of the turn log as this core reads it, from any schema.
#[derive(Debug, Deserialize)]
struct SeenRecord {
    #[serde(default)]
    schema: Option<u64>,
    turn: u64,
}

/// The number of the latest turn in the end of the turn log, `tail`: its last line that reads as
/// a record (the first line of a tail may be cut, and is skipped). `None` when no line does.
/// Refuses a record whose schema is newer than this core supports, since the turn log is only
/// read to append to it.
pub(crate) fn last_turn_number(tail: &[u8]) -> Result<Option<u64>> {
    for line in tail.split(|&byte| byte == b'\n').rev() {
        let Ok(record) = serde_json::from_slice::<SeenRecord>(line) else {
            continue;
        };
        check_record_schema(record.schema)?;
        return Ok(Some(record.turn));
    }
    Ok(None)
}

/// Every line of the turn log `log`, whole, in the order written. A line that does not read as a
/// record (one cut short by a crash, or blank) is skipped. Refuses a record whose schema is newer
/// than this core supports, since it could mean anything.
pub(crate) fn read_log(log: &[u8]) -> Result<Vec<TurnRecord>> {
    let mut records = Vec::new();
    for line in log.split(|&byte| byte == b'\n') {
        let Ok(record) = serde_json::from_slice::<TurnRecord>(line) else {
            if let Ok(seen) = serde_json::from_slice::<SeenRecord>(line) {
                check_record_schema(seen.schema)?;
            }
            continue;
        };
        check_record_schema(Some(record.schema))?;
        records.push(record);
    }
    Ok(records)
}

/// Refuses a turn log record of a schema newer than this core supports.
fn check_record_schema(schema: Option<u64>) -> Result<()> {
    if let Some(schema) = schema
        && schema > TURNS_SCHEMA
    {
        bail!(
            "turns.jsonl has schema {schema}, newer than the schema {TURNS_SCHEMA} this \
             orca-term supports"
        );
    }
    Ok(())
}

/// The number the next turn gets: one more than the highest the log or the status knows.
pub(crate) fn next_turn_number(last_logged: Option<u64>, status: &Status) -> u64 {
    let open = status.turn.as_ref().map(|turn| turn.number);
    let closed = status.latest.as_ref().map(|turn| turn.number);
    [last_logged, open, closed]
        .into_iter()
        .flatten()
        .max()
        .unwrap_or(0)
        + 1
}

/// Refuses to write over a `status.json` whose schema is newer than this core supports.
pub(crate) fn check_status_overwrite(read: &StatusRead) -> Result<()> {
    if let StatusRead::NewerSchema(schema) = read {
        bail!(
            "status.json has schema {schema}, newer than the schema {STATUS_SCHEMA} this \
             orca-term supports"
        );
    }
    Ok(())
}

/// `text` as one line of at most [`MAX_TEXT_CHARS`] characters: its first line that is not
/// blank, trimmed, with control characters turned into spaces. Empty when every line is blank.
pub(crate) fn one_line(text: &str) -> String {
    let line = text.lines().map(str::trim).find(|line| !line.is_empty());
    cap(line.unwrap_or_default())
}

/// `text` whole, every line kept, trimmed and cut to at most [`MAX_MESSAGE_CHARS`] characters,
/// with control characters other than line feeds and tabs turned into spaces. Empty when it is
/// blank.
pub(crate) fn message(text: &str) -> String {
    text.trim()
        .replace("\r\n", "\n")
        .chars()
        .take(MAX_MESSAGE_CHARS)
        .map(|c| {
            if c.is_control() && c != '\n' && c != '\t' {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// `text` cut to at most [`MAX_TEXT_CHARS`] characters, on a character boundary, with control
/// characters turned into spaces.
pub(crate) fn cap(text: &str) -> String {
    text.chars()
        .take(MAX_TEXT_CHARS)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_log(bytes: &[u8]) -> Vec<TurnRecord> {
        bytes
            .split(|&byte| byte == b'\n')
            .filter(|line| !line.is_empty())
            .map(|line| serde_json::from_slice(line).unwrap())
            .collect()
    }

    fn open(number: u64) -> OpenTurn {
        OpenTurn {
            number,
            session: Some("s1".to_owned()),
            prompt_id: Some("p1".to_owned()),
            prompt: "fix it".to_owned(),
            started_at: 10,
            phase: Phase::Working,
            activity: "fix it".to_owned(),
            denials: Vec::new(),
            agent: "claude".to_owned(),
            begin: None,
        }
    }

    fn closed(number: u64) -> ClosedTurn {
        ClosedTurn {
            number,
            session: Some("s1".to_owned()),
            prompt_id: Some("p1".to_owned()),
            prompt: "fix it".to_owned(),
            outcome: Outcome::Failed,
            detail: Some("API Error".to_owned()),
            error: Some("rate_limit".to_owned()),
            ended_at: 20,
            denials: Vec::new(),
            message: None,
            begin: None,
            end: None,
            end_observed: true,
        }
    }

    fn denial(id: Option<&str>, text: &str) -> Denial {
        Denial {
            tool: "Bash".to_owned(),
            text: text.to_owned(),
            id: id.map(str::to_owned),
        }
    }

    #[test]
    fn the_status_is_written_with_its_schema_and_read_back() {
        let status = Status {
            session: Some("s1".to_owned()),
            transcript: Some(PathBuf::from("/home/me/.claude/projects/-w-fix/s1.jsonl")),
            agent: Some("claude".to_owned()),
            turn: Some(open(2)),
            queued: Some("and then".to_owned()),
            latest: Some(closed(1)),
            snapshot: Some("c0ffee".to_owned()),
            updated_at: 30,
        };
        let bytes = status_bytes(&status).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(
            text.starts_with(
                r#"{"schema":1,"session":"s1","transcript":"/home/me/.claude/projects/-w-fix/s1.jsonl","agent":"claude","turn":{"number":2,"#
            ),
            "{text}"
        );
        assert!(text.contains(r#""phase":"working""#), "{text}");
        assert!(text.contains(r#""outcome":"failed""#), "{text}");
        assert_eq!(
            read_status(Some(&bytes)),
            StatusRead::Usable(Box::new(status))
        );
    }

    #[test]
    fn the_sessions_agent_is_the_one_recorded_else_claude() {
        let before = read_status(Some(br#"{"schema":1,"session":"s1"}"#))
            .usable()
            .unwrap();
        assert_eq!(
            before.agent, None,
            "a status written before the agent was named"
        );
        assert_eq!(before.session_agent(), "claude");
        let opencodes = Status {
            agent: Some("opencode".to_owned()),
            ..before
        };
        assert_eq!(opencodes.session_agent(), "opencode");
        let text = String::from_utf8(status_bytes(&Status::default()).unwrap()).unwrap();
        assert!(
            !text.contains("agent"),
            "no agent written when none: {text}"
        );
    }

    #[test]
    fn a_status_this_core_cannot_use_reads_as_absent() {
        assert_eq!(read_status(None), StatusRead::Missing);
        assert_eq!(
            read_status(Some(br#"{"schema":2,"turn":"elsewhere"}"#)),
            StatusRead::NewerSchema(2)
        );
        assert!(matches!(
            read_status(Some(b"{not json")),
            StatusRead::Unreadable(_)
        ));
        assert!(matches!(
            read_status(Some(br#"{"schema":1,"turn":7}"#)),
            StatusRead::Unreadable(_)
        ));
        assert_eq!(read_status(Some(b"{not json")).usable(), None);
        assert_eq!(
            read_status(Some(br#"{"schema":1,"future":true}"#)).usable(),
            Some(Status::default()),
            "unknown fields are ignored"
        );
    }

    #[test]
    fn a_status_written_before_the_transcript_still_reads() {
        let old = br#"{"schema":1,"session":"s1","turn":null,"queued":null,"latest":null,"updated_at":5}"#;
        assert_eq!(
            read_status(Some(old)).usable(),
            Some(Status {
                session: Some("s1".to_owned()),
                updated_at: 5,
                ..Status::default()
            })
        );
    }

    #[test]
    fn only_a_newer_status_refuses_to_be_overwritten() {
        assert!(check_status_overwrite(&StatusRead::NewerSchema(2)).is_err());
        check_status_overwrite(&StatusRead::Missing).unwrap();
        check_status_overwrite(&StatusRead::Unreadable("x".to_owned())).unwrap();
        check_status_overwrite(&StatusRead::Usable(Box::default())).unwrap();
    }

    #[test]
    fn a_turn_logs_one_line_at_its_start_and_one_at_its_end() {
        let start = TurnRecord::start(&open(3)).to_line().unwrap();
        assert_eq!(
            String::from_utf8(start.clone()).unwrap(),
            "{\"schema\":1,\"kind\":\"start\",\"turn\":3,\"session\":\"s1\",\"prompt_id\":\"p1\",\"at\":10,\"prompt\":\"fix it\",\"agent\":\"claude\"}\n"
        );
        let end = TurnRecord::end(&closed(3)).to_line().unwrap();
        assert_eq!(
            String::from_utf8(end.clone()).unwrap(),
            "{\"schema\":1,\"kind\":\"end\",\"turn\":3,\"session\":\"s1\",\"prompt_id\":\"p1\",\"at\":20,\"outcome\":\"failed\",\"detail\":\"API Error\",\"error\":\"rate_limit\",\"end_observed\":true}\n"
        );
        let log = [start, end].concat();
        let records = parse_log(&log);
        assert_eq!(records.len(), 2);
        assert_eq!(records[1].outcome, Some(Outcome::Failed));
    }

    #[test]
    fn the_last_turn_number_comes_from_the_last_whole_line() {
        assert_eq!(last_turn_number(b"").unwrap(), None);
        let log = b"rn\":1,\"cut\n{\"schema\":1,\"kind\":\"start\",\"turn\":4}\n{\"schema\":1,\"kind\":\"end\",\"turn\":5}\n";
        assert_eq!(last_turn_number(log).unwrap(), Some(5));
        assert_eq!(
            last_turn_number(b"{\"schema\":1,\"turn\":2}\n{\"torn").unwrap(),
            Some(2),
            "a torn last line is skipped"
        );
        assert!(last_turn_number(b"{\"schema\":9,\"turn\":2}\n").is_err());
    }

    #[test]
    fn the_next_turn_number_is_one_more_than_any_known() {
        assert_eq!(next_turn_number(None, &Status::default()), 1);
        assert_eq!(next_turn_number(Some(4), &Status::default()), 5);
        let status = Status {
            turn: Some(open(7)),
            latest: Some(closed(6)),
            ..Status::default()
        };
        assert_eq!(next_turn_number(Some(4), &status), 8);
    }

    #[test]
    fn a_turn_keeps_each_denied_call_once_and_at_most_its_cap() {
        let mut turn = open(1);
        turn.record_denial(denial(Some("t1"), "Bash: ls"));
        turn.record_denial(denial(Some("t1"), "Bash: ls"));
        turn.record_denial(denial(Some("t2"), "Bash: ls"));
        turn.record_denial(denial(None, "Bash: pwd"));
        turn.record_denial(denial(None, "Bash: pwd"));
        let ids: Vec<Option<&str>> = turn.denials.iter().map(|d| d.id.as_deref()).collect();
        assert_eq!(
            ids,
            [Some("t1"), Some("t2"), None, None],
            "deduplicated by the call's id; a call without one cannot be told apart"
        );
        for n in 0..MAX_DENIALS * 2 {
            turn.record_denial(denial(Some(&format!("x{n}")), "Bash: ls"));
        }
        assert_eq!(turn.denials.len(), MAX_DENIALS);
    }

    #[test]
    fn the_latest_turns_denials_are_the_open_turns_else_the_latest_ended() {
        assert!(Status::default().latest_denials().is_empty());
        let mut ended = closed(1);
        ended.denials = vec![denial(Some("t1"), "Bash: ls")];
        let mut status = Status {
            latest: Some(ended),
            ..Status::default()
        };
        assert_eq!(status.latest_denials().len(), 1);
        status.turn = Some(open(2));
        assert!(
            status.latest_denials().is_empty(),
            "a turn without denials opened since"
        );
    }

    #[test]
    fn an_end_line_carries_the_denials_without_their_ids() {
        let mut ended = closed(3);
        ended.denials = vec![denial(Some("toolu_1"), "Bash: FOO=bar ls")];
        ended.message = Some("API Error: 400\nwhole".to_owned());
        let line = String::from_utf8(TurnRecord::end(&ended).to_line().unwrap()).unwrap();
        assert!(
            line.ends_with(
                r#""error":"rate_limit","denials":[{"tool":"Bash","text":"Bash: FOO=bar ls"}],"end_observed":true}
"#
            ),
            "{line}"
        );
        assert!(
            !line.contains("whole"),
            "the whole message stays in status.json"
        );
        let status = Status {
            latest: Some(ended.clone()),
            ..Status::default()
        };
        let bytes = status_bytes(&status).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(text.contains(r#""id":"toolu_1""#), "{text}");
        assert_eq!(
            read_status(Some(&bytes)).usable().unwrap().latest,
            Some(ended)
        );
    }

    #[test]
    fn lines_and_statuses_written_before_the_denials_still_read() {
        let old_end = br#"{"schema":1,"kind":"end","turn":3,"session":"s1","prompt_id":"p1","at":20,"outcome":"finished"}"#;
        let record: TurnRecord = serde_json::from_slice(old_end).unwrap();
        assert!(record.denials.is_empty());
        let old = br#"{"schema":1,"session":"s1","turn":{"number":2,"session":"s1","prompt_id":"p2","prompt":"go","started_at":1,"phase":"working","activity":"go"},"latest":{"number":1,"session":"s1","prompt_id":"p1","prompt":"x","outcome":"failed","detail":"API Error","error":"unknown","ended_at":1},"updated_at":5}"#;
        let status = read_status(Some(old)).usable().unwrap();
        assert!(status.turn.unwrap().denials.is_empty());
        let latest = status.latest.unwrap();
        assert!(latest.denials.is_empty());
        assert_eq!(latest.message, None);
    }

    #[test]
    fn a_start_line_carries_its_sessions_transcript_and_an_older_line_reads_as_none() {
        let line = TurnRecord::start(&open(1)).with_transcript(Some(PathBuf::from("/t/s1.jsonl")));
        let text = String::from_utf8(line.to_line().unwrap()).unwrap();
        assert!(
            text.ends_with("\"transcript\":\"/t/s1.jsonl\"}\n"),
            "{text}"
        );
        let read = read_log(text.as_bytes()).unwrap();
        assert_eq!(read[0].transcript, Some(PathBuf::from("/t/s1.jsonl")));
        let without = String::from_utf8(TurnRecord::start(&open(1)).to_line().unwrap()).unwrap();
        assert!(!without.contains("transcript"), "{without}");
        let old = br#"{"schema":1,"kind":"start","turn":1,"session":"s1","prompt_id":"p1","at":10,"prompt":"old"}"#;
        assert_eq!(read_log(old).unwrap()[0].transcript, None);
    }

    #[test]
    fn a_turns_snapshots_go_on_its_lines_and_in_the_status() {
        let mut started = open(4);
        started.begin = Some("b".repeat(40));
        let line = String::from_utf8(TurnRecord::start(&started).to_line().unwrap()).unwrap();
        assert!(
            line.ends_with(&format!(
                r#""agent":"claude","begin":"{}"}}
"#,
                "b".repeat(40)
            )),
            "{line}"
        );
        let mut ended = closed(4);
        ended.begin = Some("b".repeat(40));
        ended.end = Some("e".repeat(40));
        ended.end_observed = false;
        let line = String::from_utf8(TurnRecord::end(&ended).to_line().unwrap()).unwrap();
        assert!(
            line.ends_with(&format!(
                r#""begin":"{}","end":"{}","end_observed":false}}
"#,
                "b".repeat(40),
                "e".repeat(40)
            )),
            "{line}"
        );
        assert!(!line.contains("agent"), "the agent goes on the start line");
        let status = Status {
            turn: Some(started),
            latest: Some(ended),
            snapshot: Some("b".repeat(40)),
            ..Status::default()
        };
        let bytes = status_bytes(&status).unwrap();
        assert_eq!(read_status(Some(&bytes)).usable(), Some(status));
    }

    #[test]
    fn the_whole_log_reads_every_record_skipping_a_cut_line_and_refusing_a_newer_schema() {
        let start = TurnRecord::start(&open(1)).to_line().unwrap();
        let end = TurnRecord::end(&closed(1)).to_line().unwrap();
        let log = [start, b"{\"schema\":1,\"kind\":\"st\n".to_vec(), end].concat();
        let records = read_log(&log).unwrap();
        assert_eq!(
            records.iter().map(|r| r.kind).collect::<Vec<_>>(),
            [RecordKind::Start, RecordKind::End]
        );
        assert_eq!(read_log(b"").unwrap(), []);
        let newer = br#"{"schema":2,"kind":"start","turn":2,"at":1}"#;
        assert!(read_log(newer).is_err());
    }

    #[test]
    fn lines_and_statuses_written_before_the_snapshots_still_read() {
        let old_start = br#"{"schema":1,"kind":"start","turn":3,"session":"s1","prompt_id":"p1","at":10,"prompt":"go"}"#;
        let record: TurnRecord = serde_json::from_slice(old_start).unwrap();
        assert_eq!(
            (record.agent, record.begin, record.end, record.end_observed),
            (None, None, None, None)
        );
        let old_end = br#"{"schema":1,"kind":"end","turn":3,"session":"s1","prompt_id":"p1","at":20,"outcome":"finished"}"#;
        let record: TurnRecord = serde_json::from_slice(old_end).unwrap();
        assert_eq!(record.end_observed, None);
        let old = br#"{"schema":1,"session":"s1","turn":{"number":2,"session":"s1","prompt_id":"p2","prompt":"go","started_at":1,"phase":"working","activity":"go"},"latest":{"number":1,"session":"s1","prompt_id":"p1","prompt":"x","outcome":"finished","detail":"ok","error":null,"ended_at":1},"updated_at":5}"#;
        let status = read_status(Some(old)).usable().unwrap();
        assert_eq!(status.snapshot, None);
        let open = status.turn.unwrap();
        assert_eq!((open.agent.as_str(), open.begin), ("", None));
        let latest = status.latest.unwrap();
        assert_eq!(
            (latest.begin, latest.end, latest.end_observed),
            (None, None, false)
        );
    }

    #[test]
    fn a_message_keeps_every_line_up_to_its_cap() {
        assert_eq!(
            message("\n API Error: 400\r\n{\"type\":\"error\"}\tend \u{1b}[0m\n"),
            "API Error: 400\n{\"type\":\"error\"}\tend  [0m"
        );
        assert_eq!(message(" \n "), "");
        assert_eq!(
            message(&"x".repeat(MAX_MESSAGE_CHARS + 9)).chars().count(),
            MAX_MESSAGE_CHARS
        );
    }

    #[test]
    fn texts_are_kept_to_one_capped_line() {
        assert_eq!(one_line("\n  \n  first line  \nsecond"), "first line");
        assert_eq!(one_line("   \n"), "");
        assert_eq!(cap("a\tb"), "a b");
        let long = "é".repeat(MAX_TEXT_CHARS + 50);
        let capped = one_line(&long);
        assert_eq!(capped.chars().count(), MAX_TEXT_CHARS);
        assert!(long.starts_with(&capped), "cut on a character boundary");
    }
}
