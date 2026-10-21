//! The live checks' recording tap (#44), driven with the fake agents (seam 1): when `agent start`
//! runs with `ORCA_TERM_RECORD` naming a file, the agent inherits it, so claude's hooks append
//! every payload they receive to that file, `<event> <payload>` per line, and the holder's hook
//! appends every line opencode's plugin writes, both as received and in order. Without the
//! variable nothing is recorded anywhere.

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
#[path = "support/opencode.rs"]
#[expect(
    dead_code,
    reason = "the opencode fixture is shared between suites, and this one leaves part of it unused"
)]
mod opencode;
#[expect(
    dead_code,
    reason = "the fixture is shared between suites, and this one leaves part of it unused"
)]
mod support;

use std::fmt::Write as _;
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use fixture::{AgentRepo, wait_until};
use opencode::OpenCodeRepo;
use serde_json::{Value, json};

/// The variable the tap reads.
const RECORD_VAR: &str = "ORCA_TERM_RECORD";

/// A recording of what claude 2.1.300 sent for a plain prompt.
const RECORDING: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/claude-hooks/p-hello-2.1.300.log"
);

/// The recording's hooks as (event, payload), its `{{worktree}}` made `worktree`.
fn recorded_hooks(worktree: &Path) -> Vec<(String, String)> {
    fs::read_to_string(RECORDING)
        .unwrap()
        .lines()
        .map(|line| {
            let (event, payload) = line.split_once(' ').unwrap();
            let payload = payload.replace("{{worktree}}", worktree.to_str().unwrap());
            (event.to_owned(), payload)
        })
        .collect()
}

/// Runs every handler the settings register for `event`, as claude does (the command and its
/// arguments, `payload` on stdin, in the worktree), with `record` as the tap's file when given:
/// claude's hooks inherit claude's environment, which the agent got from `agent start`.
fn fire(settings: &Value, worktree: &Path, event: &str, payload: &str, record: Option<&Path>) {
    for entry in settings["hooks"][event].as_array().into_iter().flatten() {
        for handler in entry["hooks"].as_array().unwrap() {
            let args: Vec<&str> = handler["args"]
                .as_array()
                .unwrap()
                .iter()
                .map(|arg| arg.as_str().unwrap())
                .collect();
            let mut command = Command::new(handler["command"].as_str().unwrap());
            command.args(&args).current_dir(worktree).env_clear();
            if let Some(record) = record {
                command.env(RECORD_VAR, record);
            }
            let mut child = command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap();
            child
                .stdin
                .take()
                .unwrap()
                .write_all(payload.as_bytes())
                .unwrap();
            let out = child.wait_with_output().unwrap();
            assert_eq!(out.status.code(), Some(0), "{event}");
            assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{event}");
        }
    }
}

/// The names of the variables the fake claude was started with.
fn claude_env_names(fx: &AgentRepo) -> Vec<String> {
    fs::read_to_string(fx.log.with_extension("env"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// Runs `command`, an `agent start`, with the tap's file `record` in its environment when given,
/// and without the variable otherwise.
fn start(mut command: Command, record: Option<&Path>) {
    if let Some(record) = record {
        command.env(RECORD_VAR, record);
    } else {
        command.env_remove(RECORD_VAR);
    }
    let out = command.output().unwrap();
    assert!(
        out.status.success(),
        "agent start: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn claudes_hooks_record_each_payload_after_its_event_as_received() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let record = fx.repo.tmp.path().join("hooks.log");
    start(fx.command(&["agent", "start", "fix"]), Some(&record));
    assert!(
        claude_env_names(&fx).iter().any(|name| name == RECORD_VAR),
        "claude inherits the variable, so its hooks do"
    );
    let settings = fx.settings();
    let hooks = recorded_hooks(&worktree);
    for (event, payload) in &hooks {
        fire(&settings, &worktree, event, payload, Some(&record));
    }
    let mut expected = String::new();
    for (event, payload) in &hooks {
        writeln!(expected, "{event} {payload}").unwrap();
    }
    assert_eq!(fs::read_to_string(&record).unwrap(), expected);
    assert_eq!(
        fx.listed("fix")["state"],
        "finished",
        "the hooks still did their work"
    );
}

#[test]
fn without_the_variable_or_with_it_empty_nothing_is_recorded() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    start(fx.command(&["agent", "start", "fix"]), None);
    assert!(
        !claude_env_names(&fx).iter().any(|name| name == RECORD_VAR),
        "orca-term never sets the variable itself"
    );
    let settings = fx.settings();
    let admin = fx.admin_dir(&worktree);
    let empty = PathBuf::new();
    for (event, payload) in recorded_hooks(&worktree) {
        fire(&settings, &worktree, &event, &payload, None);
        fire(&settings, &worktree, &event, &payload, Some(&empty));
    }
    assert_eq!(fx.listed("fix")["state"], "finished");
    let hook_log = fs::read_to_string(admin.join("hook.log")).unwrap_or_default();
    assert!(
        !hook_log.contains("recording tap"),
        "an empty variable leaves the tap off: {hook_log}"
    );
}

#[test]
fn opencodes_lines_are_recorded_by_the_holders_hook_as_received_in_order() {
    let oc = OpenCodeRepo::new();
    let worktree = oc.fx.new_worktree("demo");
    let record = oc.fx.repo.tmp.path().join("events.log");
    start(
        oc.command(&["agent", "start", "demo", "--agent", "opencode"]),
        Some(&record),
    );
    let mut driver = oc.driver("demo");
    let file = worktree.join("README.md");
    let lines = [
        json!({"type": "prompt", "session": "ses_1", "parent": null, "text": "look"}),
        json!({
            "type": "tool", "session": "ses_1", "parent": null, "tool": "read",
            "input": {"filePath": file.to_str().unwrap()}
        }),
        json!({
            "type": "idle", "session": "ses_1", "parent": null, "text": "ok",
            "finish": "stop", "error": null
        }),
    ];
    for line in &lines {
        driver.event(line);
    }
    wait_until("the turn finished", || {
        oc.fx.listed("demo")["state"] == "finished"
    });
    driver.detach();
    let mut expected = String::new();
    for line in &lines {
        writeln!(expected, "{line}").unwrap();
    }
    assert_eq!(fs::read_to_string(&record).unwrap(), expected);
}
