//! The turn commands (#38), `turn ls`, `turn diff` and `turn blame` (seam 1): each test builds a
//! worktree's turns by firing the real hooks as claude does (`support/turns.rs`), then runs the
//! commands and compares them with what real git says of the snapshots.

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
#[path = "support/turns.rs"]
#[expect(
    dead_code,
    reason = "the turn driver is shared between suites, and this one leaves part of it unused"
)]
mod turns;

use std::fs;
use std::io::Write as _;
use std::process::Output;

use fixture::AgentRepo;
use serde_json::{Value, json};
use turns::{SESSION, Snapped, stop, submit};

/// A turn log line pair written before #38: no agent, no snapshots, no `end_observed`.
const OLD_TURN: &str = concat!(
    r#"{"schema":1,"kind":"start","turn":1,"session":"s-0","prompt_id":"p0","at":10,"prompt":"an old prompt"}"#,
    "\n",
    r#"{"schema":1,"kind":"end","turn":1,"session":"s-0","prompt_id":"p0","at":20,"outcome":"finished","detail":"ok"}"#,
    "\n",
);

/// Runs the binary with `args` from the primary checkout.
fn run(fx: &AgentRepo, args: &[&str]) -> Output {
    fx.run(args)
}

/// Runs the binary with `args`, which must fail, and returns its stderr.
fn fails(fx: &AgentRepo, args: &[&str]) -> String {
    let out = run(fx, args);
    assert!(!out.status.success(), "{args:?} succeeded");
    String::from_utf8(out.stderr).unwrap()
}

/// Runs the binary with `args` and `--json`, which must succeed, and parses its document.
fn json_of(fx: &AgentRepo, args: &[&str]) -> Value {
    let mut args = args.to_vec();
    args.push("--json");
    serde_json::from_str(&fx.ok(&args)).unwrap()
}

/// A worktree whose turn log starts with a turn from before #38, then: turn 2 finished, turn 3
/// interrupted by the prompt of turn 4, which is open and waits for the person.
fn four_turns(fx: &AgentRepo) -> Snapped<'_> {
    let wt = Snapped::new(fx, "fix");
    fs::write(wt.admin.join("turns.jsonl"), OLD_TURN).unwrap();
    wt.fire(&submit("p1"));
    wt.append("file.txt", "turn two");
    wt.fire(&stop("p1"));
    wt.fire(&submit("p2"));
    wt.append("file.txt", "turn three");
    wt.fire(&submit("p3"));
    wt.fire(&json!({
        "hook_event_name": "PermissionRequest",
        "session_id": SESSION,
        "prompt_id": "p3",
        "tool_name": "Bash",
        "tool_input": {"command": "ls"},
    }));
    wt
}

/// The first seven characters of `id`.
fn short(id: &str) -> &str {
    &id[..7]
}

#[test]
fn ls_lists_every_turn_oldest_first_the_open_one_last() {
    let fx = AgentRepo::new();
    let wt = four_turns(&fx);
    let (b2, e2, e3) = (wt.snap(2, "begin"), wt.snap(2, "end"), wt.snap(3, "end"));
    let (b3, b4) = (wt.snap(3, "begin"), wt.snap(4, "begin"));
    assert_eq!(b4, e3, "one snapshot ends turn 3 and begins turn 4");

    let table = fx.ok(&["turn", "ls", "fix"]);
    let expected = [
        "TURN  OUTCOME      AGENT   BEGIN    END      PROMPT".to_owned(),
        "1     finished     -       -        -        an old prompt".to_owned(),
        format!(
            "2     finished     claude  {}  {}  do p1",
            short(&b2),
            short(&e2)
        ),
        format!(
            "3     interrupted  claude  {}  {}  do p2",
            short(&b3),
            short(&e3)
        ),
        format!("4     waiting      claude  {}  -        do p3", short(&b4)),
    ]
    .join("\n");
    assert_eq!(table, format!("{expected}\n"));
}

