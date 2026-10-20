//! Domain: a worktree's detail (`worktree detail`), everything the client's detail float shows:
//! the worktree as `worktree ls` shows it, the whole text of why it is in its state, the tool
//! calls its agent was denied in its latest turn, the end of its setup log, and how far it drifted
//! from the repository's local default branch. All of it is read on request; nothing is fetched
//! and nothing is written.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::domain::branch::Branch;
use crate::domain::fleet::Worktree;
use crate::domain::setup::{self, Exit, SetupRecord};
use crate::domain::state::State;
use crate::domain::turn::{Denial, Status};

/// What `worktree detail` found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Detail {
    /// The worktree as `worktree ls` shows it.
    pub(crate) worktree: Worktree,
    /// Why the worktree is in its state, whole: see [`whole_detail`].
    pub(crate) why: Option<String>,
    /// The tool calls its agent was denied in its latest turn, open or ended, in order.
    pub(crate) denials: Vec<Denial>,
    /// Its latest setup, `None` when it never ran one (or its state cannot be read).
    pub(crate) setup: Option<SetupDetail>,
    /// How far it is from the local default branch, `None` when that cannot be told.
    pub(crate) drift: Option<Drift>,
}

/// A worktree's latest setup, as the detail shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SetupDetail {
    pub(crate) progress: SetupProgress,
    /// The setup log.
    pub(crate) log: PathBuf,
    /// Its last lines, as a person reads them (`setup::log_tail`); empty when it is missing.
    pub(crate) tail: Vec<String>,
}

/// Where a worktree's latest setup is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupProgress {
    /// It runs right now.
    Running,
    /// It was recorded as running, but its process died before recording its end.
    Interrupted,
    Succeeded,
    /// It failed, as `Exit` says.
    Failed(Exit),
}

impl SetupProgress {
    /// Where the setup `worktree.json` records as `record` is, for a worktree in `state`: a setup
    /// recorded as running runs only while the worktree is setting up (its lock held).
    pub(crate) fn of(record: SetupRecord, state: &State) -> Self {
        match record {
            SetupRecord::Running if *state == State::SettingUp => Self::Running,
            SetupRecord::Running => Self::Interrupted,
            SetupRecord::Succeeded => Self::Succeeded,
            SetupRecord::Failed(exit) => Self::Failed(exit),
        }
    }
}

/// How far a worktree's `HEAD` is from the local default branch: the commits it has that the
/// branch does not (`ahead`), and the commits the branch has that it does not (`behind`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Drift {
    /// The local default branch it is compared with.
    pub(crate) against: Branch,
    pub(crate) ahead: u64,
    pub(crate) behind: u64,
}

impl Drift {
    /// The drift from `against` that `git rev-list --left-right --count <against>...HEAD` printed
    /// as `text`: the commits only on the left side (`behind`), a tab, the commits only on the
    /// right side (`ahead`).
    pub(crate) fn from_counts(against: Branch, text: &str) -> Result<Self> {
        let mut counts = text.split_whitespace();
        let (Some(behind), Some(ahead), None) = (counts.next(), counts.next(), counts.next())
        else {
            bail!(
                "git printed `{}` where two commit counts were expected",
                text.trim()
            );
        };
        let count = |n: &str| {
            n.parse::<u64>()
                .with_context(|| format!("git printed `{n}` where a commit count was expected"))
        };
        Ok(Self {
            against,
            ahead: count(ahead)?,
            behind: count(behind)?,
        })
    }
}

