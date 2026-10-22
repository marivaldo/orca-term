//! Every recording of a live scenario replays to the turns its scenario expects (seam 1, #44).
//!
//! Each scenario in `tests/live/*.scenario` names its agent, its recording and the outcome of each
//! turn it expects (`expect finished`, `expect interrupted finished`). `just check-live` records
//! what the real agent sent beside the recordings already there, as `<recording>-<version>.log`:
//! claude's hooks in `tests/fixtures/claude-hooks/`, opencode's lines in
//! `tests/fixtures/opencode-events/`. This suite replays every version of every scenario's
//! recording through the real `orca-term hook`, as the agent drives it, and reads the outcomes from
//! the turn log, so a new version of an agent that behaves differently makes it fail, naming the
//! recording. A scenario with no recording yet has nothing to replay.

#![expect(
    clippy::unwrap_used,
    reason = "clippy.toml relaxes unwrap only inside #[test] fns; fixture helpers panic on failure too"
)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "a black-box test builds real repositories and files around the binary it drives"
)]

#[path = "support/agent.rs"]
#[expect(
    dead_code,
    reason = "the agent fixture is shared between suites, and this one leaves part of it unused"
)]
mod fixture;
#[expect(
    dead_code,
    reason = "the fixture is shared between suites, and this one leaves part of it unused"
)]
mod support;

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use fixture::AgentRepo;
use serde_json::Value;

/// Where the scenarios are.
const SCENARIOS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/live");

/// Where the recordings are.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");

/// What this suite reads of a scenario: its agent, its recording and the outcomes it expects.
#[derive(Debug)]
struct Scenario {
    name: String,
    agent: String,
    recording: String,
    expect: Vec<String>,
}

impl Scenario {
    /// Reads the scenario file at `path`; the runner (`cargo xtask check-live`) checks the rest.
    fn read(path: &Path) -> Self {
        let text = fs::read_to_string(path).unwrap();
        let mut scenario = Self {
            name: path.file_stem().unwrap().to_str().unwrap().to_owned(),
            agent: String::new(),
            recording: String::new(),
            expect: Vec::new(),
        };
        for line in text.lines() {
            match line.split_once(' ') {
                Some(("agent", agent)) => agent.clone_into(&mut scenario.agent),
                Some(("recording", recording)) => recording.clone_into(&mut scenario.recording),
                Some(("expect", outcomes)) => {
                    scenario.expect = outcomes.split_whitespace().map(str::to_owned).collect();
                }
                _ => {}
            }
        }
        scenario
    }

    /// The directory the scenario's recordings are in.
    fn dir(&self) -> PathBuf {
        let dir = if self.agent == "opencode" {
            "opencode-events"
        } else {
            "claude-hooks"
        };
        Path::new(FIXTURES).join(dir)
    }

    /// Every recording of the scenario, `<recording>-<version>.log`, oldest version first.
    fn recordings(&self) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(self.dir()) else {
            return Vec::new();
        };
        let mut found: Vec<(Vec<u64>, PathBuf)> = entries
            .map(|entry| entry.unwrap().path())
            .filter_map(|path| {
                let name = path.file_name()?.to_str()?;
                let (recording, version) = name.strip_suffix(".log")?.rsplit_once('-')?;
                let parts: Option<Vec<u64>> = version.split('.').map(|p| p.parse().ok()).collect();
                (recording == self.recording).then_some((parts?, path))
            })
            .collect();
        found.sort();
        found.into_iter().map(|(_, path)| path).collect()
    }
}

/// Every scenario, by name.
fn scenarios() -> Vec<Scenario> {
    let mut paths: Vec<PathBuf> = fs::read_dir(SCENARIOS)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "scenario"))
        .collect();
    paths.sort();
    paths.iter().map(|path| Scenario::read(path)).collect()
}

