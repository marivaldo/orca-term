//! The live checks (#44): `cargo xtask check-live [--orca-term <path>] [<scenario>...]`, which
//! `just check-live` runs and CI never does. It drives the installed claude and opencode through
//! the real orca-term, re-records what they send in place, then runs the suites that replay the
//! recordings:
//!
//! 1. it checks the agents the scenarios need with `orca-term doctor --json`, and refuses when one
//!    is missing or older than orca-term needs (opencode too, which `doctor` calls optional);
//! 2. it runs each scenario of `tests/live/` (`live/scenario.rs`) in a throwaway repository
//!    (`live/drive.rs`), and writes what the agent sent, normalised, to
//!    `tests/fixtures/<claude-hooks|opencode-events>/<recording>-<version>.log`
//!    (`live/recording.rs`): a new version lands beside the old ones, so `git diff` shows what
//!    changed;
//! 3. it runs the suites that replay the recordings ([`SUITES`]), each with `cargo test`;
//! 4. it prints the summary (`live/summary.rs`) and exits 1 when a suite diverged or a scenario
//!    failed, 2 when it refused or could not start.
//!
//! `cargo xtask record-live <scenario>` records one scenario by hand, attaching this terminal to
//! the agent (`just record-live <scenario>`): for what cannot be typed reliably.

mod drive;
mod recording;
mod scenario;
mod summary;

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

use scenario::{Agent, Scenario};
use summary::{ScenarioResult, SuiteResult, Summary};

/// Where the scenarios are, from the repository's root.
const SCENARIOS: &str = "tests/live";

/// Where the recordings are, from the repository's root.
const FIXTURES: &str = "tests/fixtures";

/// The suites that replay the recordings: every recording of every scenario (`recordings`),
/// claude's recorded hooks (`hooks`, `snapshots`), opencode's lines (`opencode`), and the tested
/// claude, which must be the newest one recorded (`doctor`).
const SUITES: &[&str] = &["recordings", "hooks", "snapshots", "opencode", "doctor"];

const USAGE: &str = "usage: cargo xtask check-live [--orca-term <path>] [<scenario>...] | \
                     record-live [--orca-term <path>] <scenario>";

