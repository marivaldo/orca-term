//! Domain: whether an agent's sandbox can run on this machine (ADR 0007). claude's native sandbox
//! runs `sandbox-exec` on macOS and `bwrap` with `socat` on Linux; opencode runs inside the
//! sandbox orca-term wraps it in (`sandbox/policy.rs`, #35), `sandbox-exec` on macOS and `bwrap`
//! on Linux. On any other system there is no sandbox. The adapters say which tools are on PATH;
//! this module decides what is missing, what `orca-term doctor` shows for it and which typed error
//! says so, in the same words.

pub(crate) mod policy;
pub(crate) mod signing;

use anyhow::Result;

use crate::domain::agent::Agent;
use crate::domain::error::TypedError;
use crate::domain::prerequisite::{Check, Status};

/// The operating systems the sandboxes know.
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

/// The programs `agent`'s sandbox needs on PATH on `os`. opencode's needs no `socat`: that is
/// claude's own proxy.
pub(crate) fn required_tools(os: Platform, agent: Agent) -> &'static [&'static str] {
    match (os, agent) {
        (Platform::MacOs, _) => &["sandbox-exec"],
        (Platform::Linux, Agent::ClaudeCode) => &["bwrap", "socat"],
        (Platform::Linux, Agent::OpenCode) => &["bwrap"],
        (Platform::Other, _) => &[],
    }
}

/// Why `agent`'s sandbox cannot run on `os`, `found` saying which tools are on PATH: the tools
/// missing (none when the system has no sandbox at all) and the fix; `None` when it can run.
fn problem(
    os: Platform,
    agent: Agent,
    found: &dyn Fn(&str) -> bool,
) -> Option<(Vec<String>, String)> {
    let hint = match (os, agent) {
        (Platform::MacOs, _) => {
            "sandbox-exec ships with macOS in /usr/bin; is /usr/bin on PATH?".to_owned()
        }
        (Platform::Linux, Agent::ClaudeCode) => {
            "install bubblewrap and socat (`sudo apt-get install bubblewrap socat`)".to_owned()
        }
        (Platform::Linux, Agent::OpenCode) => {
            "install bubblewrap (`sudo apt-get install bubblewrap`)".to_owned()
        }
        (Platform::Other, _) => {
            let message = format!("{}'s sandbox runs on macOS and Linux only", agent.program());
            return Some((Vec::new(), message));
        }
    };
    let missing: Vec<String> = required_tools(os, agent)
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
/// its tools, or missing with the same diagnosis as the `no_sandbox` error. It checks claude's
/// tools, which include everything opencode's sandbox needs.
pub(crate) fn check(os: Platform, found: &dyn Fn(&str) -> bool) -> Check {
    let agent = Agent::ClaudeCode;
    let (status, found, message) = match problem(os, agent, found) {
        None => {
            let tools = required_tools(os, agent).join(" and ");
            (Status::Ok, Some(tools.clone()), format!("sandbox {tools}"))
        }
        Some((missing, hint)) => (
            Status::Missing,
            None,
            format!("sandbox: {}", diagnosis(&missing, &hint)),
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

/// Refuses to launch `agent` when its sandbox cannot run on `os`: no sandbox exists there, or one
/// of its tools is missing, `found` saying which tools are on PATH. The error is the typed
/// `no_sandbox`.
pub(crate) fn check_available(
    os: Platform,
    agent: Agent,
    found: &dyn Fn(&str) -> bool,
) -> Result<()> {
    match problem(os, agent, found) {
        None => Ok(()),
        Some((missing, hint)) => Err(TypedError::NoSandbox {
            agent,
            missing,
            hint,
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
        assert_eq!(
            required_tools(Platform::MacOs, Agent::ClaudeCode),
            ["sandbox-exec"]
        );
        assert_eq!(
            required_tools(Platform::Linux, Agent::ClaudeCode),
            ["bwrap", "socat"]
        );
        assert!(required_tools(Platform::Other, Agent::ClaudeCode).is_empty());
    }

    #[test]
    fn opencodes_sandbox_needs_sandbox_exec_or_bwrap_alone() {
        assert_eq!(
            required_tools(Platform::MacOs, Agent::OpenCode),
            ["sandbox-exec"]
        );
        assert_eq!(required_tools(Platform::Linux, Agent::OpenCode), ["bwrap"]);
        check_available(Platform::Linux, Agent::OpenCode, &|tool| tool == "bwrap").unwrap();
        let err = check_available(Platform::Linux, Agent::OpenCode, &|_| false).unwrap_err();
        assert_eq!(error::kind(&err), "no_sandbox");
        assert_eq!(error::exit_code(&err), 5);
        assert_eq!(
            err.to_string(),
            "opencode's sandbox is not available: bwrap not found on PATH; install bubblewrap \
             (`sudo apt-get install bubblewrap`); run orca-term doctor for the full report"
        );
        let err = check_available(Platform::Other, Agent::OpenCode, &|_| true).unwrap_err();
        assert!(
            err.to_string()
                .contains("opencode's sandbox runs on macOS and Linux only"),
            "{err}"
        );
    }

    #[test]
    fn a_missing_tool_is_the_typed_no_sandbox_error_naming_the_fix() {
        check_available(Platform::Linux, Agent::ClaudeCode, &|_| true).unwrap();
        let err = check_available(Platform::Linux, Agent::ClaudeCode, &|tool| tool == "bwrap")
            .unwrap_err();
        assert_eq!(error::kind(&err), "no_sandbox");
        assert_eq!(error::exit_code(&err), 5);
        assert_eq!(
            err.to_string(),
            "claude's sandbox is not available: socat not found on PATH; install bubblewrap and \
             socat (`sudo apt-get install bubblewrap socat`); run orca-term doctor for the full \
             report"
        );
        let err = check_available(Platform::MacOs, Agent::ClaudeCode, &|_| false).unwrap_err();
        assert!(
            err.to_string().contains("sandbox-exec not found on PATH"),
            "{err}"
        );
    }

    #[test]
    fn another_operating_system_has_no_sandbox_at_all() {
        let err = check_available(Platform::Other, Agent::ClaudeCode, &|_| true).unwrap_err();
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
            let err = check_available(os, Agent::ClaudeCode, found)
                .unwrap_err()
                .to_string();
            assert!(err.contains(diagnosis), "{err} / {diagnosis}");
        }
    }
}
