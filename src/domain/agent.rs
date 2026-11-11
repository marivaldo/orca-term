//! Domain: the agent a worktree runs, `claude-code` or `opencode`, and the facts about the holder
//! that keeps it running (ADR 0009). The agent is recorded in `worktree.json`; the holder's pid and
//! socket are volatile, written to `agent.json` and trusted only while `agent.lock` is held.

use std::fmt;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::domain::config::Setting;
use crate::domain::fleet::Worktree;
use crate::domain::launch::SessionId;
use crate::domain::sandbox::signing::SigningReach;
use crate::domain::state::LockStatus;

/// The agents a worktree can run. `worktree.json` records one under its glossary name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Agent {
    ClaudeCode,
    OpenCode,
}

impl Agent {
    /// Every agent, in the order the reports and the client list them.
    pub(crate) const ALL: [Self; 2] = [Self::ClaudeCode, Self::OpenCode];

    /// The agent's name in `worktree.json` and in every report: `claude-code` or `opencode`.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::OpenCode => "opencode",
        }
    }

    /// The program the agent is, looked up on PATH: `claude` or `opencode`. Its hooks name it the
    /// same way (`hook claude`, `hook opencode`), and so do the turns it opens.
    pub(crate) const fn program(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::OpenCode => "opencode",
        }
    }

    /// The agent `worktree.json` records as `name`, or `None` for a name this core does not know.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|agent| agent.name() == name)
    }

    /// The agent whose program is `program`, as its hooks name it, or `None`.
    pub(crate) fn from_program(program: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|agent| agent.program() == program)
    }
}

/// What `--agent` and `agent swap` take: an agent's name, `claude-code` or `opencode`.
impl FromStr for Agent {
    type Err = String;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::from_name(text).ok_or_else(|| {
            let names: Vec<&str> = Self::ALL.iter().map(|agent| agent.name()).collect();
            format!(
                "unknown agent `{text}`; the agents are {}",
                names.join(" and ")
            )
        })
    }
}

impl fmt::Display for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Whether a worktree's holder is alive right now: the read-only probe of `agent.lock`, which the
/// holder keeps locked for its whole life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum HolderStatus {
    Alive,
    Gone,
}

impl HolderStatus {
    /// What the probe of `agent.lock` means.
    pub(crate) fn from_lock(lock: LockStatus) -> Self {
        match lock {
            LockStatus::Held => Self::Alive,
            LockStatus::Free => Self::Gone,
        }
    }

    pub(crate) fn is_alive(self) -> bool {
        self == Self::Alive
    }
}

/// What the holder writes to `agent.json` once the agent runs. Volatile facts: a holder killed
/// outright leaves the file behind, so readers believe it only while `agent.lock` is held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct HolderRecord {
    /// The holder's pid.
    pub(crate) pid: u32,
    /// The absolute path of `agent.sock`, for display; clients connect by a relative name.
    pub(crate) socket: PathBuf,
}

impl HolderRecord {
    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    pub(crate) fn parse(bytes: &[u8]) -> Result<Self> {
        serde_json::from_slice(bytes).context("could not parse agent.json")
    }
}

/// How a worktree's agent ended last, which its holder writes to `agent-end.json` as it leaves,
/// beside the agent's last output in `agent.log` (#178): the application shows a failed one as a
/// card with that output. The next holder removes both as it starts, so it is read only while no
/// holder is alive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AgentEnd {
    /// How it ended, as the holder tells its clients: `exit 1`, `signal 9`.
    pub(crate) exit: String,
    /// Whether it failed: it ended with an error and nobody asked it to stop.
    pub(crate) failed: bool,
    /// When the holder started it, and when it ended, in Unix milliseconds.
    pub(crate) started_at: u64,
    pub(crate) ended_at: u64,
}

