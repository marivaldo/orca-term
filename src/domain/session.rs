//! Domain: a worktree's sessions (#41), as `session ls` lists them and `session resume` and
//! `session new` change which one is current.
//!
//! A session is a distinct pair of an agent and that agent's id for it. A worktree's sessions are
//! every pair its turn log names (`turns.jsonl`, `domain/turn.rs`), plus the current session
//! `status.json` records, even before its first turn. Nothing else records them: a session left
//! before its first turn, once another one is current, is forgotten, and there was nothing in it
//! to resume.
//!
//! Which session is current is `status.json`'s `session` and `agent`. `session new` clears them,
//! so the next start begins a new session; `session resume` sets them to the session chosen, with
//! the transcript claude keeps it in, so the start that follows resumes it (`domain/launch.rs`).
//! Every session keeps its turns either way.

use std::cmp::Reverse;
use std::path::PathBuf;

use crate::domain::agent::{Agent, Started, Swapped};
use crate::domain::error::{GoneReason, TypedError};
use crate::domain::fleet::Worktree;
use crate::domain::history::LoggedTurn;
use crate::domain::launch::{Resume, SessionId};
use crate::domain::turn::Status;

/// One session of a worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Session {
    /// The agent's id for it.
    pub(crate) id: String,
    /// The agent it belongs to, as its hooks name it: `claude` or `opencode`.
    pub(crate) agent: String,
    /// The first line of its first turn's prompt; `None` before its first turn.
    pub(crate) first_prompt: Option<String>,
    /// How many turns it holds.
    pub(crate) turns: usize,
    /// When it was last active, in Unix milliseconds: its latest turn's start or end, else, for
    /// the current session before its first turn, when its hooks last reported. `None` when
    /// nothing says.
    pub(crate) last_activity: Option<u64>,
    /// Whether it is the worktree's current session.
    pub(crate) current: bool,
    /// The transcript claude keeps it in, the latest one recorded for it; `None` for opencode's.
    pub(crate) transcript: Option<PathBuf>,
}

impl Session {
    /// The agent the session belongs to, when it is one this core knows.
    pub(crate) fn agent(&self) -> Option<Agent> {
        Agent::from_program(&self.agent)
    }
}

/// The sessions of a worktree from its turns (`turns`, from the turn log, oldest first) and its
/// live status (`status`, `None` when there is none to use), the most recently active first, the
/// current one first among sessions active at the same time, those with no known activity last.
pub(crate) fn sessions(turns: &[LoggedTurn], status: Option<&Status>) -> Vec<Session> {
    let mut sessions: Vec<Session> = Vec::new();
    for turn in turns {
        let Some(id) = &turn.session else {
            continue;
        };
        // A turn logged before turns named their agent was claude's, the only agent then.
        let agent = turn.agent.as_deref().unwrap_or(Agent::ClaudeCode.program());
        let session = find_or_add(&mut sessions, id, agent);
        if session.first_prompt.is_none() {
            session.first_prompt = Some(turn.prompt.clone());
        }
        session.turns += 1;
        let times = [session.last_activity, turn.started_at, turn.ended_at];
        session.last_activity = times.into_iter().flatten().max();
        if turn.transcript.is_some() {
            session.transcript.clone_from(&turn.transcript);
        }
    }
    if let Some(status) = status
        && let Some(id) = &status.session
    {
        let session = find_or_add(&mut sessions, id, status.session_agent());
        session.current = true;
        if session.turns == 0 && status.updated_at > 0 {
            session.last_activity = Some(status.updated_at);
        }
        if status.transcript.is_some() {
            session.transcript.clone_from(&status.transcript);
        }
    }
    sessions.sort_by_key(|session| (Reverse(session.last_activity), !session.current));
    sessions
}

/// The session of `sessions` that `agent` names `id`, added with nothing in it when there is none.
fn find_or_add<'a>(sessions: &'a mut Vec<Session>, id: &str, agent: &str) -> &'a mut Session {
    let known = sessions
        .iter()
        .position(|session| session.id == id && session.agent == agent);
    if let Some(index) = known {
        return &mut sessions[index];
    }
    sessions.push(Session {
        id: id.to_owned(),
        agent: agent.to_owned(),
        first_prompt: None,
        turns: 0,
        last_activity: None,
        current: false,
        transcript: None,
    });
    let last = sessions.len() - 1;
    &mut sessions[last]
}

