//! Domain: the snapshots that bound a worktree's turns (ADR 0006, #38).
//!
//! A snapshot is the worktree's whole tree (tracked and untracked files, `.gitignore` respected)
//! as an unsigned synthetic commit, taken by the hook of an event that opens or closes a turn,
//! never by any other. One snapshot is the end of the turn the event closed and the begin of the
//! turn it opened, whichever of the two happened: a prompt that interrupts a turn takes one
//! snapshot that is both. A turn's diff is `begin..end`.
//!
//! The snapshots of a worktree form one chain, each the parent of the next, held by the ref
//! `refs/orca-term/worktrees/<id>`, `<id>` being the worktree's git admin directory
//! (`.git/worktrees/<id>`). The ref is shared by every worktree so that a `git gc` run anywhere
//! keeps the chain. `status.json` records the chain's tip: a ref that does not match it belongs to
//! a previous worktree (git reuses a freed id) and is replaced by a snapshot with no parent.
//!
//! A snapshot that ends a turn is authored by the turn's agent; one that only begins a turn holds
//! the person's edits since the previous snapshot, and is authored by the person. So `git blame`
//! over the chain says which turn, or the person, wrote each line. The turn log, not the commit
//! message, is the source of truth for which turn a commit bounds.

use std::path::{Path, PathBuf};

use crate::domain::state::GIT_WORKTREES_DIR;
use crate::domain::turn::{RecordKind, Status, TurnRecord};

/// Where every worktree's chain of snapshots lives: `refs/orca-term/worktrees/<id>`.
pub(crate) const CHAIN_REFS: &str = "refs/orca-term/worktrees/";

/// The domain of every identity the core makes up: reserved, so it reaches nobody.
const INVALID_DOMAIN: &str = "orca-term.invalid";

/// The name a person's snapshot is authored with when git has no `user.name`.
const PERSON: &str = "person";

/// The name every snapshot is committed with.
const COMMITTER: &str = "orca-term";

/// The ref holding the chain of the worktree whose git admin directory is `.git/worktrees/<id>`.
pub(crate) fn chain_ref(id: &str) -> String {
    format!("{CHAIN_REFS}{id}")
}

/// The id of the worktree whose chain the ref `name` holds, `None` for a ref outside
/// [`CHAIN_REFS`] or one nested deeper.
pub(crate) fn chain_id(name: &str) -> Option<&str> {
    let id = name.strip_prefix(CHAIN_REFS)?;
    (!id.is_empty() && !id.contains('/')).then_some(id)
}

/// git's admin directory of the worktree `id`, in the git common dir `common`: the directory whose
/// absence makes the chain an orphan.
pub(crate) fn git_admin_dir(common: &Path, id: &str) -> PathBuf {
    common.join(GIT_WORKTREES_DIR).join(id)
}

/// The turn a snapshot ends, and whether that turn's end was observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Ending {
    pub(crate) turn: u64,
    pub(crate) observed: bool,
}

/// What one snapshot bounds: the end of a turn, the begin of a turn, or both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Bounds {
    pub(crate) ends: Option<Ending>,
    pub(crate) begins: Option<u64>,
    /// The session of the turn it begins, else of the turn it ends.
    pub(crate) session: Option<String>,
}

/// What a snapshot taken after an event that wrote `records` to the turn log bounds: `None` when
/// the event neither opened nor closed a turn, so no snapshot is taken.
pub(crate) fn bounds(records: &[TurnRecord]) -> Option<Bounds> {
    let ended = records.iter().find(|r| r.kind == RecordKind::End);
    let begun = records.iter().find(|r| r.kind == RecordKind::Start);
    if ended.is_none() && begun.is_none() {
        return None;
    }
    Some(Bounds {
        ends: ended.map(|record| Ending {
            turn: record.turn,
            observed: record.end_observed.unwrap_or(false),
        }),
        begins: begun.map(|record| record.turn),
        session: begun.or(ended).and_then(|record| record.session.clone()),
    })
}