/// `check-live`: records the scenarios named (all but those recorded by hand when none is), runs
/// the suites and prints the summary.
pub(crate) fn check_live(args: &[&str]) -> ExitCode {
    match check(args) {
        Ok(summary) => {
            say(&summary.render());
            if summary.all_well() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(err) => {
            warn(&format!("check-live: {err:#}"));
            ExitCode::from(2)
        }
    }
}

/// `record-live <scenario>`: records one scenario by hand, attaching this terminal to the agent.
pub(crate) fn record_live(args: &[&str]) -> ExitCode {
    match record_by_hand(args) {
        Ok(file) => {
            say(&format!(
                "record-live: recorded {file}\nreview it with git diff, then run just check-live"
            ));
            ExitCode::SUCCESS
        }
        Err(err) => {
            warn(&format!("record-live: {err:#}"));
            ExitCode::from(2)
        }
    }
}

fn check(args: &[&str]) -> Result<Summary> {
    let options = Options::parse(args)?;
    let all = read_scenarios(Path::new(SCENARIOS))?;
    let chosen = choose(&all, &options.names)?;
    let orca_term = options.orca_term()?;
    let needed: Vec<Agent> = chosen
        .iter()
        .filter(|scenario| !scenario.manual)
        .map(|scenario| scenario.agent)
        .collect();
    let versions = check_agents(&orca_term, &needed)?;
    let mut scenarios = Vec::new();
    for scenario in chosen {
        let result = if scenario.manual {
            ScenarioResult::Manual
        } else {
            warn(&format!(
                "check-live: {} ({})...",
                scenario.name,
                scenario.agent.name()
            ));
            record_scenario(&orca_term, scenario, &versions)
        };
        scenarios.push((scenario.name.clone(), result));
    }
    warn("check-live: the suites that replay the recordings...");
    let suites = SUITES.iter().map(|suite| run_suite(suite)).collect();
    Ok(Summary {
        agents: versions
            .iter()
            .map(|(program, version)| format!("{program} {version}"))
            .collect(),
        scenarios,
        suites,
        fixtures: fixtures_status(),
    })
}

fn record_by_hand(args: &[&str]) -> Result<String> {
    let options = Options::parse(args)?;
    let [name] = options.names.as_slice() else {
        bail!(USAGE);
    };
    let all = read_scenarios(Path::new(SCENARIOS))?;
    let scenario = choose(&all, std::slice::from_ref(name))?
        .into_iter()
        .next()
        .context("no scenario")?;
    let orca_term = options.orca_term()?;
    let versions = check_agents(&orca_term, &[scenario.agent])?;
    warn(&format!(
        "record-live: {} ({}), in a throwaway repository:\n\n{}\n",
        scenario.name,
        scenario.agent.name(),
        scenario.about.join("\n")
    ));
    let manual = Scenario {
        manual: true,
        ..scenario.clone()
    };
    match record_scenario(&orca_term, &manual, &versions) {
        ScenarioResult::Recorded(file) => Ok(file),
        ScenarioResult::Failed(why) => bail!("{why}"),
        ScenarioResult::Manual => bail!("nothing was recorded"),
    }
}

/// What `check-live` was asked.
#[derive(Debug, Default, PartialEq, Eq)]
struct Options {
    /// The orca-term to drive; `cargo build` makes one when none is given.
    orca_term: Option<PathBuf>,
    /// The scenarios to run; every one when empty.
    names: Vec<String>,
}

impl Options {
    fn parse(args: &[&str]) -> Result<Self> {
        let mut options = Self::default();
        let mut words = args.iter();
        while let Some(word) = words.next() {
            match *word {
                "--orca-term" => {
                    let path = words.next().context(USAGE)?;
                    options.orca_term = Some(PathBuf::from(path));
                }
                flag if flag.starts_with('-') => bail!(USAGE),
                name => options.names.push(name.to_owned()),
            }
        }
        Ok(options)
    }

    /// The orca-term to drive, by its absolute path: the one given, else the debug build `cargo
    /// build` makes.
    fn orca_term(&self) -> Result<PathBuf> {
        let path = if let Some(path) = &self.orca_term {
            path.clone()
        } else {
            warn("check-live: cargo build...");
            let status = Command::new(cargo())
                .args(["build", "--locked", "--quiet", "--package", "orca-term"])
                .status()
                .context("could not run cargo")?;
            if !status.success() {
                bail!("cargo build failed");
            }
            let target = std::env::var_os("CARGO_TARGET_DIR").unwrap_or_else(|| "target".into());
            PathBuf::from(target).join("debug/orca-term")
        };
        path.canonicalize()
            .with_context(|| format!("no orca-term at {}", path.display()))
    }
}

/// cargo, as `CARGO` names it when cargo runs this (`cargo xtask`), else from PATH.
fn cargo() -> OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

/// Every scenario in `dir`, by name.
fn read_scenarios(dir: &Path) -> Result<Vec<Scenario>> {
    let mut paths: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("could not list {}", dir.display()))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "scenario"))
        .collect();
    paths.sort();
    let mut scenarios: Vec<Scenario> = Vec::new();
    for path in paths {
        let name = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .context("a scenario's file name is not text")?;
        let text = fs::read_to_string(&path)?;
        let scenario = scenario::parse(name, &text)
            .map_err(|why| anyhow::anyhow!("{}: {why}", path.display()))?;
        if let Some(other) = scenarios
            .iter()
            .find(|other| other.agent == scenario.agent && other.recording == scenario.recording)
        {
            bail!(
                "{}: the recording {} is {}'s already",
                path.display(),
                scenario.recording,
                other.name
            );
        }
        scenarios.push(scenario);
    }
    Ok(scenarios)
}

/// The scenarios `names` names, in that order, or every one when it names none.
fn choose<'a>(all: &'a [Scenario], names: &[String]) -> Result<Vec<&'a Scenario>> {
    if names.is_empty() {
        return Ok(all.iter().collect());
    }
    names
        .iter()
        .map(|name| {
            all.iter()
                .find(|scenario| scenario.name == *name)
                .with_context(|| format!("no scenario {name} in {SCENARIOS}"))
        })
        .collect()
}

/// Asks `orca-term doctor --json` about git, the sandbox and the agents `needed`, and returns each
/// needed agent's version as (program, version), or refuses naming every problem.
fn check_agents(orca_term: &Path, needed: &[Agent]) -> Result<Vec<(String, String)>> {
    let out = Command::new(orca_term)
        .args(["doctor", "--json"])
        .output()
        .context("could not run orca-term doctor")?;
    let doc: Value = serde_json::from_slice(&out.stdout).context("doctor printed no JSON")?;
    let checks = doc["checks"].as_array().cloned().unwrap_or_default();
    let programs: Vec<&str> = [Agent::ClaudeCode, Agent::OpenCode]
        .into_iter()
        .filter(|agent| needed.contains(agent))
        .map(Agent::program)
        .collect();
    let (versions, problems) = judge(&checks, &programs);
    if !problems.is_empty() {
        bail!(
            "refused (orca-term doctor tells more):\n  {}",
            problems.join("\n  ")
        );
    }
    Ok(versions)
}