#[test]
fn ls_json_gives_each_turn_with_its_snapshots_and_nulls_for_what_is_unknown() {
    let fx = AgentRepo::new();
    let wt = four_turns(&fx);
    let doc = json_of(&fx, &["turn", "ls", "fix"]);
    assert_eq!(doc["contract"], 1);
    assert!(doc["version"].is_string());
    assert_eq!(doc["name"], "fix");
    assert_eq!(doc["path"], json!(wt.worktree));
    let turns = doc["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 4);
    assert_eq!(
        turns[0],
        json!({
            "number": 1, "agent": null, "session": "s-0", "prompt": "an old prompt",
            "started_at": 10, "ended_at": 20, "outcome": "finished", "phase": null,
            "end_observed": null, "begin": null, "end": null, "denials": 0,
        })
    );
    let second = &turns[1];
    assert_eq!(second["agent"], "claude");
    assert_eq!(second["session"], SESSION);
    assert_eq!(second["outcome"], "finished");
    assert_eq!(second["end_observed"], true);
    assert_eq!(second["begin"], json!(wt.snap(2, "begin")));
    assert_eq!(second["end"], json!(wt.snap(2, "end")));
    assert_eq!(turns[2]["outcome"], "interrupted");
    assert_eq!(turns[2]["end_observed"], false);
    let open = &turns[3];
    assert_eq!(
        (&open["outcome"], &open["phase"], &open["ended_at"]),
        (&Value::Null, &json!("waiting"), &Value::Null)
    );
    assert_eq!(
        (&open["end_observed"], &open["end"]),
        (&Value::Null, &Value::Null)
    );
    assert_eq!(open["begin"], json!(wt.snap(4, "begin")));
}

#[test]
fn ls_of_a_worktree_with_no_turns_says_so() {
    let fx = AgentRepo::new();
    Snapped::new(&fx, "fix");
    assert_eq!(fx.ok(&["turn", "ls", "fix"]), "no turns in worktree fix\n");
    assert_eq!(json_of(&fx, &["turn", "ls", "fix"])["turns"], json!([]));
}

#[test]
fn the_turn_commands_only_read() {
    let fx = AgentRepo::new();
    let wt = four_turns(&fx);
    wt.append("file.txt", "since");
    let before = (
        fs::read(wt.admin.join("status.json")).unwrap(),
        fs::read(wt.admin.join("turns.jsonl")).unwrap(),
        wt.git(&["for-each-ref"]),
        wt.porcelain(),
    );
    fx.ok(&["turn", "ls", "fix"]);
    fx.ok(&["turn", "diff", "fix", "2"]);
    fx.ok(&["turn", "blame", "fix", "file.txt"]);
    let after = (
        fs::read(wt.admin.join("status.json")).unwrap(),
        fs::read(wt.admin.join("turns.jsonl")).unwrap(),
        wt.git(&["for-each-ref"]),
        wt.porcelain(),
    );
    assert_eq!(before, after);
}

#[test]
fn diff_prints_git_diff_from_the_turns_begin_to_its_end() {
    let fx = AgentRepo::new();
    let wt = four_turns(&fx);
    let (begin, end) = (wt.snap(2, "begin"), wt.snap(2, "end"));
    let out = run(&fx, &["turn", "diff", "fix", "2"]);
    assert!(out.status.success());
    let patch = String::from_utf8(out.stdout).unwrap();
    assert_eq!(patch, format!("{}\n", wt.git(&["diff", &begin, &end])));
    assert!(patch.contains("+turn two"), "{patch}");
    assert!(out.stderr.is_empty(), "an observed end needs no warning");

    let doc = json_of(&fx, &["turn", "diff", "fix", "2"]);
    assert_eq!(doc["contract"], 1);
    assert_eq!(doc["name"], "fix");
    assert_eq!(doc["turn"], 2);
    assert_eq!(doc["begin"], json!(begin));
    assert_eq!(doc["end"], json!(end));
    assert_eq!(doc["end_observed"], true);
    assert_eq!(doc["patch"], json!(patch));
}

#[test]
fn diff_of_an_interrupted_turn_warns_its_end_was_not_observed() {
    let fx = AgentRepo::new();
    let wt = four_turns(&fx);
    let out = run(&fx, &["turn", "diff", "fix", "3"]);
    assert!(out.status.success());
    let stderr = String::from_utf8(out.stderr).unwrap();
    assert_eq!(
        stderr,
        "turn 3: its end was not observed (it was interrupted), so this diff may include edits \
         made after the interruption\n"
    );
    let patch = String::from_utf8(out.stdout).unwrap();
    assert!(patch.contains("+turn three"), "{patch}");
    let doc = json_of(&fx, &["turn", "diff", "fix", "3"]);
    assert_eq!(doc["end_observed"], false);
    assert_eq!(doc["end"], json!(wt.snap(3, "end")));
}

#[test]
fn diff_refuses_a_missing_turn_an_open_one_and_one_from_before_the_snapshots() {
    let fx = AgentRepo::new();
    four_turns(&fx);
    for (turn, message) in [
        ("9", "there is no turn 9"),
        ("4", "turn 4 has not ended yet"),
        ("1", "turn 1 ran before orca-term took snapshots"),
    ] {
        let stderr = fails(&fx, &["turn", "diff", "fix", turn]);
        assert!(stderr.contains(message), "{turn}: {stderr}");
        let out = run(&fx, &["turn", "diff", "fix", turn, "--json"]);
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["error"]["kind"], "other", "{turn}: {doc}");
    }
}

