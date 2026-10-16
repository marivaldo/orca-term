//! Domain: the agent a worktree runs, `claude-code` for now, and the facts about the holder that
//! keeps it running (ADR 0009). The agent is recorded in `worktree.json`; the holder's pid and
//! socket are volatile, written to `agent.json` and trusted only while `agent.lock` is held.

use std::fmt;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::domain::config::Setting;
use crate::domain::fleet::Worktree;
use crate::domain::launch::SessionId;
use crate::domain::state::LockStatus;

/// The agents a worktree can run. `worktree.json` records one under its glossary name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Agent {
    ClaudeCode,
}

impl Agent {
    /// The agent's name in `worktree.json` and in every report: `claude-code`.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
        }
    }

    /// The program the agent is: `claude`, looked up on PATH.
    pub(crate) const fn program(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
        }
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
