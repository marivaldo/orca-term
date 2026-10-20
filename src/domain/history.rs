//! Domain: a worktree's turns as `turn ls` and `turn diff` show them (#38), gathered from the turn
//! log (`turns.jsonl`, `domain/turn.rs`): one entry per turn, its start and end lines merged,
//! oldest first. The open turn is told apart by the live status, which also says whether it
//! waits for the person.
//!
//! A turn's diff runs from its begin snapshot to its end snapshot (`domain/snapshot.rs`). A turn
//! recorded before the snapshots existed has neither, and has no diff.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::domain::turn::{Outcome, Phase, RecordKind, Status, TurnRecord};

/// One turn of a worktree, as its turn log and the live status tell it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LoggedTurn {
    pub(crate) number: u64,
    /// The turn's agent, from its start line; `None` for a turn logged before it was recorded.
    pub(crate) agent: Option<String>,
    pub(crate) session: Option<String>,
    /// The first line of the prompt that started it; empty when its start line is missing.
    pub(crate) prompt: String,
    /// When it started, `None` when its start line is missing.
    pub(crate) started_at: Option<u64>,
    /// When it ended, `None` while it is open.
    pub(crate) ended_at: Option<u64>,
    pub(crate) progress: Progress,
    /// Whether its end was observed; `None` while it is open, and for a turn ended before the
    /// snapshots existed.
    pub(crate) end_observed: Option<bool>,
    /// The snapshot it began on, `None` when it was not taken.
    pub(crate) begin: Option<String>,
    /// The snapshot it ended on, `None` while it is open or when it was not taken.
    pub(crate) end: Option<String>,
    /// How many tool calls its agent was denied during it.
    pub(crate) denials: usize,
}

/// Where a turn is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Progress {
    /// It ended, as the outcome says.
    Ended(Outcome),
    /// It is the open turn, its agent working or waiting.
    Open(Phase),
    /// It has a start line and no end line, but it is not the open turn: its end was never
    /// logged.
    Unended,
}

/// A worktree's turns, and where it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct History {
    /// The worktree's name.
    pub(crate) name: String,
    /// The worktree's path.
    pub(crate) path: PathBuf,
    /// Its turns, oldest first.
    pub(crate) turns: Vec<LoggedTurn>,
}

/// The turns the turn log `records` holds, oldest first, the open one told apart by the live
/// status `status` (which also counts its denials so far).
pub(crate) fn turns(records: &[TurnRecord], status: Option<&Status>) -> Vec<LoggedTurn> {
    let mut by_number: BTreeMap<u64, LoggedTurn> = BTreeMap::new();
    for record in records {
        let turn = by_number
            .entry(record.turn)
            .or_insert_with(|| empty(record.turn));
        match record.kind {
            RecordKind::Start => apply_start(turn, record),
            RecordKind::End => apply_end(turn, record),
        }
    }
    let open = status.and_then(|status| status.turn.as_ref());
    for turn in by_number.values_mut() {
        if let (Progress::Unended, Some(open)) = (turn.progress, open)
            && open.number == turn.number
        {
            turn.progress = Progress::Open(open.phase);
            turn.denials = open.denials.len();
        }
    }
    by_number.into_values().collect()
}

/// A turn nothing is known about yet but its number.
fn empty(number: u64) -> LoggedTurn {
    LoggedTurn {
        number,
        agent: None,
        session: None,
        prompt: String::new(),
        started_at: None,
        ended_at: None,
        progress: Progress::Unended,
        end_observed: None,
        begin: None,
        end: None,
        denials: 0,
    }
}

/// What the start line `record` says of its turn.
fn apply_start(turn: &mut LoggedTurn, record: &TurnRecord) {
    turn.agent.clone_from(&record.agent);
    turn.session.clone_from(&record.session);
    turn.prompt = record.prompt.clone().unwrap_or_default();
    turn.started_at = Some(record.at);
    if turn.begin.is_none() {
        turn.begin.clone_from(&record.begin);
    }
}