impl AgentEnd {
    /// The end of an agent that exited with `code`, or was killed by `signal`, after the holder
    /// was asked to stop it or not (`stopped`). Exit 0, or any end after a stop (`agent stop`
    /// hangs it up), is not a failure.
    pub(crate) fn new(
        code: Option<i32>,
        signal: Option<i32>,
        stopped: bool,
        started_at: u64,
        ended_at: u64,
    ) -> Self {
        Self {
            exit: crate::domain::protocol::exit_text(code, signal),
            failed: !stopped && code != Some(0),
            started_at,
            ended_at,
        }
    }

    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }

    /// The end `agent-end.json` holds, or `None` when it is missing or cannot be read: the card is
    /// a courtesy, never a reason for a read to fail.
    pub(crate) fn read(bytes: Option<&[u8]>) -> Option<Self> {
        serde_json::from_slice(bytes?).ok()
    }
}

/// A terminal's size in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Size {
    pub(crate) rows: u16,
    pub(crate) cols: u16,
}

impl Size {
    /// The size an agent starts with when nobody says otherwise: 24 rows by 80 columns.
    pub(crate) const DEFAULT: Self = Self { rows: 24, cols: 80 };

    /// This size with `rows` and `cols` replaced where given.
    pub(crate) fn with(self, rows: Option<u16>, cols: Option<u16>) -> Self {
        Self {
            rows: rows.unwrap_or(self.rows),
            cols: cols.unwrap_or(self.cols),
        }
    }

    /// A size one column narrower, the first half of the "jiggle" that makes an agent redraw its
    /// screen: a resize to the same size changes nothing for it.
    pub(crate) fn jiggled(self) -> Self {
        Self {
            rows: self.rows,
            cols: self.cols.saturating_sub(1).max(1),
        }
    }
}

/// Which session `agent start` starts the agent on when no holder is alive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StartMode {
    /// The current session `status.json` records, resumed; a new session when none is recorded.
    Resume,
    /// A new session, even when one is recorded (`--fresh`).
    Fresh,
}

impl StartMode {
    /// The mode the `--fresh` flag asks for: `Fresh` when given, else `Resume`.
    pub(crate) fn from_fresh_flag(fresh: bool) -> Self {
        if fresh { Self::Fresh } else { Self::Resume }
    }
}

/// What `agent start` did about the holder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartOutcome {
    /// A holder was started, with this pid, on the session `resumed` names, or on a new session
    /// when it is `None`.
    Started {
        pid: u32,
        resumed: Option<SessionId>,
    },
    /// A holder was already alive; its pid when `agent.json` could be read.
    AlreadyRunning { pid: Option<u32> },
}

/// What `agent start` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Started {
    /// The worktree as `worktree ls` now shows it.
    pub(crate) worktree: Worktree,
    pub(crate) agent: Agent,
    pub(crate) outcome: StartOutcome,
    /// The extra writable paths the sandbox got, and the config file they came from. `None` when
    /// nothing was launched.
    pub(crate) writable: Option<Setting<Vec<PathBuf>>>,
    /// Which sockets of the person's signing agent the sandbox reaches (#108). `None` when
    /// `sandbox.agent_socket` is off, or when nothing was launched.
    pub(crate) signing: Option<SigningReach>,
}

/// What `agent swap` reports: the agent started, and the one the worktree had before.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Swapped {
    /// The agent the worktree recorded before the swap; `None` when it had none.
    pub(crate) previous: Option<Agent>,
    /// Whether a holder was alive, and was stopped before the new agent started.
    pub(crate) stopped: bool,
    /// What starting the new agent did.
    pub(crate) started: Started,
}

/// What `agent stop` did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopOutcome {
    /// The holder was asked to end the agent, and is gone.
    Stopped,
    /// No holder was alive: nothing to do.
    NotRunning,
}

/// What `agent stop` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stopped {
    pub(crate) worktree: Worktree,
    pub(crate) outcome: StopOutcome,
}

