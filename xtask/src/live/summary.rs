//! What `check-live` prints once it is done: the agents' versions, each scenario recorded or
//! failed, each suite passed or diverged (with its failing tests), and `git status --short` of the
//! fixtures, for the person to review what changed. Also whether it all went well, for the exit
//! code.

use std::fmt::Write as _;

/// How one scenario went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ScenarioResult {
    /// The recording was written to this file.
    Recorded(String),
    /// Nothing was written, for this reason.
    Failed(String),
    /// Recorded by hand only (`just record-live <name>`), so skipped.
    Manual,
}

/// How one suite went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SuiteResult {
    pub(crate) name: String,
    pub(crate) passed: bool,
    /// The names of its failing tests, when cargo listed them.
    pub(crate) failing: Vec<String>,
}

/// Everything the summary shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Summary {
    /// Each agent driven, as `claude 2.1.306`.
    pub(crate) agents: Vec<String>,
    /// Each scenario's name and how it went.
    pub(crate) scenarios: Vec<(String, ScenarioResult)>,
    pub(crate) suites: Vec<SuiteResult>,
    /// `git status --short` of the fixtures, one line each, or why it could not be read.
    pub(crate) fixtures: Vec<String>,
}

impl Summary {
    /// The suites that diverged.
    fn diverged(&self) -> Vec<&str> {
        self.suites
            .iter()
            .filter(|suite| !suite.passed)
            .map(|suite| suite.name.as_str())
            .collect()
    }

    /// The scenarios that failed.
    fn failed(&self) -> Vec<&str> {
        self.scenarios
            .iter()
            .filter(|(_, result)| matches!(result, ScenarioResult::Failed(_)))
            .map(|(name, _)| name.as_str())
            .collect()
    }

    /// Whether every scenario was recorded and every suite passed: `check-live` exits 0 only then.
    pub(crate) fn all_well(&self) -> bool {
        self.diverged().is_empty() && self.failed().is_empty()
    }

    /// The summary as printed.
    pub(crate) fn render(&self) -> String {
        let width = self
            .scenarios
            .iter()
            .map(|(name, _)| name.len())
            .chain(self.suites.iter().map(|suite| suite.name.len()))
            .max()
            .unwrap_or(0);
        let mut text = format!("check-live: {}\n\nscenarios:\n", self.agents.join(", "));
        for (name, result) in &self.scenarios {
            let manual = format!("recorded by hand: just record-live {name}");
            let (status, detail) = match result {
                ScenarioResult::Recorded(file) => ("recorded", file),
                ScenarioResult::Failed(why) => ("failed", why),
                ScenarioResult::Manual => ("manual", &manual),
            };
            let _ = writeln!(text, "  {status:<9} {name:<width$}  {detail}");
        }
        text.push_str("suites:\n");
        for suite in &self.suites {
            let status = if suite.passed { "passed" } else { "diverged" };
            let line = format!(
                "  {status:<9} {:<width$}  {}",
                suite.name,
                suite.failing.join(", ")
            );
            let _ = writeln!(text, "{}", line.trim_end());
        }
        text.push_str("fixtures (git status --short):\n");
        if self.fixtures.is_empty() {
            text.push_str("  unchanged\n");
        }
        for line in &self.fixtures {
            let _ = writeln!(text, "  {line}");
        }
        let _ = writeln!(text, "{}", self.verdict());
        text
    }

    /// The last line: all well, or what diverged and what failed.
    fn verdict(&self) -> String {
        if self.all_well() {
            return "check-live: every scenario recorded, every suite passed".to_owned();
        }
        let mut parts = Vec::new();
        let diverged = self.diverged();
        if !diverged.is_empty() {
            parts.push(format!("diverged: {}", diverged.join(", ")));
        }
        let failed = self.failed();
        if !failed.is_empty() {
            parts.push(format!("failed: {}", failed.join(", ")));
        }
        format!("check-live: {}", parts.join("; "))
    }
}

/// The failing tests cargo listed at the end of a suite's output: the names indented under the
/// last `failures:` line, up to the blank line after them.
pub(crate) fn failing_tests(output: &str) -> Vec<String> {
    let lines: Vec<&str> = output.lines().collect();
    let Some(start) = lines
        .iter()
        .rposition(|line| line.trim_end() == "failures:")
    else {
        return Vec::new();
    };
    lines[start + 1..]
        .iter()
        .take_while(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> Summary {
        Summary {
            agents: vec!["claude 2.1.306".to_owned(), "opencode 1.18.0".to_owned()],
            scenarios: vec![
                (
                    "p-hello".to_owned(),
                    ScenarioResult::Recorded(
                        "tests/fixtures/claude-hooks/p-hello-2.1.306.log".to_owned(),
                    ),
                ),
                (
                    "tui-perm".to_owned(),
                    ScenarioResult::Failed(
                        "timed out after 180 s waiting for PermissionRequest".to_owned(),
                    ),
                ),
                ("tui-ask".to_owned(), ScenarioResult::Manual),
            ],
            suites: vec![
                SuiteResult {
                    name: "hooks".to_owned(),
                    passed: true,
                    failing: Vec::new(),
                },
                SuiteResult {
                    name: "recordings".to_owned(),
                    passed: false,
                    failing: vec![
                        "every_recording_replays_to_the_turns_its_scenario_expects".to_owned(),
                    ],
                },
            ],
            fixtures: vec!["?? tests/fixtures/claude-hooks/p-hello-2.1.306.log".to_owned()],
        }
    }

    #[test]
    fn the_summary_names_each_scenario_and_suite_and_what_changed() {
        assert_eq!(
            summary().render(),
            "check-live: claude 2.1.306, opencode 1.18.0

scenarios:
  recorded  p-hello     tests/fixtures/claude-hooks/p-hello-2.1.306.log
  failed    tui-perm    timed out after 180 s waiting for PermissionRequest
  manual    tui-ask     recorded by hand: just record-live tui-ask
suites:
  passed    hooks
  diverged  recordings  every_recording_replays_to_the_turns_its_scenario_expects
fixtures (git status --short):
  ?? tests/fixtures/claude-hooks/p-hello-2.1.306.log
check-live: diverged: recordings; failed: tui-perm
"
        );
    }

    #[test]
    fn only_every_scenario_recorded_and_every_suite_passed_is_all_well() {
        let mut summary = summary();
        assert!(!summary.all_well());
        summary.scenarios.remove(1);
        assert!(!summary.all_well(), "a suite still diverged");
        summary.suites.truncate(1);
        assert!(
            summary.all_well(),
            "a scenario recorded by hand is skipped, not failed"
        );
        summary.fixtures.clear();
        let text = summary.render();
        assert!(
            text.contains("fixtures (git status --short):\n  unchanged\n"),
            "{text}"
        );
        assert!(text.ends_with("check-live: every scenario recorded, every suite passed\n"));
        summary.suites.clear();
        summary
            .scenarios
            .push(("x".to_owned(), ScenarioResult::Failed("why".to_owned())));
        assert!(!summary.all_well(), "a failed scenario is not well either");
    }

    #[test]
    fn the_failing_tests_are_read_from_the_end_of_cargos_output() {
        let output = "\
running 2 tests
test a ... ok
test b ... FAILED

failures:

---- b stdout ----
thread 'b' panicked at tests/x.rs:1:1:
boom

failures:
    b
    c

test result: FAILED. 1 passed; 2 failed
";
        assert_eq!(failing_tests(output), ["b", "c"]);
        assert!(failing_tests("test result: ok.").is_empty());
    }
}