/// What `doctor`'s `checks` say of git, the sandbox and each agent of `programs`: each agent's
/// version, and every problem. git and the sandbox follow `doctor` (missing or too old refuses);
/// an agent needs a version at least the one orca-term needs, as `agent start` does, even opencode,
/// which `doctor` calls optional.
fn judge(checks: &[Value], programs: &[&str]) -> (Vec<(String, String)>, Vec<String>) {
    let check = |name: &str| checks.iter().find(|check| check["name"] == name);
    let refuses = |check: &Value| matches!(check["status"].as_str(), Some("missing" | "too_old"));
    let message = |check: &Value| check["message"].as_str().unwrap_or_default().to_owned();
    let mut problems: Vec<String> = ["git", "sandbox"]
        .into_iter()
        .filter_map(check)
        .filter(|check| refuses(check))
        .map(message)
        .collect();
    let mut versions = Vec::new();
    for program in programs {
        let Some(check) = check(program) else {
            problems.push(format!("{program}: doctor does not check it"));
            continue;
        };
        let found = check["found"].as_str();
        let need = check["need"].as_str().unwrap_or("0");
        match found {
            _ if refuses(check) => problems.push(message(check)),
            None => problems.push(format!(
                "{program} is not installed, and the chosen scenarios need it"
            )),
            Some(found) if older(found, need) => {
                problems.push(format!(
                    "{program} {found}: orca-term needs {need} or later"
                ));
            }
            Some(found) => versions.push(((*program).to_owned(), found.to_owned())),
        }
    }
    (versions, problems)
}

/// Whether the version `found` is older than `need`, both dotted numbers.
fn older(found: &str, need: &str) -> bool {
    let numbers = |text: &str| -> Vec<u64> {
        text.split('.')
            .map(|part| part.parse().unwrap_or(0))
            .collect()
    };
    numbers(found) < numbers(need)
}

/// Runs `scenario` and writes its recording, named after its agent's version in `versions`.
fn record_scenario(
    orca_term: &Path,
    scenario: &Scenario,
    versions: &[(String, String)],
) -> ScenarioResult {
    let Some((_, version)) = versions
        .iter()
        .find(|(program, _)| program == scenario.agent.program())
    else {
        return ScenarioResult::Failed("its agent's version is unknown".to_owned());
    };
    let text = match drive::record(orca_term, scenario) {
        Ok(text) => text,
        Err(why) => return ScenarioResult::Failed(why),
    };
    let dir = Path::new(FIXTURES).join(scenario.agent.recordings_dir());
    let file = dir.join(recording::file_name(&scenario.recording, version));
    let written = fs::create_dir_all(&dir).and_then(|()| fs::write(&file, &text));
    if let Err(err) = written {
        return ScenarioResult::Failed(format!("could not write {}: {err}", file.display()));
    }
    let mut shown = file.display().to_string();
    if let Some(home) = std::env::var_os("HOME").filter(|home| !home.is_empty())
        && let Some(home) = home.to_str()
        && text.contains(home)
    {
        shown.push_str(" (it still holds your home directory: review it before committing)");
    }
    ScenarioResult::Recorded(shown)
}

/// Runs the suite `name` with `cargo test`: passed, or diverged with its failing tests.
fn run_suite(name: &str) -> SuiteResult {
    let out = Command::new(cargo())
        .args(["test", "--locked", "--quiet", "--test", name])
        .output();
    match out {
        Ok(out) => {
            let mut text = String::from_utf8_lossy(&out.stdout).into_owned();
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            SuiteResult {
                name: name.to_owned(),
                passed: out.status.success(),
                failing: summary::failing_tests(&text),
            }
        }
        Err(err) => SuiteResult {
            name: name.to_owned(),
            passed: false,
            failing: vec![format!("could not run cargo: {err}")],
        },
    }
}

/// `git status --short` of the fixtures, one line each.
fn fixtures_status() -> Vec<String> {
    let out = Command::new("git")
        .args(["status", "--short", "--", FIXTURES])
        .output();
    match out {
        Ok(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_owned)
            .collect(),
        Ok(out) => vec![format!(
            "(git status failed: {})",
            String::from_utf8_lossy(&out.stderr).trim()
        )],
        Err(err) => vec![format!("(could not run git: {err})")],
    }
}

fn say(text: &str) {
    #[expect(clippy::print_stdout, reason = "the summary is check-live's output")]
    {
        print!("{text}");
        if !text.ends_with('\n') {
            println!();
        }
    }
}