/// A worktree whose `file.txt` holds a line from before any turn, one by turn 1, one by the
/// person between turns, one by turn 2, and one changed since the latest snapshot.
fn blamed(fx: &AgentRepo) -> Snapped<'_> {
    fx.repo.write("file.txt", "start\n");
    fx.repo.commit_all("file");
    let wt = Snapped::new(fx, "fix");
    wt.fire(&submit("p1"));
    wt.append("file.txt", "turn one");
    wt.fire(&stop("p1"));
    wt.append("file.txt", "by hand");
    wt.fire(&submit("p2"));
    wt.append("file.txt", "turn two");
    wt.fire(&stop("p2"));
    wt.append("file.txt", "since");
    wt
}

#[test]
fn blame_gives_each_line_to_its_turn_between_turns_or_since_the_last_snapshot() {
    let fx = AgentRepo::new();
    let wt = blamed(&fx);
    assert_eq!(
        fx.ok(&["turn", "blame", "fix", "file.txt"]),
        "   -     1  start\n   1     2  turn one\n   -     3  by hand\n   2     4  turn two\n   \
         ·     5  since\n"
    );

    let doc = json_of(&fx, &["turn", "blame", "fix", "file.txt"]);
    assert_eq!(doc["contract"], 1);
    assert_eq!(doc["name"], "fix");
    assert_eq!(doc["path"], json!(wt.worktree));
    assert_eq!(doc["file"], "file.txt");
    let lines = doc["lines"].as_array().unwrap();
    assert_eq!(
        lines[0],
        json!({"line": 1, "source": "between_turns", "turn": null, "agent": null,
               "snapshot": wt.snap(1, "begin"), "text": "start"})
    );
    assert_eq!(
        lines[1],
        json!({"line": 2, "source": "turn", "turn": 1, "agent": "claude",
               "snapshot": wt.snap(1, "end"), "text": "turn one"})
    );
    assert_eq!(lines[2]["source"], "between_turns");
    assert_eq!(lines[2]["snapshot"], json!(wt.snap(2, "begin")));
    assert_eq!(
        (&lines[3]["turn"], &lines[3]["source"]),
        (&json!(2), &json!("turn"))
    );
    assert_eq!(
        lines[4],
        json!({"line": 5, "source": "since_snapshot", "turn": null, "agent": null,
               "snapshot": null, "text": "since"})
    );
}

#[test]
fn blame_takes_a_path_absolute_inside_the_worktree_and_refuses_one_outside() {
    let fx = AgentRepo::new();
    let wt = blamed(&fx);
    let inside = wt.worktree.join("file.txt");
    let relative = fx.ok(&["turn", "blame", "fix", "file.txt"]);
    assert_eq!(
        fx.ok(&["turn", "blame", "fix", inside.to_str().unwrap()]),
        relative
    );
    let outside = fx.repo.root.join("file.txt");
    let stderr = fails(&fx, &["turn", "blame", "fix", outside.to_str().unwrap()]);
    assert!(stderr.contains("is outside the worktree"), "{stderr}");
    let stderr = fails(&fx, &["turn", "blame", "fix", "../file.txt"]);
    assert!(stderr.contains("leaves the worktree"), "{stderr}");
    let stderr = fails(&fx, &["turn", "blame", "fix", "missing.txt"]);
    assert!(
        stderr.contains("missing.txt is not a file of worktree fix"),
        "{stderr}"
    );
}

#[test]
fn blame_of_a_file_new_since_the_last_snapshot_or_with_no_chain_is_all_since_the_snapshot() {
    let fx = AgentRepo::new();
    let wt = blamed(&fx);
    let mut file = fs::File::create(wt.worktree.join("new.txt")).unwrap();
    file.write_all(b"a\nb\n").unwrap();
    assert_eq!(
        fx.ok(&["turn", "blame", "fix", "new.txt"]),
        "   ·     1  a\n   ·     2  b\n"
    );

    let fresh = Snapped::new(&fx, "fresh");
    fresh.write("plain.txt", "only\n");
    let doc = json_of(&fx, &["turn", "blame", "fresh", "plain.txt"]);
    assert_eq!(
        doc["lines"],
        json!([{"line": 1, "source": "since_snapshot", "turn": null, "agent": null,
                "snapshot": null, "text": "only"}])
    );
}