/// The snapshot's commit message: a subject saying which turns it bounds, a blank line, then
/// trailers naming the worktree `worktree`, the session when known, the turns, and whether the
/// end was observed (only on an end).
pub(crate) fn message(bounds: &Bounds, worktree: &str) -> String {
    let mut halves = Vec::new();
    if let Some(ending) = bounds.ends {
        halves.push(format!("end of turn {}", ending.turn));
    }
    if let Some(turn) = bounds.begins {
        halves.push(format!("begin of turn {turn}"));
    }
    let mut lines = vec![
        format!("orca-term: {}", halves.join(", ")),
        String::new(),
        format!("Orca-Term-Worktree: {worktree}"),
    ];
    if let Some(session) = &bounds.session {
        lines.push(format!("Orca-Term-Session: {session}"));
    }
    if let Some(ending) = bounds.ends {
        lines.push(format!("Orca-Term-Ends: {}", ending.turn));
    }
    if let Some(turn) = bounds.begins {
        lines.push(format!("Orca-Term-Begins: {turn}"));
    }
    if let Some(ending) = bounds.ends {
        lines.push(format!("Orca-Term-End-Observed: {}", ending.observed));
    }
    lines.join("\n") + "\n"
}

/// A name and an email, as git records an author or a committer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Identity {
    pub(crate) name: String,
    pub(crate) email: String,
}

impl Identity {
    /// The identity of the agent named `agent`: `claude <claude@orca-term.invalid>`.
    pub(crate) fn agent(agent: &str) -> Self {
        Self {
            name: agent.to_owned(),
            email: format!("{agent}@{INVALID_DOMAIN}"),
        }
    }

    /// The person, from git's `user.name` and `user.email`, each falling back to
    /// `person <person@orca-term.invalid>` when unset or blank.
    pub(crate) fn person(name: Option<String>, email: Option<String>) -> Self {
        let given = |value: Option<String>| value.filter(|text| !text.trim().is_empty());
        Self {
            name: given(name).unwrap_or_else(|| PERSON.to_owned()),
            email: given(email).unwrap_or_else(|| format!("{PERSON}@{INVALID_DOMAIN}")),
        }
    }

    /// The committer of every snapshot: `orca-term <orca-term@orca-term.invalid>`.
    pub(crate) fn committer() -> Self {
        Self {
            name: COMMITTER.to_owned(),
            email: format!("{COMMITTER}@{INVALID_DOMAIN}"),
        }
    }
}

/// Whether the snapshot is the agent's: it ends a turn, so it holds the turn's edits. One that
/// only begins a turn holds the person's edits since the previous snapshot.
pub(crate) fn authored_by_agent(bounds: &Bounds) -> bool {
    bounds.ends.is_some()
}

/// The time `millis` (Unix milliseconds) in git's internal date format, in UTC.
pub(crate) fn git_date(millis: u64) -> String {
    format!("{} +0000", millis / 1000)
}

/// How a new snapshot joins the chain: its parent, and what the ref must still hold when it is
/// moved to the new snapshot (`None`: the ref must not exist).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChainStep {
    pub(crate) parent: Option<String>,
    pub(crate) expected: Option<String>,
}

/// How a new snapshot joins the chain whose tip `status.json` records as `recorded`, when the ref
/// holds `current`. The ref's commit is the parent only when it is the recorded tip. A ref that
/// holds anything else (no tip recorded, or another one) is stale, a previous worktree's chain
/// under a reused id: the new snapshot has no parent and replaces it. No ref, no parent.
pub(crate) fn chain_step(recorded: Option<&str>, current: Option<&str>) -> ChainStep {
    let parent = match (recorded, current) {
        (Some(recorded), Some(current)) if recorded == current => Some(current.to_owned()),
        _ => None,
    };
    ChainStep {
        parent,
        expected: current.map(str::to_owned),
    }
}

