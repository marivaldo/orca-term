//! Domain: the programs orca-term needs on the machine, and what `orca-term doctor` says about
//! each one. A check is ok, a warning, missing, too old, or only information; a missing or too old
//! one stops every command that needs it with the typed `prerequisite` error, whose message is the
//! doctor's own line. Nothing here looks at the machine: the ops ask PATH and `--version`, and
//! this module decides what the answer means. Nothing is ever installed.

use std::fmt;

use anyhow::Result;

use crate::domain::agent::Agent;
use crate::domain::contract::{CONTRACT, VERSION};
use crate::domain::error::TypedError;

/// The oldest claude orca-term launches: the first whose sandbox honours our settings as
/// "admin-required", so the repository's own `.claude/settings*.json` cannot loosen them.
pub(crate) const CLAUDE_FLOOR: &str = "2.1.295";

/// The newest claude orca-term was tested with: the version of the newest recordings of claude's
/// hooks in `tests/fixtures/claude-hooks/` (`*-2.1.306.log`). A newer claude is warned about,
/// never refused. Raise it when new recordings land.
pub(crate) const CLAUDE_TESTED: &str = "2.1.306";

/// The oldest opencode orca-term runs: the version whose plugin API and events the plugin was
/// written against (#35).
pub(crate) const OPENCODE_FLOOR: &str = "1.18.0";

/// The oldest git orca-term supports.
pub(crate) const GIT_FLOOR: &str = "2.38";

/// Whether a program must be there for orca-term to work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Need {
    /// Missing or too old, it is refused.
    Required,
    /// Missing, it is fine; too old, it is only warned about.
    Optional,
}

/// A program orca-term needs, and from which version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Requirement {
    /// The program, looked up on PATH; also the check's name.
    pub(crate) program: &'static str,
    /// The oldest version accepted, as people write it (`2.38`).
    pub(crate) floor: &'static str,
    /// The newest version tested, when a newer one may behave differently.
    pub(crate) tested: Option<&'static str>,
    pub(crate) need: Need,
    /// How to install it, for the message that says it is missing.
    pub(crate) install: &'static str,
}

/// git, which every command but `doctor` and the hooks runs.
pub(crate) const GIT: Requirement = Requirement {
    program: "git",
    floor: GIT_FLOOR,
    tested: None,
    need: Need::Required,
    install: "install git 2.38 or later",
};

/// claude, the agent `agent start` launches.
pub(crate) const CLAUDE: Requirement = Requirement {
    program: Agent::ClaudeCode.program(),
    floor: CLAUDE_FLOOR,
    tested: Some(CLAUDE_TESTED),
    need: Need::Required,
    install: "install Claude Code (https://docs.anthropic.com/en/docs/claude-code)",
};

/// opencode, which `doctor` reports as optional, since a person may use only claude: absent is
/// fine, too old only a warning. Starting it needs it all the same ([`agent_requirement`]).
pub(crate) const OPENCODE: Requirement = Requirement {
    program: Agent::OpenCode.program(),
    floor: OPENCODE_FLOOR,
    tested: None,
    need: Need::Optional,
    install: "install opencode (https://opencode.ai)",
};

/// The agents `doctor` checks, in the order it shows them.
pub(crate) const AGENTS: &[Requirement] = &[CLAUDE, OPENCODE];

/// What `agent start` needs to launch `agent`: its program on PATH, at its floor or later, both
/// required whatever `doctor` says of an agent nobody launches.
pub(crate) const fn agent_requirement(agent: Agent) -> Requirement {
    let requirement = match agent {
        Agent::ClaudeCode => CLAUDE,
        Agent::OpenCode => OPENCODE,
    };
    Requirement {
        need: Need::Required,
        ..requirement
    }
}

/// What the end of every `prerequisite` message says.
pub(crate) const DOCTOR_HINT: &str = "run orca-term doctor for the full report";

/// A `major.minor.patch` version; a missing patch reads as 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Version(u64, u64, u64);

impl Version {
    /// The first version in `text`: the first word starting with a digit (or `v` and a digit),
    /// read up to its first part that is not a number. `git version 2.39.5 (Apple Git-154)` is
    /// 2.39.5, `2.1.303 (Claude Code)` is 2.1.303. It needs at least a major and a minor.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let word = text
            .split_whitespace()
            .map(|word| word.strip_prefix('v').unwrap_or(word))
            .find(|word| word.starts_with(|c: char| c.is_ascii_digit()))?;
        let mut numbers = word.split('.').map_while(leading_number);
        let major = numbers.next()?;
        let minor = numbers.next()?;
        Some(Self(major, minor, numbers.next().unwrap_or(0)))
    }
}

