//! App: the agent's panel, the open worktree's agent terminal in the centre (#174). Its screen
//! (`emulator.rs`) is fed with what the agent's holder sends: the replay first, then the agent's
//! output as it comes. The connection itself lives in the run loop (`app.rs`); this is what is
//! drawn and whether an agent runs.
//!
//! The answers the screen has for the agent's questions are not sent: every client of a holder
//! would answer, and the terminal the agent was started from already does.

use std::path::PathBuf;

use ratatui::layout::Rect;

use crate::adapters::holder::client::Heard;
use crate::app::emulator::Emulator;

/// Whether the panel shows a running agent, and how it got there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    /// No agent runs in the worktree: `enter` starts one.
    NoAgent,
    /// The core is starting the agent.
    Starting,
    /// Attached to the agent's holder.
    Attached,
    /// The agent ended, as the text says (`exit 0`): `enter` starts it again.
    Ended(String),
}

/// The open worktree's agent terminal.
#[derive(Debug)]
pub(crate) struct Panel {
    /// The worktree whose agent this is.
    pub(crate) worktree: PathBuf,
    pub(crate) status: Status,
    pub(crate) screen: Emulator,
}

impl Panel {
    /// The panel of the worktree at `worktree`, empty, `size` cells large.
    pub(crate) fn new(worktree: PathBuf, status: Status, size: Rect) -> Self {
        Self {
            worktree,
            status,
            screen: Emulator::new(size),
        }
    }

    /// What the holder said: output for the screen, or the agent's end.
    pub(crate) fn heard(&mut self, heard: Heard) {
        match heard {
            Heard::Output(bytes) => {
                self.screen.feed(&bytes);
            }
            Heard::Ended(text) => self.status = Status::Ended(text),
            Heard::Closed => {
                if self.status == Status::Attached {
                    self.status = Status::NoAgent;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> Panel {
        Panel::new(
            PathBuf::from("/w"),
            Status::Attached,
            Rect::new(0, 0, 20, 4),
        )
    }

    #[test]
    fn the_holders_output_reaches_the_screen() {
        let mut panel = panel();
        panel.heard(Heard::Output(b"hello".to_vec()));
        assert_eq!(panel.screen.cursor(Rect::new(0, 0, 20, 4)), Some((5, 0)));
    }

    #[test]
    fn the_agents_end_and_a_lost_holder_are_told_apart() {
        let mut panel = panel();
        panel.heard(Heard::Ended("exit 3".to_owned()));
        panel.heard(Heard::Closed);
        assert_eq!(panel.status, Status::Ended("exit 3".to_owned()));
        let mut panel = self::panel();
        panel.heard(Heard::Closed);
        assert_eq!(
            panel.status,
            Status::NoAgent,
            "the holder went without a word"
        );
    }
}
