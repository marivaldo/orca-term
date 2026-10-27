//! Schema 1, frozen (#118): the samples in `tests/fixtures/schema-1/` hold one file of every kind
//! of state the core wrote under schema 1. Each test copies them into a fresh repository
//! (`support/schema_samples.rs`) and checks, through the real binary (seam 1), that the current
//! core still reads every one of them, and that reading rewrites none.
//!
//! When a schema rises, these tests keep the old files readable; `cargo xtask schema-fixtures`
//! makes sure the new schema gets samples of its own.

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
#[path = "support/schema_samples.rs"]
mod schema_samples;
#[expect(
    dead_code,
    reason = "the fixture is shared between suites, and this one leaves part of it unused"
)]
mod support;

use fixture::AgentRepo;
use schema_samples::WORKTREE;
use serde_json::{Value, json};

/// The two claude sessions of the samples: the first had one turn, the second, current, two.
const FIRST_SESSION: &str = "2189ccfd-f8cc-4ab3-bff7-c1f97f5673d9";
const CURRENT_SESSION: &str = "518bf6c9-3d1c-4b26-a5aa-504d04051e06";

/// Runs the binary with `args` and `--json`, which must succeed, and parses its document.
fn json_of(fx: &AgentRepo, args: &[&str]) -> Value {
    let mut args = args.to_vec();
    args.push("--json");
    serde_json::from_str(&fx.ok(&args)).unwrap()
}

/// Picks `fields` out of each object of `list`, to compare the parts a test cares about.
fn pick(list: &Value, fields: &[&str]) -> Vec<Value> {
    list.as_array()
        .unwrap()
        .iter()
        .map(|item| {
            Value::Object(
                fields
                    .iter()
                    .map(|f| ((*f).to_owned(), item[*f].clone()))
                    .collect(),
            )
        })
        .collect()
}

/// The worktree as `worktree ls` and `worktree detail` show it, from `worktree.json`,
/// `status.json`, `seen.json`, `notes.json` and `pr.json`.
fn check_the_listed_worktree(listed: &Value) {
    assert_eq!(listed["name"], WORKTREE);
    assert_eq!(
        listed["state"], "finished",
        "status.json: the latest turn finished"
    );
    assert_eq!(listed["activity"], "All done. Second line here.");
    assert_eq!(
        listed["agent"], "claude-code",
        "worktree.json: the recorded agent"
    );
    assert_eq!(
        listed["unseen"], true,
        "seen.json: seen through turn 1 of 3"
    );
    assert_eq!(listed["notes"], 2, "notes.json: two pending notes");
    assert_eq!(listed["last_activity"], 1_793_047_380_849_u64);
    assert_eq!(
        listed["pr"],
        json!({
            "fetched_at": 1_793_047_381_369_u64,
            "number": 12,
            "title": "Teach the app to count",
            "state": "open",
            "url": "https://github.com/o/r/pull/12",
            "checks": "passing",
        }),
        "pr.json: the pull request found"
    );
}

#[test]
fn every_schema_1_file_still_reads_and_reading_rewrites_none() {
    let fx = AgentRepo::new();
    let sampled = schema_samples::build(&fx, 1);

    let fleet = json_of(&fx, &["worktree", "ls"]);
    check_the_listed_worktree(&fleet["worktrees"][0]);

    let detail = json_of(&fx, &["worktree", "detail", WORKTREE]);
    check_the_listed_worktree(&detail["worktree"]);
    assert_eq!(
        detail["setup"]["status"], "succeeded",
        "worktree.json: the setup"
    );

    let turns = json_of(&fx, &["turn", "ls", WORKTREE]);
    assert_eq!(
        pick(
            &turns["turns"],
            &["number", "session", "prompt", "outcome", "end_observed"]
        ),
        [
            json!({"number": 1, "session": FIRST_SESSION, "prompt": "reply with the single word ok", "outcome": "finished", "end_observed": true}),
            json!({"number": 2, "session": CURRENT_SESSION, "prompt": "PERM please", "outcome": "interrupted", "end_observed": false}),
            json!({"number": 3, "session": CURRENT_SESSION, "prompt": "hello again", "outcome": "finished", "end_observed": true}),
        ],
        "turns.jsonl: every turn"
    );
    assert_eq!(
        turns["turns"][2]["end"],
        "9c8a2104649652d8a0ef09c6fd5fcbc2820d8778"
    );

    let notes = json_of(&fx, &["note", "ls", WORKTREE]);
    assert_eq!(
        pick(&notes["notes"], &["id", "path", "current", "body"]),
        [
            json!({"id": 1, "path": "src/app.txt", "current": {"scope": "line", "line": 2}, "body": "Say why two comes before three."}),
            json!({"id": 2, "path": "src/app.txt", "current": {"scope": "file"}, "body": "Add a header to this file."}),
        ],
        "notes.json and the copy of the file the line note was made against"
    );

    let sessions = json_of(&fx, &["session", "ls", WORKTREE]);
    assert_eq!(
        pick(&sessions["sessions"], &["id", "agent", "turns", "current"]),
        [
            json!({"id": CURRENT_SESSION, "agent": "claude-code", "turns": 2, "current": true}),
            json!({"id": FIRST_SESSION, "agent": "claude-code", "turns": 1, "current": false}),
        ],
        "status.json's current session and the turn log's sessions"
    );

    assert_eq!(
        sampled.changed_files(),
        Vec::<std::path::PathBuf>::new(),
        "a read rewrote a state file"
    );
}

#[test]
fn the_schema_1_approvals_still_approve_their_setup_script_and_writable_list() {
    let fx = AgentRepo::new();
    let sampled = schema_samples::build(&fx, 1);

    // Neither command is given an approval nor a terminal to ask at: only the samples' approvals
    // let the committed script run and the committed list widen the sandbox.
    let created = fx.ok(&["worktree", "new", "second"]);
    assert!(created.contains("setup:  succeeded"), "{created}");
    let second = sampled.worktree.parent().unwrap().join("second");
    assert!(
        second.join("setup-ran.txt").is_file(),
        "the setup script ran"
    );

    fx.ok(&["agent", "start", "second"]);
    let writable = fx.settings()["sandbox"]["filesystem"]["allowWrite"].clone();
    let cache = fx.repo.home.join(".cache/demo");
    assert!(
        writable
            .as_array()
            .unwrap()
            .iter()
            .any(|path| path == cache.to_str().unwrap()),
        "the approved list is in the sandbox: {writable}"
    );

    assert_eq!(
        sampled.changed_files(),
        Vec::<std::path::PathBuf>::new(),
        "using the approvals rewrote a state file"
    );
}