/// The digits `part` starts with, as a number; `None` when it starts with none.
fn leading_number(part: &str) -> Option<u64> {
    let end = part
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(part.len());
    part[..end].parse().ok()
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

/// What asking a program for its version gave.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Probe {
    /// The program is not on PATH.
    NotFound,
    /// `<program> --version` printed this.
    Printed(String),
    /// `<program> --version` could not run or failed, as this says.
    Failed(String),
}

/// How a check came out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    Ok,
    /// Usable, but worth a look.
    Warn,
    /// Not there, or not usable at all: commands that need it refuse.
    Missing,
    /// Older than orca-term needs: commands that need it refuse.
    TooOld,
    /// Not a prerequisite, only information.
    Info,
}

impl Status {
    /// How `--json` names the status.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Missing => "missing",
            Self::TooOld => "too_old",
            Self::Info => "info",
        }
    }

    /// How the text report shows the status.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::TooOld => "too old",
            other => other.name(),
        }
    }

    /// Whether a command that needs the prerequisite must refuse.
    pub(crate) fn refuses(self) -> bool {
        matches!(self, Self::Missing | Self::TooOld)
    }
}

/// One line of the doctor's report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Check {
    /// What is checked: `git`, `sandbox`, `claude`, `opencode`, `signing` (#105) or `contract`.
    pub(crate) name: &'static str,
    pub(crate) status: Status,
    /// What was found, such as a version; `None` when nothing was.
    pub(crate) found: Option<String>,
    /// What orca-term needs, such as the oldest version; `None` when nothing in particular.
    pub(crate) need: Option<String>,
    /// The line the report shows after the status, naming the fix when there is one to make.
    pub(crate) message: String,
}

/// The doctor's report: every check, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Report {
    pub(crate) checks: Vec<Check>,
}

impl Report {
    /// The `prerequisite` error the report stands for, when a check is missing or too old.
    pub(crate) fn refusal(&self) -> Option<TypedError> {
        refusal(&self.checks)
    }
}

/// The check of the program `requirement` names, from what asking it for its version gave.
pub(crate) fn check_program(requirement: &Requirement, probe: &Probe) -> Check {
    let program = requirement.program;
    let optional = requirement.need == Need::Optional;
    let check = |status, found: Option<Version>, message: String| Check {
        name: program,
        status,
        found: found.map(|version| version.to_string()),
        need: Some(requirement.floor.to_owned()),
        message,
    };
    let output = match probe {
        Probe::NotFound if optional => {
            return check(
                Status::Ok,
                None,
                format!("{program} not installed (optional)"),
            );
        }
        Probe::NotFound => {
            let message = format!("{program}: not found on PATH; {}", requirement.install);
            return check(Status::Missing, None, message);
        }
        Probe::Failed(why) => return check(unusable(optional), None, format!("{program}: {why}")),
        Probe::Printed(output) => output.trim(),
    };
    let Some(version) = Version::parse(output) else {
        let message = format!(
            "{program}: could not read its version (`{program} --version` printed `{output}`)"
        );
        return check(unusable(optional), None, message);
    };
    if Version::parse(requirement.floor).is_some_and(|floor| version < floor) {
        let status = if optional {
            Status::Warn
        } else {
            Status::TooOld
        };
        let message = format!(
            "{program} {version}: orca-term needs {} or later",
            requirement.floor
        );
        return check(status, Some(version), message);
    }
    if let Some(tested) = requirement.tested
        && Version::parse(tested).is_some_and(|tested| version > tested)
    {
        let message =
            format!("{program} {version}: newer than the tested {tested}; it may still work");
        return check(Status::Warn, Some(version), message);
    }
    check(Status::Ok, Some(version), format!("{program} {version}"))
}

/// The status of a program whose version cannot be read: missing when it is required.
fn unusable(optional: bool) -> Status {
    if optional {
        Status::Warn
    } else {
        Status::Missing
    }
}