/// Records the snapshot `id` in what the event wrote: the begin of the turn a start line opens
/// (on the line and on the open turn), the end of the turn an end line closes (on the line and on
/// the latest closed turn), and the chain's tip in the status.
pub(crate) fn fill(records: &mut [TurnRecord], status: &mut Status, id: &str) {
    for record in records.iter_mut() {
        match record.kind {
            RecordKind::Start => {
                record.begin = Some(id.to_owned());
                if let Some(open) = status.turn.as_mut().filter(|t| t.number == record.turn) {
                    open.begin = Some(id.to_owned());
                }
            }
            RecordKind::End => {
                record.end = Some(id.to_owned());
                if let Some(closed) = status.latest.as_mut().filter(|t| t.number == record.turn) {
                    closed.end = Some(id.to_owned());
                }
            }
        }
    }
    status.snapshot = Some(id.to_owned());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::turn::{ClosedTurn, OpenTurn, Outcome, Phase};

    const BEGIN: &str = "1111111111111111111111111111111111111111";
    const SNAP: &str = "2222222222222222222222222222222222222222";

    fn open(number: u64, begin: Option<&str>) -> OpenTurn {
        OpenTurn {
            number,
            session: Some("s1".to_owned()),
            prompt_id: Some(format!("p{number}")),
            prompt: "go".to_owned(),
            started_at: 1,
            phase: Phase::Working,
            activity: "go".to_owned(),
            denials: Vec::new(),
            agent: "claude".to_owned(),
            begin: begin.map(str::to_owned),
        }
    }

    fn closed(number: u64, outcome: Outcome, begin: Option<&str>) -> ClosedTurn {
        ClosedTurn {
            number,
            session: Some("s1".to_owned()),
            prompt_id: Some(format!("p{number}")),
            prompt: "go".to_owned(),
            outcome,
            detail: None,
            error: None,
            ended_at: 2,
            denials: Vec::new(),
            message: None,
            begin: begin.map(str::to_owned),
            end: None,
            end_observed: outcome != Outcome::Interrupted,
        }
    }

    #[test]
    fn only_an_event_that_opens_or_closes_a_turn_takes_a_snapshot() {
        assert_eq!(bounds(&[]), None);
        let start = TurnRecord::start(&open(3, None));
        let end = TurnRecord::end(&closed(2, Outcome::Interrupted, Some(BEGIN)));
        assert_eq!(
            bounds(&[end.clone(), start.clone()]),
            Some(Bounds {
                ends: Some(Ending {
                    turn: 2,
                    observed: false
                }),
                begins: Some(3),
                session: Some("s1".to_owned()),
            })
        );
        let finished = TurnRecord::end(&closed(2, Outcome::Finished, Some(BEGIN)));
        assert_eq!(
            bounds(&[finished]).unwrap().ends,
            Some(Ending {
                turn: 2,
                observed: true
            })
        );
        let begun = bounds(&[start]).unwrap();
        assert_eq!((begun.ends, begun.begins), (None, Some(3)));
    }

    #[test]
    fn the_message_names_what_the_snapshot_bounds_in_its_subject_and_trailers() {
        let both = Bounds {
            ends: Some(Ending {
                turn: 2,
                observed: false,
            }),
            begins: Some(3),
            session: Some("s1".to_owned()),
        };
        assert_eq!(
            message(&both, "fix"),
            "orca-term: end of turn 2, begin of turn 3\n\nOrca-Term-Worktree: fix\n\
             Orca-Term-Session: s1\nOrca-Term-Ends: 2\nOrca-Term-Begins: 3\n\
             Orca-Term-End-Observed: false\n"
        );
        let end = Bounds {
            begins: None,
            ends: Some(Ending {
                turn: 2,
                observed: true,
            }),
            session: None,
        };
        assert_eq!(
            message(&end, "fix"),
            "orca-term: end of turn 2\n\nOrca-Term-Worktree: fix\nOrca-Term-Ends: 2\n\
             Orca-Term-End-Observed: true\n"
        );
        let begin = Bounds { ends: None, ..both };
        assert_eq!(
            message(&begin, "fix"),
            "orca-term: begin of turn 3\n\nOrca-Term-Worktree: fix\nOrca-Term-Session: s1\n\
             Orca-Term-Begins: 3\n"
        );
    }

    #[test]
    fn the_ref_is_the_parent_only_when_it_is_the_recorded_tip() {
        assert_eq!(
            chain_step(None, None),
            ChainStep {
                parent: None,
                expected: None
            },
            "no ref: no parent, and the ref must still not exist"
        );
        assert_eq!(
            chain_step(Some(BEGIN), None),
            ChainStep {
                parent: None,
                expected: None
            },
            "a recorded tip whose ref is gone starts again"
        );
        assert_eq!(
            chain_step(Some(BEGIN), Some(BEGIN)),
            ChainStep {
                parent: Some(BEGIN.to_owned()),
                expected: Some(BEGIN.to_owned())
            }
        );
        let stale = ChainStep {
            parent: None,
            expected: Some(SNAP.to_owned()),
        };
        assert_eq!(
            chain_step(None, Some(SNAP)),
            stale,
            "a ref with no recorded tip is a previous worktree's: replaced"
        );
        assert_eq!(
            chain_step(Some(BEGIN), Some(SNAP)),
            stale,
            "a ref with another tip is replaced"
        );
    }

    #[test]
    fn a_begin_snapshot_fills_the_start_line_the_open_turn_and_the_tip() {
        let mut status = Status {
            turn: Some(open(1, None)),
            ..Status::default()
        };
        let mut records = vec![TurnRecord::start(&open(1, None))];
        fill(&mut records, &mut status, SNAP);
        assert_eq!(records[0].begin.as_deref(), Some(SNAP));
        assert_eq!(status.turn.unwrap().begin.as_deref(), Some(SNAP));
        assert_eq!(status.snapshot.as_deref(), Some(SNAP));
    }

    #[test]
    fn an_end_snapshot_fills_the_end_line_and_the_latest_turn_keeping_its_begin() {
        let ended = closed(1, Outcome::Finished, Some(BEGIN));
        let mut status = Status {
            latest: Some(ended.clone()),
            snapshot: Some(BEGIN.to_owned()),
            ..Status::default()
        };
        let mut records = vec![TurnRecord::end(&ended)];
        fill(&mut records, &mut status, SNAP);
        assert_eq!(
            (records[0].begin.as_deref(), records[0].end.as_deref()),
            (Some(BEGIN), Some(SNAP))
        );
        let latest = status.latest.unwrap();
        assert_eq!(
            (latest.begin.as_deref(), latest.end.as_deref()),
            (Some(BEGIN), Some(SNAP))
        );
        assert_eq!(status.snapshot.as_deref(), Some(SNAP));
    }

    #[test]
    fn one_snapshot_is_the_end_of_one_turn_and_the_begin_of_the_next() {
        let ended = closed(1, Outcome::Interrupted, Some(BEGIN));
        let mut status = Status {
            turn: Some(open(2, None)),
            latest: Some(ended.clone()),
            ..Status::default()
        };
        let mut records = vec![TurnRecord::end(&ended), TurnRecord::start(&open(2, None))];
        fill(&mut records, &mut status, SNAP);
        assert_eq!(records[0].begin.as_deref(), Some(BEGIN), "its own begin");
        assert_eq!(records[0].end.as_deref(), Some(SNAP));
        assert_eq!(records[0].end_observed, Some(false));
        assert_eq!(records[1].begin.as_deref(), Some(SNAP));
        assert_eq!(status.latest.unwrap().end.as_deref(), Some(SNAP));
        assert_eq!(status.turn.unwrap().begin.as_deref(), Some(SNAP));
    }

    #[test]
    fn the_agent_ends_a_turn_and_the_person_begins_one() {
        let ending = Bounds {
            ends: Some(Ending {
                turn: 1,
                observed: true,
            }),
            begins: Some(2),
            session: None,
        };
        assert!(authored_by_agent(&ending));
        assert!(!authored_by_agent(&Bounds {
            ends: None,
            ..ending
        }));
        assert_eq!(
            Identity::agent("claude"),
            Identity {
                name: "claude".to_owned(),
                email: "claude@orca-term.invalid".to_owned()
            }
        );
        assert_eq!(
            Identity::person(Some("Ana".to_owned()), Some("ana@example.com".to_owned())),
            Identity {
                name: "Ana".to_owned(),
                email: "ana@example.com".to_owned()
            }
        );
        assert_eq!(
            Identity::person(None, Some(" ".to_owned())),
            Identity {
                name: "person".to_owned(),
                email: "person@orca-term.invalid".to_owned()
            }
        );
        assert_eq!(Identity::committer().email, "orca-term@orca-term.invalid");
    }

    #[test]
    fn refs_name_the_worktree_by_its_admin_dir_id() {
        assert_eq!(chain_ref("fix1"), "refs/orca-term/worktrees/fix1");
        assert_eq!(chain_id("refs/orca-term/worktrees/fix1"), Some("fix1"));
        assert_eq!(chain_id("refs/orca-term/worktrees/"), None);
        assert_eq!(chain_id("refs/orca-term/worktrees/a/b"), None);
        assert_eq!(chain_id("refs/heads/fix1"), None);
        assert_eq!(
            git_admin_dir(Path::new("/r/.git"), "fix1"),
            PathBuf::from("/r/.git/worktrees/fix1")
        );
        assert_eq!(git_date(1_700_000_000_999), "1700000000 +0000");
    }
}
