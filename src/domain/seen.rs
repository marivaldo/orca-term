//! Domain: the unseen mark (`CONTEXT.md`, **Unseen**). A worktree is unseen when its latest turn
//! ended after the person last reviewed it, whatever the outcome, or when the person marked it so
//! by hand. The mark is independent of the worktree's state.
//!
//! What the person has seen is kept in `seen.json`, in the worktree's admin directory beside
//! `status.json`: the number of the latest turn the person has seen, and the mark set by hand.
//! Only the person's commands (`worktree seen`, `worktree unseen`) write it, replacing it whole
//! (write and rename); the hooks never touch it. It takes no lock: those commands are its only
//! writers, and the last write wins.
//!
//! Whether a worktree is unseen is derived on read, from this record and the hooks' live status.
//! Like every state file it carries `schema`: a file this core cannot use reads as nothing seen and
//! no mark, so `worktree ls` never fails on it, and a schema newer than this core supports is never
//! written over.

use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::domain::agent::HolderStatus;
use crate::domain::turn::Status;

/// The newest schema of `seen.json` this core reads and writes.
pub(crate) const SEEN_SCHEMA: u64 = 1;

/// What the person has seen of a worktree, as `seen.json` records it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Seen {
    /// The number of the latest turn the person has seen; 0 when none.
    #[serde(default)]
    pub(crate) seen_through: u64,
    /// Set by `worktree unseen`: the worktree is unseen whatever its turns, until `worktree seen`.
    #[serde(default)]
    pub(crate) marked: bool,
}

/// `seen.json` as this core writes it: the record with its schema.
#[derive(Debug, Serialize)]
struct SeenFile<'a> {
    schema: u64,
    #[serde(flatten)]
    seen: &'a Seen,
}

/// Only the schema of `seen.json`, read before anything else.
#[derive(Debug, Deserialize)]
struct SchemaOnly {
    #[serde(default)]
    schema: Option<u64>,
}

/// What reading `seen.json` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SeenRead {
    /// No file: the person never marked the worktree seen or unseen.
    Missing,
    /// A record this core can use.
    Usable(Seen),
    /// A file whose schema is newer than this core supports.
    NewerSchema(u64),
    /// A file that is not a record, for the reason given.
    Unreadable(String),
}

impl SeenRead {
    /// The record, or nothing seen and no mark when there is none this core can use.
    pub(crate) fn usable(self) -> Seen {
        match self {
            Self::Usable(seen) => seen,
            Self::Missing | Self::NewerSchema(_) | Self::Unreadable(_) => Seen::default(),
        }
    }
}

/// What `seen.json`, whose content is `bytes` (`None` when it does not exist), holds.
pub(crate) fn read(bytes: Option<&[u8]>) -> SeenRead {
    let Some(bytes) = bytes else {
        return SeenRead::Missing;
    };
    match serde_json::from_slice::<SchemaOnly>(bytes) {
        Ok(SchemaOnly {
            schema: Some(schema),
        }) if schema > SEEN_SCHEMA => return SeenRead::NewerSchema(schema),
        Ok(_) => {}
        Err(err) => return SeenRead::Unreadable(err.to_string()),
    }
    match serde_json::from_slice::<Seen>(bytes) {
        Ok(seen) => SeenRead::Usable(seen),
        Err(err) => SeenRead::Unreadable(err.to_string()),
    }
}

/// The bytes written to `seen.json`.
pub(crate) fn to_bytes(seen: &Seen) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec(&SeenFile {
        schema: SEEN_SCHEMA,
        seen,
    })?)
}

/// Refuses to write over a `seen.json` whose schema is newer than this core supports. A missing
/// or unreadable file is written over.
pub(crate) fn check_overwrite(read: &SeenRead) -> Result<()> {
    if let SeenRead::NewerSchema(schema) = read {
        bail!(
            "seen.json has schema {schema}, newer than the schema {SEEN_SCHEMA} this orca-term \
             supports; upgrade orca-term to change this worktree"
        );
    }
    Ok(())
}

/// The number of the worktree's latest turn that ended, from the hooks' live status (`None` when
/// there is none this core can use) and whether its holder is alive: the latest closed turn, or
/// the open turn when its holder is gone, since an open turn whose agent died has ended (it shows
/// as interrupted). `None` when no turn has ended.
pub(crate) fn latest_ended(status: Option<&Status>, holder: HolderStatus) -> Option<u64> {
    let status = status?;
    match (&status.turn, holder) {
        (Some(open), HolderStatus::Gone) => Some(open.number),
        _ => status.latest.as_ref().map(|closed| closed.number),
    }
}