/// The contract this binary speaks and its version: information, never a refusal.
pub(crate) fn contract_check() -> Check {
    Check {
        name: "contract",
        status: Status::Info,
        found: Some(CONTRACT.to_string()),
        need: None,
        message: format!("contract {CONTRACT}, orca-term {VERSION}"),
    }
}

/// The `prerequisite` error for the checks of `checks` that are missing or too old, if any.
pub(crate) fn refusal(checks: &[Check]) -> Option<TypedError> {
    let problems: Vec<String> = checks
        .iter()
        .filter(|check| check.status.refuses())
        .map(|check| check.message.clone())
        .collect();
    if problems.is_empty() {
        return None;
    }
    Some(TypedError::Prerequisite { problems })
}

/// Refuses, as the typed `prerequisite`, when `check` is missing or too old.
pub(crate) fn require(check: &Check) -> Result<()> {
    match refusal(std::slice::from_ref(check)) {
        Some(err) => Err(err.into()),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error;

    fn printed(text: &str) -> Probe {
        Probe::Printed(text.to_owned())
    }

    #[test]
    fn versions_are_read_from_what_the_programs_print() {
        assert_eq!(
            Version::parse("git version 2.55.0\n"),
            Some(Version(2, 55, 0))
        );
        assert_eq!(
            Version::parse("git version 2.39.5 (Apple Git-154)"),
            Some(Version(2, 39, 5))
        );
        assert_eq!(
            Version::parse("2.1.303 (Claude Code)"),
            Some(Version(2, 1, 303))
        );
        assert_eq!(Version::parse("v1.18.2"), Some(Version(1, 18, 2)));
        assert_eq!(Version::parse("2.38"), Some(Version(2, 38, 0)));
        assert_eq!(
            Version::parse("git version 2.45.windows.1"),
            Some(Version(2, 45, 0))
        );
        assert_eq!(Version::parse("1.18.0-beta.2"), Some(Version(1, 18, 0)));
        assert_eq!(Version::parse("Claude Code"), None);
        assert_eq!(Version::parse("7"), None);
        assert_eq!(Version::parse(""), None);
    }

    #[test]
    fn every_floor_and_tested_version_reads_as_a_version() {
        for requirement in [GIT, CLAUDE, OPENCODE] {
            assert!(
                Version::parse(requirement.floor).is_some(),
                "{requirement:?}"
            );
            if let Some(tested) = requirement.tested {
                assert!(Version::parse(tested).is_some(), "{requirement:?}");
                assert!(Version::parse(tested) >= Version::parse(requirement.floor));
            }
        }
        assert_eq!(CLAUDE.program, "claude");
    }

    #[test]
    fn git_is_ok_from_the_floor_and_too_old_below_it() {
        let ok = check_program(&GIT, &printed("git version 2.55.0"));
        assert_eq!(ok.status, Status::Ok);
        assert_eq!(ok.message, "git 2.55.0");
        assert_eq!(ok.found.as_deref(), Some("2.55.0"));
        assert_eq!(ok.need.as_deref(), Some("2.38"));
        let floor = check_program(&GIT, &printed("git version 2.38.0"));
        assert_eq!(floor.status, Status::Ok);
        let old = check_program(&GIT, &printed("git version 2.30.1"));
        assert_eq!(old.status, Status::TooOld);
        assert_eq!(old.message, "git 2.30.1: orca-term needs 2.38 or later");
    }

    #[test]
    fn a_missing_required_program_names_how_to_install_it() {
        let missing = check_program(&CLAUDE, &Probe::NotFound);
        assert_eq!(missing.status, Status::Missing);
        assert_eq!(missing.found, None);
        assert_eq!(
            missing.message,
            "claude: not found on PATH; install Claude Code \
             (https://docs.anthropic.com/en/docs/claude-code)"
        );
        let git = check_program(&GIT, &Probe::NotFound);
        assert_eq!(
            git.message,
            "git: not found on PATH; install git 2.38 or later"
        );
    }

    #[test]
    fn claude_newer_than_the_tested_version_is_a_warning_never_a_refusal() {
        let tested = check_program(&CLAUDE, &printed("2.1.306 (Claude Code)"));
        assert_eq!(tested.status, Status::Ok);
        let floor = check_program(&CLAUDE, &printed("2.1.295 (Claude Code)"));
        assert_eq!(floor.status, Status::Ok);
        let newer = check_program(&CLAUDE, &printed("2.1.310 (Claude Code)"));
        assert_eq!(newer.status, Status::Warn);
        assert_eq!(
            newer.message,
            "claude 2.1.310: newer than the tested 2.1.306; it may still work"
        );
        let old = check_program(&CLAUDE, &printed("2.1.294 (Claude Code)"));
        assert_eq!(old.status, Status::TooOld);
        assert_eq!(
            old.message,
            "claude 2.1.294: orca-term needs 2.1.295 or later"
        );
    }

    #[test]
    fn an_unreadable_version_is_missing_when_required() {
        let garbled = check_program(&CLAUDE, &printed("Claude Code"));
        assert_eq!(garbled.status, Status::Missing);
        assert_eq!(
            garbled.message,
            "claude: could not read its version (`claude --version` printed `Claude Code`)"
        );
        let failed = check_program(
            &GIT,
            &Probe::Failed("git --version failed: boom".to_owned()),
        );
        assert_eq!(failed.status, Status::Missing);
        assert_eq!(failed.message, "git: git --version failed: boom");
    }

    #[test]
    fn opencode_is_optional_absent_and_only_warned_about_when_old() {
        let absent = check_program(&OPENCODE, &Probe::NotFound);
        assert_eq!(absent.status, Status::Ok);
        assert_eq!(absent.message, "opencode not installed (optional)");
        let old = check_program(&OPENCODE, &printed("1.17.9"));
        assert_eq!(old.status, Status::Warn);
        assert_eq!(
            old.message,
            "opencode 1.17.9: orca-term needs 1.18.0 or later"
        );
        assert_eq!(
            check_program(&OPENCODE, &printed("1.18.0")).status,
            Status::Ok
        );
        assert_eq!(
            check_program(&OPENCODE, &printed("??")).status,
            Status::Warn
        );
    }

    #[test]
    fn starting_opencode_requires_it_from_its_floor() {
        let need = agent_requirement(Agent::OpenCode);
        let missing = check_program(&need, &Probe::NotFound);
        assert_eq!(missing.status, Status::Missing);
        assert_eq!(
            missing.message,
            "opencode: not found on PATH; install opencode (https://opencode.ai)"
        );
        let old = check_program(&need, &printed("1.17.9"));
        assert_eq!(old.status, Status::TooOld);
        assert_eq!(
            old.message,
            "opencode 1.17.9: orca-term needs 1.18.0 or later"
        );
        assert_eq!(check_program(&need, &printed("1.18.0")).status, Status::Ok);
        assert_eq!(agent_requirement(Agent::ClaudeCode), CLAUDE);
    }

    #[test]
    fn the_contract_is_information() {
        let check = contract_check();
        assert_eq!(check.status, Status::Info);
        assert_eq!(check.found.as_deref(), Some("1"));
        assert_eq!(check.message, format!("contract 1, orca-term {VERSION}"));
    }

    #[test]
    fn missing_and_too_old_checks_are_the_typed_prerequisite_error() {
        let report = Report {
            checks: vec![
                check_program(&GIT, &printed("git version 2.30.1")),
                check_program(&CLAUDE, &printed("2.1.310 (Claude Code)")),
                check_program(&OPENCODE, &Probe::NotFound),
                contract_check(),
            ],
        };
        let err = anyhow::Error::new(report.refusal().unwrap());
        assert_eq!(error::kind(&err), "prerequisite");
        assert_eq!(error::exit_code(&err), 9);
        assert_eq!(
            err.to_string(),
            "git 2.30.1: orca-term needs 2.38 or later; run orca-term doctor for the full report"
        );
        let warned = check_program(&CLAUDE, &printed("2.1.310 (Claude Code)"));
        require(&warned).unwrap();
        assert!(require(&check_program(&CLAUDE, &Probe::NotFound)).is_err());
    }

    #[test]
    fn each_status_has_its_json_name_and_text_label() {
        let all = [
            Status::Ok,
            Status::Warn,
            Status::Missing,
            Status::TooOld,
            Status::Info,
        ];
        let names: Vec<_> = all.iter().map(|s| s.name()).collect();
        assert_eq!(names, ["ok", "warn", "missing", "too_old", "info"]);
        assert_eq!(Status::TooOld.label(), "too old");
        let refusing: Vec<_> = all.iter().filter(|s| s.refuses()).collect();
        assert_eq!(refusing, [&Status::Missing, &Status::TooOld]);
    }
}
