//! `orca-term note add`, `ls`, `edit`, `rm` and `send`, driven as a black box (seam 1): notes kept
//! in the worktree's admin directory, their lines translated to the file on disk now, and sending
//! them to the worktree's agent through its holder, against the fake `claude` of `tests/agent.rs`.

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
use std::path::{Path, PathBuf};
use std::process::Output;

use fixture::{AgentRepo, wait_until};
use serde_json::{Value, json};

/// The file every test attaches notes to, as committed: four lines.
const APP: &str = "one\ntwo\nthree\nfour\n";

/// A repository whose worktree `fix` holds `src/app.rs` ([`APP`]); returns the worktree's path.
fn fixture() -> (AgentRepo, PathBuf) {
    let fx = AgentRepo::new();
    fx.repo.write("src/app.rs", APP);
    fx.repo.commit_all("app");
    let worktree = fx.new_worktree("fix");
    (fx, worktree)
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// Runs the binary with `args` and `--json`, failing the test unless it succeeded.
fn ok_json(fx: &AgentRepo, args: &[&str]) -> Value {
    let mut args = args.to_vec();
    args.push("--json");
    serde_json::from_str(&fx.ok(&args)).unwrap()
}

/// Adds a note to the worktree `fix` with `args` (its path, anchor and body).
fn add(fx: &AgentRepo, args: &[&str]) -> String {
    fx.ok(&[&["note", "add", "fix"], args].concat())
}

/// `note ls --json`'s notes of the worktree `fix`.
fn listed(fx: &AgentRepo) -> Vec<Value> {
    ok_json(fx, &["note", "ls", "fix"])["notes"]
        .as_array()
        .unwrap()
        .clone()
}

/// The ids of the pending notes of `fix`.
fn ids(fx: &AgentRepo) -> Vec<u64> {
    listed(fx)
        .iter()
        .map(|note| note["id"].as_u64().unwrap())
        .collect()
}

/// The contents kept for notes on lines, by name.
fn copies(fx: &AgentRepo, worktree: &Path) -> Vec<String> {
    let dir = fx.admin_dir(worktree).join("notes");
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect()
}

/// Asserts the command failed with exit code 1 and a message containing `message`.
fn refused(out: &Output, message: &str) {
    assert_eq!(out.status.code(), Some(1), "{}", stderr(out));
    assert!(stderr(out).contains(message), "{}", stderr(out));
}

/// Makes the fake claude, once started, take its terminal raw and record every byte typed into it,
/// untouched by the terminal's line discipline, until it is stopped.
fn record_typing(fx: &AgentRepo) {
    fx.write_claude(
        "2.1.296 (Claude Code)",
        "/bin/stty raw -echo\necho \"fake claude recording\"\nexec /bin/cat > \"$log.stdin\"",
    );
}

/// Every byte typed into the recording fake claude so far.
fn typed(fx: &AgentRepo) -> Vec<u8> {
    fs::read(fx.log.with_extension("stdin")).unwrap_or_default()
}

/// Starts the worktree's agent and waits until it says `ready`.
fn start_agent(fx: &AgentRepo, ready: &str) {
    fx.ok(&["agent", "start", "fix"]);
    fx.wait_for_output("fix", ready);
}

#[test]
fn add_keeps_a_note_on_a_line_a_range_or_a_whole_file() {
    let (fx, worktree) = fixture();
    let line = ok_json(
        &fx,
        &[
            "note",
            "add",
            "fix",
            "./src/app.rs",
            "--line",
            "2",
            "--body",
            " why? ",
        ],
    );
    assert_eq!(line["name"], "fix");
    assert_eq!(line["path"], worktree.to_str().unwrap());
    let created = line["note"]["created"].as_u64().unwrap();
    assert!(created > 0);
    assert_eq!(
        line["note"],
        json!({"id": 1, "path": "src/app.rs", "anchor": {"scope": "line", "line": 2},
               "current": {"scope": "line", "line": 2}, "body": "why?", "rev": 1,
               "created": created})
    );
    let range = ok_json(
        &fx,
        &[
            "note",
            "add",
            "fix",
            "src/app.rs",
            "--lines",
            "3-4",
            "--body",
            "range",
        ],
    );
    assert_eq!(
        range["note"]["anchor"],
        json!({"scope": "lines", "start": 3, "end": 4})
    );
    // A whole-file note may name a file that is not on disk: a deleted one.
    let text = fx.ok(&["note", "add", "fix", "gone.rs", "--body", "deleted?"]);
    assert_eq!(text, "fix: note 3 on gone.rs, the whole file\n");
    assert_eq!(ids(&fx), vec![1, 2, 3]);
    // Both notes on lines share the one copy of the file's content.
    assert_eq!(copies(&fx, &worktree).len(), 1);
    assert_eq!(fx.listed("fix")["notes"], 3);
}

#[test]
fn add_refuses_what_a_note_cannot_be() {
    let (fx, worktree) = fixture();
    let add = |args: &[&str]| fx.run(&[&["note", "add"], args].concat());
    refused(&add(&["nope", "src/app.rs", "--body", "x"]), "nope");
    refused(
        &add(&[".", "src/app.rs", "--body", "x"]),
        "primary checkout",
    );
    refused(
        &add(&["fix", "../outside", "--body", "x"]),
        "leaves the worktree",
    );
    refused(&add(&["fix", "/etc/hosts", "--body", "x"]), "is absolute");
    refused(
        &add(&["fix", "src/app.rs", "--line", "5", "--body", "x"]),
        "has 4 lines",
    );
    refused(
        &add(&["fix", "src/app.rs", "--lines", "3-5", "--body", "x"]),
        "lines 3-5 is outside it",
    );
    refused(
        &add(&["fix", "gone.rs", "--line", "1", "--body", "x"]),
        "only a whole-file note",
    );
    refused(&add(&["fix", "src", "--body", "x"]), "is a directory");
    refused(
        &add(&["fix", "src/app.rs", "--body", " \n "]),
        "needs a body",
    );
    std::os::unix::fs::symlink(fx.repo.tmp.path(), worktree.join("out")).unwrap();
    refused(&add(&["fix", "out/x", "--body", "x"]), "through a symlink");
    // Usage errors: clap refuses them before anything runs.
    for args in [
        &["fix", "src/app.rs", "--line", "0", "--body", "x"][..],
        &["fix", "src/app.rs", "--lines", "4-2", "--body", "x"],
        &[
            "fix",
            "src/app.rs",
            "--line",
            "1",
            "--lines",
            "1-2",
            "--body",
            "x",
        ],
        &["fix", "src/app.rs"],
    ] {
        assert_eq!(add(args).status.code(), Some(2), "{args:?}");
    }
    let out = fx.run(&["note", "add", "nope", "a", "--body", "x", "--json"]);
    assert_eq!(json(&out)["error"]["kind"], "other");
    assert!(listed(&fx).is_empty());
    assert!(copies(&fx, &worktree).is_empty());
}

#[test]
fn ls_translates_lines_to_the_file_on_disk_now() {
    let (fx, worktree) = fixture();
    add(&fx, &["src/app.rs", "--line", "2", "--body", "a"]);
    add(&fx, &["src/app.rs", "--line", "3", "--body", "b"]);
    add(&fx, &["src/app.rs", "--lines", "1-4", "--body", "c"]);
    let file = worktree.join("src/app.rs");

    // Two lines inserted above: everything moves down by two.
    fs::write(&file, format!("zero\nhalf\n{APP}")).unwrap();
    let current: Vec<Value> = listed(&fx).iter().map(|n| n["current"].clone()).collect();
    assert_eq!(
        current,
        [
            json!({"scope": "line", "line": 4}),
            json!({"scope": "line", "line": 5}),
            json!({"scope": "lines", "start": 3, "end": 6}),
        ]
    );
    // The anchor as made never changes.
    assert_eq!(
        listed(&fx)[0]["anchor"],
        json!({"scope": "line", "line": 2})
    );

    // Line `three` deleted: note 2's line is gone, so it is on the whole file now.
    fs::write(&file, "zero\nhalf\none\ntwo\nfour\n").unwrap();
    let notes = listed(&fx);
    assert_eq!(notes[0]["current"], json!({"scope": "line", "line": 4}));
    assert_eq!(notes[1]["current"], json!({"scope": "file"}));
    assert_eq!(
        notes[2]["current"],
        json!({"scope": "lines", "start": 3, "end": 5})
    );
    let text = fx.ok(&["note", "ls", "fix"]);
    assert!(text.contains("line 3 (now the whole file)"), "{text}");

    // The file deleted: every note is on the whole file.
    fs::remove_file(&file).unwrap();
    assert!(
        listed(&fx)
            .iter()
            .all(|n| n["current"] == json!({"scope": "file"}))
    );
}

#[test]
fn edit_bumps_the_revision_and_rm_removes_the_note_and_its_unused_copy() {
    let (fx, worktree) = fixture();
    fx.ok(&[
        "note",
        "add",
        "fix",
        "src/app.rs",
        "--line",
        "1",
        "--body",
        "first",
    ]);
    fx.ok(&["note", "add", "fix", "README", "--body", "file"]);
    let edited = ok_json(&fx, &["note", "edit", "fix", "1", "--body", "second"]);
    assert_eq!(edited["note"]["body"], "second");
    assert_eq!(edited["note"]["rev"], 2);
    refused(
        &fx.run(&["note", "edit", "fix", "9", "--body", "x"]),
        "no pending note 9",
    );
    refused(
        &fx.run(&["note", "edit", "fix", "1", "--body", ""]),
        "needs a body",
    );

    assert_eq!(copies(&fx, &worktree).len(), 1);
    let removed = ok_json(&fx, &["note", "rm", "fix", "1"]);
    assert_eq!(removed["id"], 1);
    assert!(
        copies(&fx, &worktree).is_empty(),
        "no note refers to it any more"
    );
    refused(&fx.run(&["note", "rm", "fix", "1"]), "no pending note 1");
    assert_eq!(ids(&fx), vec![2]);
    // Ids are never reused.
    let next = ok_json(&fx, &["note", "add", "fix", "README", "--body", "again"]);
    assert_eq!(next["note"]["id"], 3);
}

#[test]
fn send_pastes_the_notes_in_orcas_format_then_enter_and_consumes_them() {
    let (fx, worktree) = fixture();
    fx.ok(&[
        "note",
        "add",
        "fix",
        "src/app.rs",
        "--line",
        "2",
        "--body",
        "why \"two\"?\nsay",
    ]);
    fx.ok(&[
        "note",
        "add",
        "fix",
        "src/app.rs",
        "--lines",
        "3-4",
        "--body",
        "a\\b",
    ]);
    fx.ok(&["note", "add", "fix", "gone.rs", "--body", "unsent"]);
    // A line inserted above before sending: the lines sent are the file's now.
    fs::write(worktree.join("src/app.rs"), format!("zero\n{APP}")).unwrap();
    record_typing(&fx);
    start_agent(&fx, "fake claude recording");

    let sent = ok_json(&fx, &["note", "send", "fix", "2", "1"]);

    let prompt = "File: src/app.rs\nLine: 3\nUser comment: \"why \\\"two\\\"?\\nsay\"\n\n\
                  File: src/app.rs\nLines: 4-5\nUser comment: \"a\\\\b\"";
    assert_eq!(sent["prompt"], prompt);
    assert_eq!(sent["sent"], json!([1, 2]));
    assert_eq!(sent["kept"], json!([]));
    let expected = [b"\x1b[200~", prompt.as_bytes(), b"\x1b[201~", b"\r"].concat();
    wait_until("the prompt reaching the agent", || typed(&fx) == expected);
    assert_eq!(
        ids(&fx),
        vec![3],
        "the sent notes are consumed, the other kept"
    );
    assert!(copies(&fx, &worktree).is_empty());
    assert_eq!(fx.listed("fix")["notes"], 1);
    assert_eq!(fx.listed("fix")["holder"], true, "the agent still runs");

    // The rest, by default.
    let sent = ok_json(&fx, &["note", "send", "fix"]);
    assert_eq!(sent["sent"], json!([3]));
    assert!(listed(&fx).is_empty());
    let out = fx.run(&["note", "send", "fix"]);
    refused(&out, "no pending notes");
}

#[test]
fn send_disturbs_no_terminal_and_prints_the_prompt() {
    let (fx, _) = fixture();
    fx.ok(&[
        "note",
        "add",
        "fix",
        "src/app.rs",
        "--line",
        "1",
        "--body",
        "hello",
    ]);
    start_agent(&fx, "fake claude ready");
    let text = fx.ok(&["note", "send", "fix"]);
    assert_eq!(
        text,
        "File: src/app.rs\nLine: 1\nUser comment: \"hello\"\n\nfix: sent notes 1\n"
    );
    // The fake claude echoes each line it reads; it never saw a size change.
    let seen = fx.wait_for_output("fix", "you said: User comment: \"hello\"");
    assert!(!seen.contains("size="), "{seen}");
}

#[test]
fn send_with_no_running_agent_refuses_and_consumes_nothing() {
    let (fx, worktree) = fixture();
    fx.ok(&[
        "note",
        "add",
        "fix",
        "src/app.rs",
        "--line",
        "1",
        "--body",
        "hello",
    ]);
    let out = fx.run(&["note", "send", "fix", "--json"]);
    assert_eq!(out.status.code(), Some(7));
    let doc = json(&out);
    assert_eq!(doc["error"]["kind"], "no_holder");
    let message = doc["error"]["message"].as_str().unwrap();
    assert!(message.contains("orca-term agent start fix"), "{message}");
    let out = fx.run(&["note", "send", "fix"]);
    assert_eq!(out.status.code(), Some(7));
    assert!(
        stderr(&out).contains("no running agent"),
        "{}",
        stderr(&out)
    );
    assert_eq!(ids(&fx), vec![1]);
    assert_eq!(copies(&fx, &worktree).len(), 1);
    // An id with no note is refused before anything else.
    refused(&fx.run(&["note", "send", "fix", "4"]), "no pending note 4");
}

#[test]
fn worktree_ls_counts_the_pending_notes() {
    let (fx, _) = fixture();
    fx.new_worktree("other");
    assert_eq!(fx.listed("fix")["notes"], 0);
    fx.ok(&["note", "add", "fix", "src/app.rs", "--body", "a"]);
    fx.ok(&[
        "note",
        "add",
        "fix",
        "src/app.rs",
        "--line",
        "1",
        "--body",
        "b",
    ]);
    assert_eq!(fx.listed("fix")["notes"], 2);
    assert_eq!(fx.listed("other")["notes"], 0);
    let table = fx.ok(&["worktree", "ls"]);
    let header = table.lines().next().unwrap();
    assert!(header.contains("NOTES"), "{table}");
    fx.ok(&["note", "rm", "fix", "1"]);
    assert_eq!(fx.listed("fix")["notes"], 1);
}
