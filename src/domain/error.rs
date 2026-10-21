//! Domain: the failures a client must tell apart (ADR 0010). Each one has a `kind`, which `--json`
//! prints as `error.kind`, and an exit code of its own.
//!
//! A typed error travels inside an `anyhow::Error` like any other, so the code between the failure
//! and `main` needs no conversions; [`kind`] and [`exit_code`] find it again by downcasting. Every
//! other failure has the kind `other` and exits 1. Exit code 2 is clap's, for a usage error.

use std::path::{Path, PathBuf};

use crate::domain::agent::Agent;
use crate::domain::prerequisite::DOCTOR_HINT;
use crate::domain::sandbox;
use crate::domain::trust::Subject;

/// The exit code of a failure that is not typed.
pub(crate) const FAILURE_EXIT: u8 = 1;

/// The `error.kind` of a failure that is not typed.
pub(crate) const OTHER_KIND: &str = "other";

/// A failure the client reacts to on its own, rather than only showing its message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum TypedError {
    /// The worktree is in use, as `reason` says, so a command that changes it must wait.
    #[error("{}", busy_message(name, path, *reason))]
    Busy {
        name: String,
        path: PathBuf,
        reason: BusyReason,
    },
    /// The sandbox `agent` runs in cannot run on this machine: `missing` names the tools absent
    /// from PATH (empty when the system has no sandbox at all), and `hint` the fix.
    #[error("{}", no_sandbox_message(*agent, missing, hint))]
    NoSandbox {
        agent: Agent,
        missing: Vec<String>,
        hint: String,
    },
    /// The worktree has modified or untracked files, so `worktree rm` keeps it unless forced.
    #[error(
        "worktree {name} at {} has modified or untracked files; pass --force to remove it \
         anyway, discarding its changes",
        path.display()
    )]
    Dirty { name: String, path: PathBuf },
    /// The worktree's current session cannot be resumed on a cold start, for `reason`;
    /// `session` is the id `status.json` recorded, as written there. Nothing was launched.
    #[error("{}", session_gone_message(name, session, reason))]
    SessionGone {
        name: String,
        session: String,
        reason: GoneReason,
    },
    /// No holder is alive in the worktree, so its pending notes have no agent to be sent to.
    /// Nothing was sent and every note stays pending.
    #[error(
        "worktree {name} at {} has no running agent to send notes to; start it with \
         `orca-term agent start {name}`, then send again (the notes stay pending)",
        path.display()
    )]
    NoHolder { name: String, path: PathBuf },
    /// A program the command needs is missing or too old (`domain/prerequisite.rs`): `problems`
    /// are the doctor's lines for each one, which name the fix. Nothing is installed.
    #[error("{}; {DOCTOR_HINT}", problems.join("; "))]
    Prerequisite { problems: Vec<String> },
    /// What the worktree's committed `orca-term.yaml` asks for, its setup script or its sandbox's
    /// writable paths (`subject`), is not approved in this repository (`domain/trust.rs`): nobody
    /// could be asked, or the person said no. A script did not run, and the worktree is not ready;
    /// an agent did not start. `file` is the committed file, so a client can show both before
    /// approving.
    #[error("{}", untrusted_message(name, subject, file))]
    Untrusted {
        name: String,
        subject: Subject,
        file: PathBuf,
    },
}

/// The message of [`TypedError::Untrusted`], naming the command that asks.
fn untrusted_message(name: &str, subject: &Subject, file: &Path) -> String {
    let file = file.display();
    match subject {
        Subject::Setup { .. } => format!(
            "the setup script of worktree {name}, from {file}, is not approved in this \
             repository, so it did not run and the worktree is not ready; read it, then run \
             `orca-term worktree setup {name}` in a terminal to be asked, or pass --approve-setup"
        ),
        Subject::Sandbox { writable } => format!(
            "the sandbox's extra writable paths of worktree {name} ({}), from {file}, are not \
             approved in this repository, so the agent did not start; read them, then run \
             `orca-term agent start {name}` in a terminal to be asked, or pass --approve-sandbox",
            writable.join(", ")
        ),
    }
}

/// Why a recorded session cannot be resumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GoneReason {
    /// The transcript claude kept the session in, at this path, no longer exists.
    TranscriptGone(PathBuf),
    /// No transcript was recorded for the session (or not as an absolute path).
    NoTranscript,
    /// The id is not one an agent makes, so it is never put on the agent's command line.
    InvalidId,
    /// opencode's own list of its sessions (`opencode session list`) does not hold it: deleted
    /// in opencode since (#41).
    NotListed,
}