/// How an `agent attach` ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AttachEnd {
    /// The person detached, or the holder dropped the connection; the agent runs on.
    Detached,
    /// The agent ended, as the text says (`exit 7`, `signal 9`).
    AgentEnded(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_code_is_recorded_by_name_and_run_as_claude() {
        assert_eq!(Agent::ClaudeCode.name(), "claude-code");
        assert_eq!(Agent::ClaudeCode.to_string(), "claude-code");
        assert_eq!(Agent::ClaudeCode.program(), "claude");
    }

    #[test]
    fn opencode_is_recorded_and_run_as_opencode() {
        assert_eq!(Agent::OpenCode.name(), "opencode");
        assert_eq!(Agent::OpenCode.to_string(), "opencode");
        assert_eq!(Agent::OpenCode.program(), "opencode");
    }

    #[test]
    fn an_agent_is_read_by_its_name_or_its_program() {
        assert_eq!(Agent::from_name("claude-code"), Some(Agent::ClaudeCode));
        assert_eq!(Agent::from_name("opencode"), Some(Agent::OpenCode));
        assert_eq!(
            Agent::from_name("claude"),
            None,
            "the program is not the name"
        );
        assert_eq!(Agent::from_program("claude"), Some(Agent::ClaudeCode));
        assert_eq!(Agent::from_program("opencode"), Some(Agent::OpenCode));
        assert_eq!(Agent::from_program("codex"), None);
        assert_eq!("opencode".parse::<Agent>(), Ok(Agent::OpenCode));
        assert_eq!(
            "codex".parse::<Agent>(),
            Err("unknown agent `codex`; the agents are claude-code and opencode".to_owned())
        );
    }

    #[test]
    fn a_held_agent_lock_means_a_live_holder() {
        assert!(HolderStatus::from_lock(LockStatus::Held).is_alive());
        assert!(!HolderStatus::from_lock(LockStatus::Free).is_alive());
    }

    #[test]
    fn the_holder_record_is_written_and_read_back() {
        let record = HolderRecord {
            pid: 4242,
            socket: PathBuf::from("/repo/.git/worktrees/fix/orca-term/agent.sock"),
        };
        let bytes = record.to_bytes().unwrap();
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            r#"{"pid":4242,"socket":"/repo/.git/worktrees/fix/orca-term/agent.sock"}"#
        );
        assert_eq!(HolderRecord::parse(&bytes).unwrap(), record);
        assert!(HolderRecord::parse(b"{").is_err());
    }

    #[test]
    fn an_agent_fails_on_an_error_it_was_not_asked_for() {
        let failed = AgentEnd::new(Some(7), None, false, 1_000, 61_000);
        assert_eq!(failed.exit, "exit 7");
        assert!(failed.failed);
        assert!(AgentEnd::new(None, Some(9), false, 0, 0).failed, "killed");
        assert!(!AgentEnd::new(Some(0), None, false, 0, 0).failed, "exit 0");
        assert!(
            !AgentEnd::new(Some(129), None, true, 0, 0).failed,
            "hung up by agent stop"
        );
        let bytes = failed.to_bytes().unwrap();
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            r#"{"exit":"exit 7","failed":true,"started_at":1000,"ended_at":61000}"#
        );
        assert_eq!(AgentEnd::read(Some(&bytes)), Some(failed));
        assert_eq!(AgentEnd::read(Some(b"{")), None);
        assert_eq!(AgentEnd::read(None), None);
    }

    #[test]
    fn a_size_takes_overrides_and_jiggles_one_column() {
        let size = Size::DEFAULT.with(None, Some(120));
        assert_eq!(
            size,
            Size {
                rows: 24,
                cols: 120
            }
        );
        assert_eq!(
            size.jiggled(),
            Size {
                rows: 24,
                cols: 119
            }
        );
        assert_eq!(Size { rows: 1, cols: 1 }.jiggled().cols, 1);
    }
}