/// The session of `sessions` (most recently active first) whose id is `id`, the most recently
/// active when two agents gave the same id. Refuses an id no session of the worktree `name` has.
pub(crate) fn find<'a>(
    name: &str,
    sessions: &'a [Session],
    id: &str,
) -> Result<&'a Session, SessionError> {
    sessions
        .iter()
        .find(|session| session.id == id)
        .ok_or_else(|| SessionError::NoSuchSession {
            name: name.to_owned(),
            id: id.to_owned(),
        })
}

/// Why a session command refuses a session.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum SessionError {
    #[error(
        "worktree {name} has no session {id}; `orca-term session ls {name}` lists its sessions"
    )]
    NoSuchSession { name: String, id: String },
    #[error(
        "session {id} of worktree {name} belongs to the agent {agent}, which this orca-term does \
         not know"
    )]
    UnknownAgent {
        name: String,
        id: String,
        agent: String,
    },
}

/// The agent `session` of the worktree `name` belongs to, refusing one this core does not know.
pub(crate) fn agent_of(name: &str, session: &Session) -> Result<Agent, SessionError> {
    session.agent().ok_or_else(|| SessionError::UnknownAgent {
        name: name.to_owned(),
        id: session.id.clone(),
        agent: session.agent.clone(),
    })
}

/// What resuming `session` of the worktree `name` needs: its id, checked, and for claude its
/// transcript, which the caller then looks for on disk (`launch::check_transcript`). Fails as
/// `session_gone` when the id is not one an agent makes or no transcript of a claude session is
/// recorded (as an absolute path), as a cold start does.
pub(crate) fn to_resume(name: &str, session: &Session) -> Result<Resume, TypedError> {
    let gone = |reason| TypedError::SessionGone {
        name: name.to_owned(),
        session: session.id.clone(),
        reason,
    };
    let id = SessionId::parse(&session.id).ok_or_else(|| gone(GoneReason::InvalidId))?;
    let transcript = match session.agent() {
        Some(Agent::ClaudeCode) => Some(
            session
                .transcript
                .clone()
                .filter(|path| path.is_absolute())
                .ok_or_else(|| gone(GoneReason::NoTranscript))?,
        ),
        _ => None,
    };
    Ok(Resume {
        session: id,
        transcript,
    })
}

/// `status` with `session` made the current session: its id, its agent and its transcript, so the
/// next start resumes it.
pub(crate) fn make_current(status: Status, session: &Session) -> Status {
    Status {
        session: Some(session.id.clone()),
        agent: Some(session.agent.clone()),
        transcript: session.transcript.clone(),
        ..status
    }
}

/// `status` with no current session, so the next start begins a new one. The turns stay, the
/// latest one too.
pub(crate) fn discard(status: Status) -> Status {
    Status {
        session: None,
        agent: None,
        transcript: None,
        ..status
    }
}

/// What `session ls` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionList {
    /// The worktree as `worktree ls` shows it.
    pub(crate) worktree: Worktree,
    /// Its sessions, the most recently active first.
    pub(crate) sessions: Vec<Session>,
    /// When they were read, in Unix milliseconds, for the ages the text shows.
    pub(crate) now: u64,
}

/// What `session resume` reports: the session made current, and what starting its agent did (an
/// `agent swap`'s report: the agent the worktree had before, and whether its holder was stopped).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resumed {
    pub(crate) session: String,
    pub(crate) swapped: Swapped,
}

/// What `session new` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Discarded {
    /// The worktree as `worktree ls` now shows it.
    pub(crate) worktree: Worktree,
    /// The session that was current, `None` when there was none.
    pub(crate) session: Option<String>,
    /// What starting the agent again on a new session did, when it was running; `None` when it
    /// was not, and the next start begins the new session.
    pub(crate) started: Option<Started>,
}