/// Why a worktree is busy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BusyReason {
    /// Another process holds the worktree's lock: its setup, or another orca-term command, is
    /// running on it.
    Locked,
    /// Its agent is running: a holder is alive, and git must not touch the admin directory under
    /// it.
    AgentRunning,
    /// Another agent, `running`, runs in it, so `wanted` cannot start there (#35).
    AnotherAgent { running: Agent, wanted: Agent },
    /// Its agent is in a turn, working or waiting for the person, so it is not swapped under it.
    InTurn,
    /// Its agent is in a turn, working or waiting for the person, so its session is not changed
    /// under it (`session resume`, `session new`, #41).
    SessionInTurn,
}

fn busy_message(name: &str, path: &Path, reason: BusyReason) -> String {
    let path = path.display();
    match reason {
        BusyReason::Locked => format!(
            "worktree {name} at {path} is busy: its setup or another orca-term command is running \
             on it; try again once it finishes"
        ),
        BusyReason::AgentRunning => format!(
            "worktree {name} at {path} is busy: its agent is running; stop it first with \
             `orca-term agent stop {name}`"
        ),
        BusyReason::AnotherAgent { running, wanted } => format!(
            "worktree {name} at {path} is busy: its agent {running} is running; swap it for \
             {wanted} with `orca-term agent swap {name} {wanted}`, or stop it first with \
             `orca-term agent stop {name}`"
        ),
        BusyReason::InTurn => format!(
            "worktree {name} at {path} is busy: its agent is in a turn, working or waiting for \
             you; swap agents once the turn ends, or stop it first with `orca-term agent stop \
             {name}`"
        ),
        BusyReason::SessionInTurn => format!(
            "worktree {name} at {path} is busy: its agent is in a turn, working or waiting for \
             you; change its session once the turn ends, or stop it first with `orca-term agent \
             stop {name}`"
        ),
    }
}

/// The message of [`TypedError::SessionGone`]. The id is shown cut to one short line, since an
/// id that is not one claude makes may hold anything.
fn session_gone_message(name: &str, session: &str, reason: &GoneReason) -> String {
    let reason = match reason {
        GoneReason::TranscriptGone(path) => format!("its transcript {} is gone", path.display()),
        GoneReason::NoTranscript => "no transcript of it is recorded".to_owned(),
        GoneReason::InvalidId => "its id is not one an agent makes".to_owned(),
        GoneReason::NotListed => {
            "opencode no longer lists it in `opencode session list`".to_owned()
        }
    };
    let session: String = session
        .chars()
        .take(SHOWN_ID_CHARS)
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    format!(
        "worktree {name} cannot resume its session {session}: {reason}; start a fresh session \
         with `orca-term agent start {name} --fresh`"
    )
}

/// The most characters of a session id an error message shows.
const SHOWN_ID_CHARS: usize = 128;

/// The message of [`TypedError::NoSandbox`]: whose sandbox, then the diagnosis `orca-term doctor`
/// shows for the sandbox, in the same words.
fn no_sandbox_message(agent: Agent, missing: &[String], hint: &str) -> String {
    format!(
        "{}'s sandbox is not available: {}; {DOCTOR_HINT}",
        agent.program(),
        sandbox::diagnosis(missing, hint)
    )
}

impl TypedError {
    /// How `--json` names the failure, in `error.kind`.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Busy { .. } => "busy",
            Self::NoSandbox { .. } => "no_sandbox",
            Self::Dirty { .. } => "dirty",
            Self::SessionGone { .. } => "session_gone",
            Self::NoHolder { .. } => "no_holder",
            Self::Prerequisite { .. } => "prerequisite",
            Self::Untrusted { .. } => "untrusted",
        }
    }

    /// The process's exit code for the failure.
    pub(crate) fn exit_code(&self) -> u8 {
        match self {
            Self::Busy { .. } => 3,
            Self::NoSandbox { .. } => 5,
            Self::Dirty { .. } => 6,
            Self::SessionGone { .. } => 4,
            Self::NoHolder { .. } => 7,
            Self::Prerequisite { .. } => 9,
            Self::Untrusted { .. } => 8,
        }
    }
}

/// The typed error inside `err`, at any depth of its context chain.
pub(crate) fn typed(err: &anyhow::Error) -> Option<&TypedError> {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<TypedError>())
}

/// The `error.kind` of `err`: its typed error's kind, or `other`.
pub(crate) fn kind(err: &anyhow::Error) -> &'static str {
    typed(err).map_or(OTHER_KIND, TypedError::kind)
}

/// The exit code of `err`: its typed error's code, or 1.
pub(crate) fn exit_code(err: &anyhow::Error) -> u8 {
    typed(err).map_or(FAILURE_EXIT, TypedError::exit_code)
}

#[cfg(test)]
mod tests {
    use anyhow::{Context as _, anyhow};

    use super::*;