/// Why a worktree in `state`, whose hooks' live status is `status`, is in it, whole: the
/// state's own detail (a broken worktree's reason, a failed setup's exit and the last line of its
/// log), else, for a turn that failed on an API error, everything the agent said last (every
/// line, which the fleet's one-line activity cuts; that one line for a status written before it
/// was kept). `None` for every other state. The terminal's control sequences a setup log line
/// may carry (colours) are taken out.
pub(crate) fn whole_detail(state: &State, status: Option<&Status>) -> Option<String> {
    let text = match state.detail() {
        Some(detail) => detail,
        None if *state == State::Failed => {
            let failed = status?.latest.as_ref()?;
            failed.message.clone().or_else(|| failed.detail.clone())?
        }
        None => return None,
    };
    Some(setup::strip_terminal_codes(&text))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::state::{BrokenReason, NotReadyReason};
    use crate::domain::turn::{ClosedTurn, Outcome};

    fn failed_status(detail: Option<&str>, message: Option<&str>) -> Status {
        Status {
            latest: Some(ClosedTurn {
                number: 1,
                session: None,
                prompt_id: None,
                prompt: "go".to_owned(),
                outcome: Outcome::Failed,
                detail: detail.map(str::to_owned),
                error: Some("unknown".to_owned()),
                ended_at: 1,
                denials: Vec::new(),
                message: message.map(str::to_owned),
            }),
            ..Status::default()
        }
    }

    #[test]
    fn the_whole_detail_is_the_states_or_the_failed_turns_whole_message() {
        let setup = State::NotReady(NotReadyReason::Failed {
            exit: Exit::Code(2),
            last_line: Some("error: a long reason, never cut".to_owned()),
        });
        assert_eq!(
            whole_detail(&setup, None).as_deref(),
            Some("setup failed (exit 2): error: a long reason, never cut")
        );
        let broken = State::Broken(BrokenReason::DirectoryMissing);
        assert_eq!(whole_detail(&broken, None), broken.detail());
        let coloured = State::NotReady(NotReadyReason::Failed {
            exit: Exit::Code(1),
            last_line: Some("\u{1b}[31merror\u{1b}[0m: boom".to_owned()),
        });
        assert_eq!(
            whole_detail(&coloured, None).as_deref(),
            Some("setup failed (exit 1): error: boom")
        );

        let status = failed_status(Some("API Error: 400"), Some("API Error: 400\n{\"x\":1}"));
        assert_eq!(
            whole_detail(&State::Failed, Some(&status)).as_deref(),
            Some("API Error: 400\n{\"x\":1}")
        );
        let older = failed_status(Some("API Error: 400"), None);
        assert_eq!(
            whole_detail(&State::Failed, Some(&older)).as_deref(),
            Some("API Error: 400"),
            "a status written before the whole message was kept"
        );
        assert_eq!(whole_detail(&State::Failed, None), None);
        for state in [State::Finished, State::Working, State::Idle, State::NoAgent] {
            assert_eq!(whole_detail(&state, Some(&status)), None, "{state:?}");
        }
    }

    #[test]
    fn a_setup_recorded_as_running_runs_only_while_the_worktree_sets_up() {
        assert_eq!(
            SetupProgress::of(SetupRecord::Running, &State::SettingUp),
            SetupProgress::Running
        );
        assert_eq!(
            SetupProgress::of(
                SetupRecord::Running,
                &State::NotReady(NotReadyReason::Interrupted)
            ),
            SetupProgress::Interrupted
        );
        assert_eq!(
            SetupProgress::of(SetupRecord::Succeeded, &State::NoAgent),
            SetupProgress::Succeeded
        );
        assert_eq!(
            SetupProgress::of(SetupRecord::Failed(Exit::Signal(9)), &State::NoAgent),
            SetupProgress::Failed(Exit::Signal(9))
        );
    }

    #[test]
    fn drift_counts_the_commits_on_each_side() {
        let main = || Branch::new("main");
        assert_eq!(
            Drift::from_counts(main(), "1\t3\n").unwrap(),
            Drift {
                against: main(),
                ahead: 3,
                behind: 1
            }
        );
        assert_eq!(
            Drift::from_counts(main(), "0\t0").unwrap(),
            Drift {
                against: main(),
                ahead: 0,
                behind: 0
            }
        );
        for bad in ["", "3", "1\t2\t3", "a\t1", "-1\t2"] {
            assert!(Drift::from_counts(main(), bad).is_err(), "{bad:?}");
        }
    }
}