/// What the end line `record` says of its turn. The end line repeats the begin snapshot.
fn apply_end(turn: &mut LoggedTurn, record: &TurnRecord) {
    if turn.session.is_none() {
        turn.session.clone_from(&record.session);
    }
    turn.ended_at = Some(record.at);
    if let Some(outcome) = record.outcome {
        turn.progress = Progress::Ended(outcome);
    }
    turn.end_observed = record.end_observed;
    if record.begin.is_some() {
        turn.begin.clone_from(&record.begin);
    }
    turn.end.clone_from(&record.end);
    turn.denials = record.denials.len();
}

/// The two snapshots a turn's diff runs between.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DiffBounds {
    pub(crate) begin: String,
    pub(crate) end: String,
}

/// What `turn diff` found: what one turn changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TurnDiff {
    /// The worktree's name.
    pub(crate) name: String,
    /// The worktree's path.
    pub(crate) path: PathBuf,
    /// The turn's number.
    pub(crate) turn: u64,
    pub(crate) bounds: DiffBounds,
    /// Whether the turn's end was observed: when it was not, its end snapshot was taken by
    /// whatever told the core it was over, so the patch may hold edits made after the
    /// interruption. `None` when the turn log does not say.
    pub(crate) end_observed: Option<bool>,
    /// `git diff <begin> <end>`, as git printed it.
    pub(crate) patch: Vec<u8>,
}

/// The snapshots turn `number` of `turns` ran between. Refuses a turn that does not exist, one
/// that has not ended, and one with a snapshot missing.
pub(crate) fn diff_bounds(turns: &[LoggedTurn], number: u64) -> Result<DiffBounds, TurnError> {
    let turn = find(turns, number)?;
    if !matches!(turn.progress, Progress::Ended(_)) {
        return Err(TurnError::NotEnded(number));
    }
    match (&turn.begin, &turn.end) {
        (Some(begin), Some(end)) => Ok(DiffBounds {
            begin: begin.clone(),
            end: end.clone(),
        }),
        (None, None) => Err(TurnError::BeforeSnapshots(number)),
        (None, Some(_)) => Err(TurnError::SnapshotMissing {
            turn: number,
            which: "begin",
        }),
        (Some(_), None) => Err(TurnError::SnapshotMissing {
            turn: number,
            which: "end",
        }),
    }
}

/// Turn `number` of `turns`.
pub(crate) fn find(turns: &[LoggedTurn], number: u64) -> Result<&LoggedTurn, TurnError> {
    turns
        .iter()
        .find(|turn| turn.number == number)
        .ok_or(TurnError::NoSuchTurn(number))
}