    fn busy(reason: BusyReason) -> TypedError {
        TypedError::Busy {
            name: "fix".to_owned(),
            path: PathBuf::from("/w/repo/fix"),
            reason,
        }
    }

    #[test]
    fn a_busy_worktree_has_its_own_kind_and_exit_code() {
        let err = anyhow::Error::new(busy(BusyReason::Locked));
        assert_eq!(kind(&err), "busy");
        assert_eq!(exit_code(&err), 3);
        assert_eq!(
            err.to_string(),
            "worktree fix at /w/repo/fix is busy: its setup or another orca-term command is \
             running on it; try again once it finishes"
        );
    }

    #[test]
    fn a_worktree_whose_agent_runs_is_busy_and_names_agent_stop() {
        let err = anyhow::Error::new(busy(BusyReason::AgentRunning));
        assert_eq!(kind(&err), "busy");
        assert_eq!(exit_code(&err), 3);
        assert_eq!(
            err.to_string(),
            "worktree fix at /w/repo/fix is busy: its agent is running; stop it first with \
             `orca-term agent stop fix`"
        );
    }

    #[test]
    fn a_missing_sandbox_has_its_own_kind_and_exit_code() {
        let err = anyhow::Error::new(TypedError::NoSandbox {
            agent: Agent::ClaudeCode,
            missing: vec!["bwrap".to_owned(), "socat".to_owned()],
            hint: "install them".to_owned(),
        });
        assert_eq!(kind(&err), "no_sandbox");
        assert_eq!(exit_code(&err), 5);
        assert_eq!(
            err.to_string(),
            "claude's sandbox is not available: bwrap and socat not found on PATH; install them; \
             run orca-term doctor for the full report"
        );
        let err = anyhow::Error::new(TypedError::NoSandbox {
            agent: Agent::OpenCode,
            missing: vec!["bwrap".to_owned()],
            hint: "install it".to_owned(),
        });
        assert_eq!(kind(&err), "no_sandbox");
        assert!(
            err.to_string()
                .starts_with("opencode's sandbox is not available: bwrap not found on PATH"),
            "{err}"
        );
    }

    #[test]
    fn a_worktree_running_another_agent_is_busy_naming_it_and_the_swap() {
        let err = anyhow::Error::new(busy(BusyReason::AnotherAgent {
            running: Agent::OpenCode,
            wanted: Agent::ClaudeCode,
        }));
        assert_eq!(kind(&err), "busy");
        assert_eq!(exit_code(&err), 3);
        assert_eq!(
            err.to_string(),
            "worktree fix at /w/repo/fix is busy: its agent opencode is running; swap it for \
             claude-code with `orca-term agent swap fix claude-code`, or stop it first with \
             `orca-term agent stop fix`"
        );
    }

    #[test]
    fn an_agent_in_a_turn_is_busy_for_a_swap() {
        let err = anyhow::Error::new(busy(BusyReason::InTurn));
        assert_eq!(kind(&err), "busy");
        assert_eq!(exit_code(&err), 3);
        assert_eq!(
            err.to_string(),
            "worktree fix at /w/repo/fix is busy: its agent is in a turn, working or waiting for \
             you; swap agents once the turn ends, or stop it first with `orca-term agent stop fix`"
        );
    }

    #[test]
    fn an_agent_in_a_turn_is_busy_for_a_session_change() {
        let err = anyhow::Error::new(busy(BusyReason::SessionInTurn));
        assert_eq!(kind(&err), "busy");
        assert_eq!(exit_code(&err), 3);
        assert_eq!(
            err.to_string(),
            "worktree fix at /w/repo/fix is busy: its agent is in a turn, working or waiting for \
             you; change its session once the turn ends, or stop it first with `orca-term agent \
             stop fix`"
        );
    }

    #[test]
    fn a_worktree_with_changes_has_its_own_kind_and_exit_code() {
        let err = anyhow::Error::new(TypedError::Dirty {
            name: "wip".to_owned(),
            path: PathBuf::from("/w/repo/wip"),
        });
        assert_eq!(kind(&err), "dirty");
        assert_eq!(exit_code(&err), 6);
        assert_eq!(
            err.to_string(),
            "worktree wip at /w/repo/wip has modified or untracked files; pass --force to remove \
             it anyway, discarding its changes"
        );
    }

    fn gone(reason: GoneReason) -> anyhow::Error {
        anyhow::Error::new(TypedError::SessionGone {
            name: "fix".to_owned(),
            session: "0b983a87-4074-466e-b864-422ce6f063c8".to_owned(),
            reason,
        })
    }

