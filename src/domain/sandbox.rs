//! Domain: whether claude's native sandbox can run on this machine (ADR 0007). claude runs
//! `sandbox-exec` on macOS and `bwrap` with `socat` on Linux; on any other system it has no
//! sandbox. The adapters say which tools are on PATH; this module decides what is missing, what
//! `orca-term doctor` shows for it and which typed error says so, in the same words.

use anyhow::Result;

use crate::domain::error::TypedError;
use crate::domain::prerequisite::{Check, Status};

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

/// Why the sandbox cannot run on `os`, `found` saying which tools are on PATH: the tools missing
/// (none when the system has no sandbox at all) and the fix; `None` when it can run.
fn problem(os: Platform, found: &dyn Fn(&str) -> bool) -> Option<(Vec<String>, &'static str)> {
    let hint = match os {
        Platform::MacOs => "sandbox-exec ships with macOS in /usr/bin; is /usr/bin on PATH?",
        Platform::Linux => "install bubblewrap and socat (`sudo apt-get install bubblewrap socat`)",
        Platform::Other => {
            return Some((Vec::new(), "claude's sandbox runs on macOS and Linux only"));
        }
    };
    let missing: Vec<String> = required_tools(os)
        .iter()
        .filter(|tool| !found(tool))
        .map(|tool| (*tool).to_owned())
        .collect();
    if missing.is_empty() {
        return None;
    }
    Some((missing, hint))
}

/// What is wrong with the sandbox, as both `orca-term doctor` and the `no_sandbox` error say it:
/// the `missing` tools and the `hint`, or the hint alone when no tool is named.
pub(crate) fn diagnosis(missing: &[String], hint: &str) -> String {
    if missing.is_empty() {
        return hint.to_owned();
    }
    format!("{} not found on PATH; {hint}", missing.join(" and "))
}

/// The doctor's check of the sandbox on `os`, `found` saying which tools are on PATH: ok naming
/// its tools, or missing with the same diagnosis as the `no_sandbox` error.
pub(crate) fn check(os: Platform, found: &dyn Fn(&str) -> bool) -> Check {
    let (status, found, message) = match problem(os, found) {
        None => {
            let tools = required_tools(os).join(" and ");
            (Status::Ok, Some(tools.clone()), format!("sandbox {tools}"))
        }
        Some((missing, hint)) => (
            Status::Missing,
            None,
            format!("sandbox: {}", diagnosis(&missing, hint)),
        ),
    };
    Check {
        name: "sandbox",
        status,
        found,
        need: None,
        message,
    }
}

/// Refuses to launch when the sandbox cannot run on `os`: no sandbox exists there, or one of its
/// tools is missing, `found` saying which tools are on PATH. The error is the typed `no_sandbox`.
pub(crate) fn check_available(os: Platform, found: &dyn Fn(&str) -> bool) -> Result<()> {
    match problem(os, found) {
        None => Ok(()),
        Some((missing, hint)) => Err(TypedError::NoSandbox {
            missing,
            hint: hint.to_owned(),
        }
        .into()),
    }
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
             socat (`sudo apt-get install bubblewrap socat`); run orca-term doctor for the full \
             report"
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
            "claude's sandbox is not available: claude's sandbox runs on macOS and Linux only; \
             run orca-term doctor for the full report"
        );
    }

    #[test]
    fn the_doctor_check_names_the_tools_or_says_what_the_error_says() {
        let ok = check(Platform::Linux, &|_| true);
        assert_eq!(ok.status, Status::Ok);
        assert_eq!(ok.message, "sandbox bwrap and socat");
        assert_eq!(ok.found.as_deref(), Some("bwrap and socat"));
        assert_eq!(
            check(Platform::MacOs, &|_| true).message,
            "sandbox sandbox-exec"
        );
        for (os, found) in [
            (
                Platform::Linux,
                &(|tool: &str| tool == "bwrap") as &dyn Fn(&str) -> bool,
            ),
            (Platform::MacOs, &|_: &str| false),
            (Platform::Other, &|_: &str| true),
        ] {
            let missing = check(os, found);
            assert_eq!(missing.status, Status::Missing);
            assert_eq!(missing.found, None);
            let diagnosis = missing.message.strip_prefix("sandbox: ").unwrap();
            let err = check_available(os, found).unwrap_err().to_string();
            assert!(err.contains(diagnosis), "{err} / {diagnosis}");
        }
    }
}
