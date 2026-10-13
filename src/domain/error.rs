//! Domain: the failures a client must tell apart (ADR 0010). Each one has a `kind`, which `--json`
//! prints as `error.kind`, and an exit code of its own.
//!
//! A typed error travels inside an `anyhow::Error` like any other, so the code between the failure
//! and `main` needs no conversions; [`kind`] and [`exit_code`] find it again by downcasting. Every
//! other failure has the kind `other` and exits 1. Exit code 2 is clap's, for a usage error.

use std::path::PathBuf;

/// The exit code of a failure that is not typed.
pub(crate) const FAILURE_EXIT: u8 = 1;

/// The `error.kind` of a failure that is not typed.
pub(crate) const OTHER_KIND: &str = "other";

/// A failure the client reacts to on its own, rather than only showing its message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum TypedError {
    /// Another process holds the worktree's lock: its setup, or another orca-term command, is
    /// running on it.
    #[error(
        "worktree {name} at {} is busy: its setup or another orca-term command is running on it; \
         try again once it finishes",
        path.display()
    )]
    Busy { name: String, path: PathBuf },
}

impl TypedError {
    /// How `--json` names the failure, in `error.kind`.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::Busy { .. } => "busy",
        }
    }

    /// The process's exit code for the failure.
    pub(crate) fn exit_code(&self) -> u8 {
        match self {
            Self::Busy { .. } => 3,
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

    fn busy() -> TypedError {
        TypedError::Busy {
            name: "fix".to_owned(),
            path: PathBuf::from("/w/repo/fix"),
        }
    }

    #[test]
    fn a_busy_worktree_has_its_own_kind_and_exit_code() {
        let err = anyhow::Error::new(busy());
        assert_eq!(kind(&err), "busy");
        assert_eq!(exit_code(&err), 3);
        assert_eq!(
            err.to_string(),
            "worktree fix at /w/repo/fix is busy: its setup or another orca-term command is \
             running on it; try again once it finishes"
        );
    }

    #[test]
    fn a_typed_error_is_found_under_context() {
        let err = Err::<(), _>(busy())
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