    #[test]
    fn a_session_that_cannot_be_resumed_has_its_own_kind_and_exit_code() {
        let err = gone(GoneReason::TranscriptGone(PathBuf::from("/h/t/s.jsonl")));
        assert_eq!(kind(&err), "session_gone");
        assert_eq!(exit_code(&err), 4);
        assert_eq!(
            err.to_string(),
            "worktree fix cannot resume its session 0b983a87-4074-466e-b864-422ce6f063c8: its \
             transcript /h/t/s.jsonl is gone; start a fresh session with `orca-term agent start \
             fix --fresh`"
        );
        let err = gone(GoneReason::InvalidId).to_string();
        assert!(
            err.contains(": its id is not one an agent makes; "),
            "{err}"
        );
        let err = gone(GoneReason::NoTranscript).to_string();
        assert!(err.contains(": no transcript of it is recorded; "), "{err}");
        let err = gone(GoneReason::NotListed).to_string();
        assert!(
            err.contains(": opencode no longer lists it in `opencode session list`; "),
            "{err}"
        );
    }

    #[test]
    fn a_session_id_is_shown_on_one_short_line() {
        let err = anyhow::Error::new(TypedError::SessionGone {
            name: "fix".to_owned(),
            session: format!("a\nb{}", "x".repeat(500)),
            reason: GoneReason::InvalidId,
        })
        .to_string();
        assert!(!err.contains('\n'), "{err}");
        assert!(err.contains("session a bxxx"), "{err}");
        assert!(!err.contains(&"x".repeat(127)), "{err}");
    }

    #[test]
    fn a_worktree_with_no_holder_has_its_own_kind_and_exit_code() {
        let err = anyhow::Error::new(TypedError::NoHolder {
            name: "fix".to_owned(),
            path: PathBuf::from("/w/repo/fix"),
        });
        assert_eq!(kind(&err), "no_holder");
        assert_eq!(exit_code(&err), 7);
        assert_eq!(
            err.to_string(),
            "worktree fix at /w/repo/fix has no running agent to send notes to; start it with \
             `orca-term agent start fix`, then send again (the notes stay pending)"
        );
    }

    #[test]
    fn a_missing_prerequisite_has_its_own_kind_and_exit_code() {
        let err = anyhow::Error::new(TypedError::Prerequisite {
            problems: vec![
                "git 2.30.1: orca-term needs 2.38 or later".to_owned(),
                "claude: not found on PATH; install Claude Code".to_owned(),
            ],
        });
        assert_eq!(kind(&err), "prerequisite");
        assert_eq!(exit_code(&err), 9);
        assert_eq!(
            err.to_string(),
            "git 2.30.1: orca-term needs 2.38 or later; claude: not found on PATH; install Claude \
             Code; run orca-term doctor for the full report"
        );
    }

    #[test]
    fn a_setup_script_not_approved_has_its_own_kind_and_exit_code() {
        let err = anyhow::Error::new(TypedError::Untrusted {
            name: "fix".to_owned(),
            subject: Subject::Setup {
                script: "make\n".to_owned(),
            },
            file: PathBuf::from("/w/repo/fix/orca-term.yaml"),
        });
        assert_eq!(kind(&err), "untrusted");
        assert_eq!(exit_code(&err), 8);
        assert_eq!(
            err.to_string(),
            "the setup script of worktree fix, from /w/repo/fix/orca-term.yaml, is not approved \
             in this repository, so it did not run and the worktree is not ready; read it, then \
             run `orca-term worktree setup fix` in a terminal to be asked, or pass \
             --approve-setup"
        );
    }

    #[test]
    fn writable_paths_not_approved_are_untrusted_too_and_name_agent_start() {
        let err = anyhow::Error::new(TypedError::Untrusted {
            name: "fix".to_owned(),
            subject: Subject::Sandbox {
                writable: vec!["~".to_owned(), "../shared".to_owned()],
            },
            file: PathBuf::from("/w/repo/fix/orca-term.yaml"),
        });
        assert_eq!(kind(&err), "untrusted");
        assert_eq!(exit_code(&err), 8);
        assert_eq!(
            err.to_string(),
            "the sandbox's extra writable paths of worktree fix (~, ../shared), from \
             /w/repo/fix/orca-term.yaml, are not approved in this repository, so the agent did \
             not start; read them, then run `orca-term agent start fix` in a terminal to be \
             asked, or pass --approve-sandbox"
        );
    }

    #[test]
    fn a_typed_error_is_found_under_context() {
        let err = Err::<(), _>(busy(BusyReason::Locked))
            .context("could not set up")
            .unwrap_err();
        assert_eq!(kind(&err), "busy");
        assert_eq!(exit_code(&err), 3);
    }

    #[test]
    fn any_other_failure_is_other_and_exits_1() {
        let err = anyhow!("git failed");
        assert_eq!(kind(&err), "other");
        assert_eq!(exit_code(&err), 1);
    }
}