/// Why a turn command refuses a turn.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum TurnError {
    #[error("there is no turn {0}")]
    NoSuchTurn(u64),
    #[error("turn {0} has not ended yet")]
    NotEnded(u64),
    #[error("turn {0} ran before orca-term took snapshots")]
    BeforeSnapshots(u64),
    #[error("turn {turn} has no {which} snapshot: it could not be taken (hook.log says why)")]
    SnapshotMissing { turn: u64, which: &'static str },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(lines: &[&str]) -> Vec<TurnRecord> {
        lines
            .iter()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    const OLD_TURN: &str = r#"{"schema":1,"kind":"start","turn":1,"session":"s1","prompt_id":"p1","at":10,"prompt":"old"}"#;
    const OLD_END: &str = r#"{"schema":1,"kind":"end","turn":1,"session":"s1","prompt_id":"p1","at":20,"outcome":"finished"}"#;
    const START_2: &str = r#"{"schema":1,"kind":"start","turn":2,"session":"s1","prompt_id":"p2","at":30,"prompt":"new","agent":"claude","begin":"b2"}"#;
    const END_2: &str = r#"{"schema":1,"kind":"end","turn":2,"session":"s1","prompt_id":"p2","at":40,"outcome":"interrupted","denials":[{"tool":"Bash","text":"Bash: ls"}],"begin":"b2","end":"e2","end_observed":false}"#;
    const START_3: &str = r#"{"schema":1,"kind":"start","turn":3,"session":"s1","prompt_id":"p3","at":50,"prompt":"open","agent":"claude","begin":"e2"}"#;

    fn status_open(number: u64, phase: Phase) -> Status {
        let open = r#"{"number":0,"session":"s1","prompt_id":"p","prompt":"open","started_at":50,"phase":"working","activity":"x","denials":[{"tool":"Write","text":"Write: a"}]}"#;
        let mut turn: crate::domain::turn::OpenTurn = serde_json::from_str(open).unwrap();
        turn.number = number;
        turn.phase = phase;
        Status {
            turn: Some(turn),
            ..Status::default()
        }
    }

    #[test]
    fn the_log_gives_one_turn_per_number_oldest_first_the_open_one_from_the_status() {
        let status = status_open(3, Phase::Waiting);
        let turns = turns(
            &log(&[OLD_TURN, OLD_END, START_2, END_2, START_3]),
            Some(&status),
        );
        assert_eq!(turns.len(), 3);
        let old = &turns[0];
        assert_eq!(
            (old.agent.as_deref(), old.begin.as_deref(), old.end_observed),
            (None, None, None),
            "a turn from before the snapshots"
        );
        assert_eq!(old.progress, Progress::Ended(Outcome::Finished));
        let second = &turns[1];
        assert_eq!(second.progress, Progress::Ended(Outcome::Interrupted));
        assert_eq!(
            (second.begin.as_deref(), second.end.as_deref()),
            (Some("b2"), Some("e2"))
        );
        assert_eq!((second.end_observed, second.denials), (Some(false), 1));
        assert_eq!((second.started_at, second.ended_at), (Some(30), Some(40)));
        let open = &turns[2];
        assert_eq!(open.progress, Progress::Open(Phase::Waiting));
        assert_eq!((open.ended_at, open.denials), (None, 1), "denials so far");
        assert_eq!(open.agent.as_deref(), Some("claude"));
    }

    #[test]
    fn a_turn_with_no_end_that_is_not_the_open_one_is_unended() {
        let turns = turns(&log(&[START_2]), None);
        assert_eq!(turns[0].progress, Progress::Unended);
    }

    #[test]
    fn a_turns_diff_needs_it_ended_with_both_snapshots() {
        let status = status_open(3, Phase::Working);
        let turns = turns(
            &log(&[OLD_TURN, OLD_END, START_2, END_2, START_3]),
            Some(&status),
        );
        assert_eq!(
            diff_bounds(&turns, 2),
            Ok(DiffBounds {
                begin: "b2".to_owned(),
                end: "e2".to_owned()
            })
        );
        assert_eq!(diff_bounds(&turns, 9), Err(TurnError::NoSuchTurn(9)));
        assert_eq!(diff_bounds(&turns, 3), Err(TurnError::NotEnded(3)));
        assert_eq!(diff_bounds(&turns, 1), Err(TurnError::BeforeSnapshots(1)));
        assert_eq!(
            TurnError::NotEnded(3).to_string(),
            "turn 3 has not ended yet"
        );
        assert_eq!(
            TurnError::BeforeSnapshots(1).to_string(),
            "turn 1 ran before orca-term took snapshots"
        );
        let no_end = r#"{"schema":1,"kind":"end","turn":2,"session":"s1","prompt_id":"p2","at":40,"outcome":"finished","begin":"b2","end_observed":true}"#;
        let turns = turns_of(&[START_2, no_end]);
        assert_eq!(
            diff_bounds(&turns, 2),
            Err(TurnError::SnapshotMissing {
                turn: 2,
                which: "end"
            })
        );
    }

    fn turns_of(lines: &[&str]) -> Vec<LoggedTurn> {
        turns(&log(lines), None)
    }
}