/// How long ago `then` was at `now`, both in Unix milliseconds, in words: `just now`, `5 min
/// ago`, `3 h ago`, `2 days ago`.
pub(crate) fn age(now: u64, then: u64) -> String {
    const MINUTE: u64 = 60 * 1000;
    const HOUR: u64 = 60 * MINUTE;
    const DAY: u64 = 24 * HOUR;
    let elapsed = now.saturating_sub(then);
    if elapsed < MINUTE {
        "just now".to_owned()
    } else if elapsed < HOUR {
        format!("{} min ago", elapsed / MINUTE)
    } else if elapsed < DAY {
        format!("{} h ago", elapsed / HOUR)
    } else if elapsed < 2 * DAY {
        "1 day ago".to_owned()
    } else {
        format!("{} days ago", elapsed / DAY)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::domain::history::Progress;
    use crate::domain::turn::Outcome;

    fn turn(number: u64, agent: Option<&str>, session: &str, at: u64) -> LoggedTurn {
        LoggedTurn {
            number,
            agent: agent.map(str::to_owned),
            session: Some(session.to_owned()),
            prompt: format!("prompt {number}"),
            started_at: Some(at),
            ended_at: Some(at + 5),
            progress: Progress::Ended(Outcome::Finished),
            end_observed: Some(true),
            begin: None,
            end: None,
            denials: 0,
            transcript: None,
        }
    }

    fn status(session: &str, agent: &str, updated_at: u64) -> Status {
        Status {
            session: Some(session.to_owned()),
            agent: Some(agent.to_owned()),
            updated_at,
            ..Status::default()
        }
    }

    fn summary(sessions: &[Session]) -> Vec<(&str, &str, usize, Option<u64>, bool)> {
        sessions
            .iter()
            .map(|session| {
                (
                    session.id.as_str(),
                    session.agent.as_str(),
                    session.turns,
                    session.last_activity,
                    session.current,
                )
            })
            .collect()
    }

    #[test]
    fn every_pair_of_agent_and_id_is_one_session_the_most_recent_first() {
        let turns = [
            turn(1, None, "s-1", 10),
            turn(2, Some("claude"), "s-1", 20),
            turn(3, Some("opencode"), "ses_1", 30),
            turn(4, Some("claude"), "s-2", 40),
        ];
        let sessions = sessions(&turns, Some(&status("ses_1", "opencode", 99)));
        assert_eq!(
            summary(&sessions),
            [
                ("s-2", "claude", 1, Some(45), false),
                ("ses_1", "opencode", 1, Some(35), true),
                ("s-1", "claude", 2, Some(25), false),
            ],
            "a turn logged before turns named their agent is claude's"
        );
        assert_eq!(sessions[2].first_prompt.as_deref(), Some("prompt 1"));
    }

    #[test]
    fn the_current_session_is_listed_before_its_first_turn() {
        let turns = [turn(1, Some("claude"), "s-1", 10)];
        let sessions = sessions(&turns, Some(&status("s-2", "claude", 50)));
        assert_eq!(
            summary(&sessions),
            [
                ("s-2", "claude", 0, Some(50), true),
                ("s-1", "claude", 1, Some(15), false),
            ]
        );
        assert_eq!(sessions[0].first_prompt, None);
        let unknown_time = super::sessions(&[], Some(&status("s-3", "claude", 0)));
        assert_eq!(summary(&unknown_time), [("s-3", "claude", 0, None, true)]);
        assert!(super::sessions(&[], None).is_empty());
    }

    #[test]
    fn a_status_without_an_agent_names_claudes_session() {
        let mut old = status("s-1", "claude", 5);
        old.agent = None;
        let sessions = sessions(&[turn(1, None, "s-1", 1)], Some(&old));
        assert_eq!(summary(&sessions), [("s-1", "claude", 1, Some(6), true)]);
    }

    #[test]
    fn a_sessions_transcript_is_the_latest_recorded() {
        let mut first = turn(1, Some("claude"), "s-1", 10);
        first.transcript = Some(PathBuf::from("/t/old.jsonl"));
        let mut second = turn(2, Some("claude"), "s-1", 20);
        second.transcript = Some(PathBuf::from("/t/s-1.jsonl"));
        let third = turn(3, Some("claude"), "s-1", 30);
        let listed = sessions(&[first, second, third], None);
        assert_eq!(
            listed[0].transcript.as_deref(),
            Some(Path::new("/t/s-1.jsonl"))
        );
        let mut current = status("s-1", "claude", 40);
        current.transcript = Some(PathBuf::from("/t/now.jsonl"));
        let listed = sessions(&[turn(1, Some("claude"), "s-1", 1)], Some(&current));
        assert_eq!(
            listed[0].transcript.as_deref(),
            Some(Path::new("/t/now.jsonl"))
        );
    }

    #[test]
    fn an_unknown_id_is_refused_naming_session_ls() {
        let listed = sessions(&[turn(1, Some("claude"), "s-1", 1)], None);
        assert_eq!(find("fix", &listed, "s-1").unwrap().id, "s-1");
        let err = find("fix", &listed, "nope").unwrap_err();
        assert_eq!(
            err.to_string(),
            "worktree fix has no session nope; `orca-term session ls fix` lists its sessions"
        );
    }

    #[test]
    fn resuming_needs_a_good_id_and_for_claude_a_transcript() {
        let mut claude = sessions(&[turn(1, Some("claude"), "s-1", 1)], None).remove(0);
        let err = to_resume("fix", &claude).unwrap_err();
        assert_eq!(err.kind(), "session_gone");
        assert!(
            err.to_string().contains("no transcript of it is recorded"),
            "{err}"
        );
        claude.transcript = Some(PathBuf::from("relative.jsonl"));
        assert!(to_resume("fix", &claude).is_err(), "only an absolute path");
        claude.transcript = Some(PathBuf::from("/t/s-1.jsonl"));
        let resume = to_resume("fix", &claude).unwrap();
        assert_eq!(resume.session.as_str(), "s-1");
        assert_eq!(
            resume.transcript.as_deref(),
            Some(Path::new("/t/s-1.jsonl"))
        );

        let opencode = sessions(&[turn(1, Some("opencode"), "ses_1", 1)], None).remove(0);
        assert_eq!(to_resume("fix", &opencode).unwrap().transcript, None);
        let bad = sessions(&[turn(1, Some("opencode"), "-not-an-id", 1)], None).remove(0);
        let err = to_resume("fix", &bad).unwrap_err();
        assert!(
            err.to_string().contains("its id is not one an agent makes"),
            "{err}"
        );
    }

    #[test]
    fn an_agent_this_core_does_not_know_is_refused() {
        let other = sessions(&[turn(1, Some("aider"), "a-1", 1)], None).remove(0);
        assert_eq!(
            agent_of("fix", &other).unwrap_err().to_string(),
            "session a-1 of worktree fix belongs to the agent aider, which this orca-term does \
             not know"
        );
        let opencode = sessions(&[turn(1, Some("opencode"), "ses_1", 1)], None).remove(0);
        assert_eq!(agent_of("fix", &opencode), Ok(Agent::OpenCode));
    }

    #[test]
    fn making_a_session_current_or_discarding_it_moves_only_the_pointer() {
        let mut before = status("s-2", "claude", 7);
        before.transcript = Some(PathBuf::from("/t/s-2.jsonl"));
        before.snapshot = Some("tip".to_owned());
        let mut old = sessions(&[turn(1, Some("opencode"), "ses_1", 1)], None).remove(0);
        old.transcript = None;
        let after = make_current(before.clone(), &old);
        assert_eq!(after.session.as_deref(), Some("ses_1"));
        assert_eq!(after.session_agent(), "opencode");
        assert_eq!(after.transcript, None);
        assert_eq!(
            (after.snapshot.as_deref(), after.updated_at),
            (Some("tip"), 7),
            "the rest stays"
        );
        let discarded = discard(before);
        assert_eq!(
            (discarded.session, discarded.agent, discarded.transcript),
            (None, None, None)
        );
        assert_eq!(discarded.snapshot.as_deref(), Some("tip"));
    }

    #[test]
    fn an_age_reads_in_words() {
        let minute = 60 * 1000;
        assert_eq!(age(100, 200), "just now", "a clock behind is now");
        assert_eq!(age(59_000, 0), "just now");
        assert_eq!(age(5 * minute, 0), "5 min ago");
        assert_eq!(age(3 * 60 * minute + 1, 0), "3 h ago");
        assert_eq!(age(30 * 60 * minute, 0), "1 day ago");
        assert_eq!(age(3 * 24 * 60 * minute, 0), "3 days ago");
    }
}