fn warn(text: &str) {
    #[expect(clippy::print_stderr, reason = "progress and refusals go to stderr")]
    {
        eprintln!("{text}");
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    /// The repository's root, from the xtask's own directory.
    fn root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
    }

    #[test]
    fn every_scenario_in_the_tree_reads_and_names_a_recording_of_its_own() {
        let scenarios = read_scenarios(&root().join(SCENARIOS)).unwrap();
        let names: Vec<&str> = scenarios.iter().map(|s| s.name.as_str()).collect();
        for name in [
            "p-hello",
            "tui-esc",
            "tui-perm",
            "tui-sandbox",
            "opencode-hello",
            "opencode-esc",
            "opencode-deny",
        ] {
            assert!(names.contains(&name), "{name} is missing from {names:?}");
        }
    }

    #[test]
    fn each_suite_check_live_runs_is_a_test_file() {
        for suite in SUITES {
            assert!(
                root().join("tests").join(format!("{suite}.rs")).is_file(),
                "no tests/{suite}.rs"
            );
        }
    }

    #[test]
    fn no_workflow_and_no_other_recipe_runs_the_live_checks() {
        let mut problems = Vec::new();
        let workflows = root().join(".github/workflows");
        for entry in fs::read_dir(workflows).unwrap() {
            let path = entry.unwrap().path();
            if fs::read_to_string(&path).unwrap().contains("check-live") {
                problems.push(format!("{} runs check-live", path.display()));
            }
        }
        let justfile = fs::read_to_string(root().join("justfile")).unwrap();
        problems.extend(recipes_calling(&justfile, "check-live"));
        assert!(problems.is_empty(), "{problems:?}");
        assert!(justfile.contains("\ncheck-live "), "the recipe is there");
    }

    /// The recipes of `justfile`, other than `recipe` itself, that mention `recipe` in their
    /// header (as a dependency) or their body.
    fn recipes_calling(justfile: &str, recipe: &str) -> Vec<String> {
        let mut current = String::new();
        let mut found = Vec::new();
        for line in justfile.lines() {
            let header = !line.starts_with([' ', '\t', '#']) && line.contains(':');
            if header {
                current = line.split([' ', ':']).next().unwrap_or_default().to_owned();
            }
            let mentions = line
                .split(|c: char| c.is_whitespace() || c == ':')
                .any(|w| w == recipe);
            if mentions && current != recipe && !line.starts_with('#') {
                found.push(format!("the recipe {current} runs {recipe}"));
            }
        }
        found
    }

    #[test]
    fn a_recipe_calling_the_live_checks_is_found() {
        let justfile = "check: rust check-live\n\ncheck-live *s:\n    cargo xtask check-live\n\nlua:\n    just check-live\n";
        assert_eq!(
            recipes_calling(justfile, "check-live"),
            [
                "the recipe check runs check-live",
                "the recipe lua runs check-live"
            ]
        );
    }

    #[test]
    fn options_take_the_binary_and_the_scenarios() {
        assert_eq!(Options::parse(&[]).unwrap(), Options::default());
        assert_eq!(
            Options::parse(&["--orca-term", "/b/orca-term", "p-hello", "tui-esc"]).unwrap(),
            Options {
                orca_term: Some(PathBuf::from("/b/orca-term")),
                names: vec!["p-hello".to_owned(), "tui-esc".to_owned()],
            }
        );
        assert!(Options::parse(&["--orca-term"]).is_err());
        assert!(Options::parse(&["--fast"]).is_err());
    }

    fn check(name: &str, status: &str, found: Option<&str>, need: &str) -> Value {
        json!({"name": name, "status": status, "found": found, "need": need, "message": format!("{name} says {status}")})
    }

    #[test]
    fn the_agents_are_checked_as_doctor_does_and_opencode_is_required() {
        let checks = [
            check("git", "ok", Some("2.55.0"), "2.38"),
            check("sandbox", "ok", Some("sandbox-exec"), ""),
            check("claude", "warn", Some("2.1.306"), "2.1.295"),
            check("opencode", "ok", None, "1.18.0"),
        ];
        let (versions, problems) = judge(&checks, &["claude"]);
        assert_eq!(
            versions,
            [("claude".to_owned(), "2.1.306".to_owned())],
            "newer than tested is fine"
        );
        assert!(problems.is_empty());
        let (_, problems) = judge(&checks, &["claude", "opencode"]);
        assert_eq!(
            problems,
            ["opencode is not installed, and the chosen scenarios need it"]
        );
        let old = [check("opencode", "warn", Some("1.17.9"), "1.18.0")];
        assert_eq!(
            judge(&old, &["opencode"]).1,
            ["opencode 1.17.9: orca-term needs 1.18.0 or later"]
        );
        let missing = [
            check("git", "too_old", Some("2.30.0"), "2.38"),
            check("sandbox", "missing", None, ""),
            check("claude", "missing", None, "2.1.295"),
        ];
        assert_eq!(
            judge(&missing, &["claude"]).1,
            [
                "git says too_old",
                "sandbox says missing",
                "claude says missing"
            ]
        );
    }

    #[test]
    fn versions_compare_by_their_numbers() {
        assert!(older("1.17.9", "1.18.0"));
        assert!(older("2.1.99", "2.1.295"));
        assert!(!older("2.1.295", "2.1.295"));
        assert!(!older("1.18.10", "1.18.9"));
    }
}