/// Whether a worktree is unseen: marked by hand, or its latest ended turn, `ended`, is one the
/// person has not seen. A worktree with no ended turn and no mark is not unseen.
pub(crate) fn is_unseen(seen: Seen, ended: Option<u64>) -> bool {
    seen.marked || ended.is_some_and(|number| number > seen.seen_through)
}

/// What the person asks for: `worktree seen` or `worktree unseen`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mark {
    /// The person has seen the worktree: everything that ended so far, and no mark.
    Seen,
    /// The person marks the worktree unseen by hand, keeping what they have seen.
    Unseen,
}

impl Mark {
    /// The record to write, from the one there was, `current`, and the number of the latest turn
    /// that ended, `ended`. Doing it twice gives the same record.
    pub(crate) fn apply(self, current: Seen, ended: Option<u64>) -> Seen {
        match self {
            Self::Seen => Seen {
                seen_through: ended.unwrap_or(0),
                marked: false,
            },
            Self::Unseen => Seen {
                marked: true,
                ..current
            },
        }
    }
}

/// What `worktree seen` and `worktree unseen` did: the worktree, and whether it is unseen now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Marked {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) unseen: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::turn::{ClosedTurn, OpenTurn, Outcome, Phase};

    fn open(number: u64) -> OpenTurn {
        OpenTurn {
            number,
            session: Some("s1".to_owned()),
            prompt_id: Some("p".to_owned()),
            prompt: "fix it".to_owned(),
            started_at: 1,
            phase: Phase::Working,
            activity: "fix it".to_owned(),
            denials: Vec::new(),
            agent: "claude".to_owned(),
            begin: None,
        }
    }

    fn closed(number: u64, outcome: Outcome) -> ClosedTurn {
        ClosedTurn {
            number,
            session: Some("s1".to_owned()),
            prompt_id: Some("p".to_owned()),
            prompt: "fix it".to_owned(),
            outcome,
            detail: None,
            error: None,
            ended_at: 2,
            denials: Vec::new(),
            message: None,
            begin: None,
            end: None,
            end_observed: false,
        }
    }

    /// A status whose latest closed turn is `number`, ended as `outcome`.
    fn ended(number: u64, outcome: Outcome) -> Status {
        Status {
            session: Some("s1".to_owned()),
            latest: Some(closed(number, outcome)),
            ..Status::default()
        }
    }

    fn seen(seen_through: u64, marked: bool) -> Seen {
        Seen {
            seen_through,
            marked,
        }
    }

    /// Whether a worktree with `status` and `holder` reads as unseen under the record `record`.
    fn unseen(record: Seen, status: Option<&Status>, holder: HolderStatus) -> bool {
        is_unseen(record, latest_ended(status, holder))
    }

    #[test]
    fn a_worktree_with_no_ended_turn_and_no_mark_is_not_unseen() {
        let nothing = Seen::default();
        assert!(!unseen(nothing, None, HolderStatus::Alive));
        assert!(!unseen(
            nothing,
            Some(&Status::default()),
            HolderStatus::Gone
        ));
        let working = Status {
            turn: Some(open(1)),
            ..Status::default()
        };
        assert!(
            !unseen(nothing, Some(&working), HolderStatus::Alive),
            "a turn in progress has not ended"
        );
    }

    #[test]
    fn a_turn_that_ended_after_the_last_review_is_unseen_whatever_its_outcome() {
        for outcome in [Outcome::Finished, Outcome::Failed, Outcome::Interrupted] {
            for holder in [HolderStatus::Alive, HolderStatus::Gone] {
                let status = ended(3, outcome);
                assert!(unseen(seen(2, false), Some(&status), holder), "{outcome:?}");
                assert!(unseen(seen(0, false), Some(&status), holder), "{outcome:?}");
                assert!(
                    !unseen(seen(3, false), Some(&status), holder),
                    "seen through it: {outcome:?}"
                );
                assert!(
                    !unseen(seen(5, false), Some(&status), holder),
                    "seen past it: {outcome:?}"
                );
            }
        }
    }

    #[test]
    fn an_open_turn_whose_holder_is_gone_has_ended() {
        let status = Status {
            turn: Some(open(4)),
            latest: Some(closed(3, Outcome::Finished)),
            ..Status::default()
        };
        assert_eq!(latest_ended(Some(&status), HolderStatus::Gone), Some(4));
        assert_eq!(
            latest_ended(Some(&status), HolderStatus::Alive),
            Some(3),
            "while the holder lives, the latest closed turn"
        );
        assert!(unseen(seen(3, false), Some(&status), HolderStatus::Gone));
        assert!(!unseen(seen(3, false), Some(&status), HolderStatus::Alive));
        assert!(!unseen(seen(4, false), Some(&status), HolderStatus::Gone));
    }

    #[test]
    fn a_fresh_session_keeps_the_mark() {
        // A new session reads idle, but the turn of the earlier session still ended unseen.
        let status = Status {
            session: Some("s2".to_owned()),
            latest: Some(closed(1, Outcome::Finished)),
            ..Status::default()
        };
        assert!(unseen(seen(0, false), Some(&status), HolderStatus::Alive));
        assert!(unseen(seen(1, true), Some(&status), HolderStatus::Alive));
        assert!(!unseen(seen(1, false), Some(&status), HolderStatus::Alive));
    }

    #[test]
    fn a_mark_makes_a_worktree_unseen_with_nothing_ended() {
        assert!(unseen(seen(0, true), None, HolderStatus::Gone));
        assert!(unseen(
            seen(3, true),
            Some(&ended(3, Outcome::Finished)),
            HolderStatus::Alive
        ));
    }

    #[test]
    fn seen_takes_everything_ended_and_clears_the_mark_and_unseen_keeps_it() {
        assert_eq!(Mark::Seen.apply(seen(1, true), Some(4)), seen(4, false));
        assert_eq!(Mark::Seen.apply(seen(1, true), None), seen(0, false));
        assert_eq!(Mark::Unseen.apply(seen(4, false), Some(6)), seen(4, true));
        for mark in [Mark::Seen, Mark::Unseen] {
            let once = mark.apply(seen(2, false), Some(3));
            assert_eq!(mark.apply(once, Some(3)), once, "{mark:?} is idempotent");
        }
        assert!(!is_unseen(
            Mark::Seen.apply(seen(0, true), Some(9)),
            Some(9)
        ));
        assert!(is_unseen(
            Mark::Unseen.apply(seen(9, false), Some(9)),
            Some(9)
        ));
    }

    #[test]
    fn the_record_is_written_with_its_schema_and_read_back() {
        let bytes = to_bytes(&seen(7, true)).unwrap();
        assert_eq!(bytes, br#"{"schema":1,"seen_through":7,"marked":true}"#);
        assert_eq!(read(Some(&bytes)), SeenRead::Usable(seen(7, true)));
        assert_eq!(
            read(Some(br#"{"schema":1,"future":[1]}"#)),
            SeenRead::Usable(Seen::default()),
            "unknown fields are ignored and missing ones read as nothing"
        );
    }

    #[test]
    fn a_record_this_core_cannot_use_reads_as_nothing_seen_and_no_mark() {
        assert_eq!(read(None), SeenRead::Missing);
        assert_eq!(read(None).usable(), Seen::default());
        assert_eq!(
            read(Some(br#"{"schema":2,"marked":true}"#)),
            SeenRead::NewerSchema(2)
        );
        assert_eq!(
            read(Some(br#"{"schema":2,"marked":true}"#)).usable(),
            Seen::default()
        );
        assert!(matches!(read(Some(b"{not json")), SeenRead::Unreadable(_)));
        assert_eq!(read(Some(b"{not json")).usable(), Seen::default());
        assert!(matches!(
            read(Some(br#"{"schema":1,"marked":"yes"}"#)),
            SeenRead::Unreadable(_)
        ));
    }

    #[test]
    fn only_a_newer_record_refuses_to_be_overwritten() {
        let err = check_overwrite(&SeenRead::NewerSchema(2)).unwrap_err();
        assert!(err.to_string().contains("schema 2"), "{err}");
        check_overwrite(&SeenRead::Missing).unwrap();
        check_overwrite(&SeenRead::Unreadable("x".to_owned())).unwrap();
        check_overwrite(&SeenRead::Usable(Seen::default())).unwrap();
    }
}
