//! Domain: a worktree's turns as the agent's hooks report them (ADR 0009), kept in its admin
//! directory beside `worktree.json`, in files only the hook writes:
//!
//! - `status.json`, the live status: the current session, the open turn, the latest closed turn.
//!   The hook replaces it whole on every run (write and rename).
//! - `turns.jsonl`, the turn log: one line when a turn starts and one when it ends, only ever
//!   appended to.
//!
//! Both carry `schema`, as every state file does (ADR 0008). Reading tolerates: a file this core
//! cannot use reads as absent, so `worktree ls` never fails on it. The hook refuses to write over
//! a schema newer than this core supports.
//!
//! Times are Unix milliseconds. Turns are numbered across the whole worktree, never per session.

use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// The newest schema of `status.json` this core reads and writes.
pub(crate) const STATUS_SCHEMA: u64 = 1;

/// The newest schema of a `turns.jsonl` line this core reads and writes.
pub(crate) const TURNS_SCHEMA: u64 = 1;

/// The most characters a stored text (a prompt, an activity, a detail) keeps.
pub(crate) const MAX_TEXT_CHARS: usize = 300;

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
    #[serde(default)]
    pub(crate) updated_at: u64,
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

/// One line of `turns.jsonl`. A start carries the prompt; an end carries the outcome, the
/// detail and the error.
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
        }
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
        if let Some(schema) = record.schema
            && schema > TURNS_SCHEMA
        {
            bail!(
                "turns.jsonl has schema {schema}, newer than the schema {TURNS_SCHEMA} this \
                 orca-term supports"
            );
        }
        return Ok(Some(record.turn));
    }
    Ok(None)
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
        }
    }

    #[test]
    fn the_status_is_written_with_its_schema_and_read_back() {
        let status = Status {
            session: Some("s1".to_owned()),
            transcript: Some(PathBuf::from("/home/me/.claude/projects/-w-fix/s1.jsonl")),
            turn: Some(open(2)),
            queued: Some("and then".to_owned()),
            latest: Some(closed(1)),
            updated_at: 30,
        };
        let bytes = status_bytes(&status).unwrap();
        let text = String::from_utf8(bytes.clone()).unwrap();
        assert!(
            text.starts_with(
                r#"{"schema":1,"session":"s1","transcript":"/home/me/.claude/projects/-w-fix/s1.jsonl","turn":{"number":2,"#
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
            "{\"schema\":1,\"kind\":\"start\",\"turn\":3,\"session\":\"s1\",\"prompt_id\":\"p1\",\"at\":10,\"prompt\":\"fix it\"}\n"
        );
        let end = TurnRecord::end(&closed(3)).to_line().unwrap();
        assert_eq!(
            String::from_utf8(end.clone()).unwrap(),
            "{\"schema\":1,\"kind\":\"end\",\"turn\":3,\"session\":\"s1\",\"prompt_id\":\"p1\",\"at\":20,\"outcome\":\"failed\",\"detail\":\"API Error\",\"error\":\"rate_limit\"}\n"
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