/// Replays the recording at `path` of `scenario` in a worktree of its own, running
/// `orca-term hook <agent> --admin <dir>` for each hook as the agent does, and returns the outcome
/// of each turn the turn log holds, in order (`open` for one with no end).
fn replay(fx: &AgentRepo, scenario: &Scenario, path: &Path, n: usize) -> Vec<String> {
    let name = format!("r{n}");
    let worktree = fx.new_worktree(&name);
    let admin = fx.admin_dir(&worktree);
    let tmp = fx.repo.tmp.path();
    let text = fs::read_to_string(path).unwrap();
    for line in text.lines().filter(|line| !line.is_empty()) {
        let line = line
            .replace("{{worktree}}", worktree.to_str().unwrap())
            .replace("{{transcripts}}", tmp.join("transcripts").to_str().unwrap())
            .replace("{{denied}}", tmp.join("denied").to_str().unwrap())
            .replace("{{tasks}}", tmp.join("tasks").to_str().unwrap());
        let mut command = fx.repo.command(&worktree);
        if scenario.agent == "opencode" {
            command.args(["hook", "opencode", "--admin", admin.to_str().unwrap()]);
            fire(command, &line);
        } else {
            let (event, payload) = line.split_once(' ').unwrap();
            command.args(["hook", "claude", "--admin", admin.to_str().unwrap(), event]);
            fire(command, payload);
        }
    }
    outcomes(&admin)
}

/// Runs the hook `command` with `stdin` on its standard input; a hook exits 0 and prints nothing.
fn fire(mut command: Command, stdin: &str) {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A hook that exits without reading all of stdin closes the pipe: not an error here.
    let _ = child.stdin.take().unwrap().write_all(stdin.as_bytes());
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stdout.is_empty() && out.stderr.is_empty());
}

/// The outcome of each turn in the turn log of `admin`, in order: its end line's, or `open`.
fn outcomes(admin: &Path) -> Vec<String> {
    let text = fs::read_to_string(admin.join("turns.jsonl")).unwrap_or_default();
    let lines: Vec<Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    lines
        .iter()
        .filter(|line| line["kind"] == "start")
        .map(|start| {
            lines
                .iter()
                .find(|line| line["kind"] == "end" && line["turn"] == start["turn"])
                .map_or("open", |end| end["outcome"].as_str().unwrap())
                .to_owned()
        })
        .collect()
}

#[test]
fn every_recording_replays_to_the_turns_its_scenario_expects() {
    let fx = AgentRepo::new();
    let mut replayed = 0;
    let mut diverged = Vec::new();
    for scenario in scenarios() {
        assert!(
            !scenario.recording.is_empty() && !scenario.expect.is_empty(),
            "{}: names no recording or no outcome",
            scenario.name
        );
        for path in scenario.recordings() {
            replayed += 1;
            let got = replay(&fx, &scenario, &path, replayed);
            if got != scenario.expect {
                diverged.push(format!(
                    "{}: its turns ended {got:?}, but the scenario {} expects {:?}",
                    path.file_name().unwrap().to_str().unwrap(),
                    scenario.name,
                    scenario.expect
                ));
            }
        }
    }
    assert!(replayed > 0, "no recording of any scenario was found");
    assert!(diverged.is_empty(), "{}", diverged.join("\n"));
}

#[test]
fn a_recording_belongs_to_its_scenario_by_name_and_version_only() {
    let scenario = Scenario {
        name: "tui-esc".to_owned(),
        agent: "claude-code".to_owned(),
        recording: "tui-esc".to_owned(),
        expect: Vec::new(),
    };
    let names: Vec<String> = scenario
        .recordings()
        .iter()
        .map(|path| path.file_name().unwrap().to_str().unwrap().to_owned())
        .collect();
    assert!(
        names.contains(&"tui-esc-2.1.300.log".to_owned()),
        "{names:?}"
    );
    assert!(
        names.iter().all(|name| !name.starts_with("tui-esctool")),
        "not tui-esctool's, though its name starts the same: {names:?}"
    );
}
