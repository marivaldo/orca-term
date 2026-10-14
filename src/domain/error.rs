//! Domain: the failures a client must tell apart (ADR 0010). Each one has a `kind`, which `--json`
//! prints as `error.kind`, and an exit code of its own.
//!
//! A typed error travels inside an `anyhow::Error` like any other, so the code between the failure
//! and `main` needs no conversions; [`kind`] and [`exit_code`] find it again by downcasting. Every
//! other failure has the kind `other` and exits 1. Exit code 2 is clap's, for a usage error.

use std::path::{Path, PathBuf};

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
    /// claude's sandbox cannot run on this machine: `missing` names the tools absent from PATH
    /// (empty when the system has no sandbox at all), and `hint` the fix.
    #[error("{}", no_sandbox_message(missing, hint))]
    NoSandbox { missing: Vec<String>, hint: String },
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
    }
}

fn no_sandbox_message(missing: &[String], hint: &str) -> String {
    if missing.is_empty() {
        return format!("claude's sandbox is not available: {hint}");
    }
    format!(
        "claude's sandbox is not available: {} not found on PATH; {hint}",
        missing.join(" and ")
    )
}

impl TypedError {
    /// How `--json` names the failure, in `error.kind`.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Busy { .. } => "busy",
            Self::NoSandbox { .. } => "no_sandbox",
        }
    }

    /// The process's exit code for the failure. 4 is kept for a later slice.
    pub(crate) fn exit_code(&self) -> u8 {
        match self {
            Self::Busy { .. } => 3,
            Self::NoSandbox { .. } => 5,
        }
    }
}

/// The typed error inside `err`, at any depth of its context chain.
fn typed(err: &anyhow::Error) -> Option<&TypedError> {
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
            missing: vec!["bwrap".to_owned(), "socat".to_owned()],
            hint: "install them".to_owned(),
        });
        assert_eq!(kind(&err), "no_sandbox");
        assert_eq!(exit_code(&err), 5);
        assert_eq!(
            err.to_string(),
            "claude's sandbox is not available: bwrap and socat not found on PATH; install them"
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
