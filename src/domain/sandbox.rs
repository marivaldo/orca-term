//! Domain: whether claude's native sandbox can run on this machine (ADR 0007). claude runs
//! `sandbox-exec` on macOS and `bwrap` with `socat` on Linux; on any other system it has no
//! sandbox. The adapters say which tools are on PATH; this module decides what is missing and
//! which typed error says so.

use anyhow::Result;

use crate::domain::error::TypedError;

/// The operating systems claude's sandbox knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Platform {
    MacOs,
    Linux,
    Other,
}

impl Platform {
    /// The system this binary was built for.
    pub(crate) fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else if cfg!(target_os = "linux") {
            Self::Linux
        } else {
            Self::Other
        }
    }
}

/// The programs claude's sandbox needs on PATH on `os`.
pub(crate) fn required_tools(os: Platform) -> &'static [&'static str] {
    match os {
        Platform::MacOs => &["sandbox-exec"],
        Platform::Linux => &["bwrap", "socat"],
        Platform::Other => &[],
    }
}

/// Refuses to launch when the sandbox cannot run on `os`: no sandbox exists there, or one of its
/// tools is missing, `found` saying which tools are on PATH. The error is the typed `no_sandbox`.
pub(crate) fn check_available(os: Platform, found: &dyn Fn(&str) -> bool) -> Result<()> {
    let hint = match os {
        Platform::MacOs => "sandbox-exec ships with macOS in /usr/bin; is /usr/bin on PATH?",
        Platform::Linux => "install bubblewrap and socat (`sudo apt-get install bubblewrap socat`)",
        Platform::Other => {
            return Err(TypedError::NoSandbox {
                missing: Vec::new(),
                hint: "claude's sandbox runs on macOS and Linux only".to_owned(),
            }
            .into());
        }
    };
    let missing: Vec<String> = required_tools(os)
        .iter()
        .filter(|tool| !found(tool))
        .map(|tool| (*tool).to_owned())
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    Err(TypedError::NoSandbox {
        missing,
        hint: hint.to_owned(),
    }
    .into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error;

    #[test]
    fn each_os_needs_its_own_tools() {
        assert_eq!(required_tools(Platform::MacOs), ["sandbox-exec"]);
        assert_eq!(required_tools(Platform::Linux), ["bwrap", "socat"]);
        assert!(required_tools(Platform::Other).is_empty());
    }

    #[test]
    fn a_missing_tool_is_the_typed_no_sandbox_error_naming_the_fix() {
        check_available(Platform::Linux, &|_| true).unwrap();
        let err = check_available(Platform::Linux, &|tool| tool == "bwrap").unwrap_err();
        assert_eq!(error::kind(&err), "no_sandbox");
        assert_eq!(error::exit_code(&err), 5);
        assert_eq!(
            err.to_string(),
            "claude's sandbox is not available: socat not found on PATH; install bubblewrap and \
             socat (`sudo apt-get install bubblewrap socat`)"
        );
        let err = check_available(Platform::MacOs, &|_| false).unwrap_err();
        assert!(
            err.to_string().contains("sandbox-exec not found on PATH"),
            "{err}"
        );
    }

    #[test]
    fn another_operating_system_has_no_sandbox_at_all() {
        let err = check_available(Platform::Other, &|_| true).unwrap_err();
        assert_eq!(error::kind(&err), "no_sandbox");
        assert_eq!(
            err.to_string(),
            "claude's sandbox is not available: claude's sandbox runs on macOS and Linux only"
        );
    }
}
