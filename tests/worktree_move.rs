//! Worktree state survives worktree moves (story 16, #119), driven as a black box (seam 1): a
//! worktree with a turn, a note and a recorded agent is moved with `git worktree move`, and
//! `worktree ls`, `turn ls`, `note ls` and `agent start` still find it all. orca-term keeps that
//! state in the worktree's admin directory under git's common directory, which git keeps across a
//! move, and names the worktree by its directory's basename, so a move that keeps the basename
//! keeps the name.

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

use fixture::{AgentRepo, holder_alive};
use serde_json::{Value, json};
use turns::{Snapped, stop, submit};

/// Runs the binary with `args` and `--json`, which must succeed, and parses its document.
fn json_of(fx: &AgentRepo, args: &[&str]) -> Value {
    let mut args = args.to_vec();
    args.push("--json");
    serde_json::from_str(&fx.ok(&args)).unwrap()
}

#[test]
fn a_moved_worktree_keeps_its_turns_notes_and_agent() {
    let fx = AgentRepo::new();
    fx.repo.write("src/app.rs", "one\ntwo\n");
    fx.repo.commit_all("app");
    let wt = Snapped::new(&fx, "fix");
    fx.ok(&["agent", "start", "fix"]);
    fx.ok(&["agent", "stop", "fix"]);
    // claude's transcript lives outside the worktree, so a move leaves it where it is.
    let transcript = fx.repo.tmp.path().join("transcripts/s-1.jsonl");
    std::fs::create_dir_all(transcript.parent().unwrap()).unwrap();
    std::fs::write(&transcript, "").unwrap();
    let mut prompt = submit("p1");
    prompt["transcript_path"] = json!(transcript);
    wt.fire(&prompt);
    wt.append("src/app.rs", "three");
    wt.fire(&stop("p1"));
    fx.ok(&[
        "note",
        "add",
        "fix",
        "src/app.rs",
        "--line",
        "1",
        "--body",
        "why?",
    ]);

    let moved = fx.repo.tmp.path().join("elsewhere/fix");
    std::fs::create_dir_all(moved.parent().unwrap()).unwrap();
    fx.repo.git(&[
        "worktree",
        "move",
        wt.worktree.to_str().unwrap(),
        moved.to_str().unwrap(),
    ]);
    let moved = moved.canonicalize().unwrap();
    assert!(!wt.worktree.exists());

    let listed = fx.listed("fix");
    assert_eq!(listed["path"], moved.to_str().unwrap(), "{listed}");
    assert_eq!(listed["agent"], "claude-code", "{listed}");
    assert_eq!(listed["notes"], 1, "{listed}");

    let turns = json_of(&fx, &["turn", "ls", "fix"]);
    let turns = turns["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 1, "{turns:?}");
    assert_eq!(turns[0]["prompt"], "do p1");
    assert_eq!(turns[0]["outcome"], "finished");

    let notes = json_of(&fx, &["note", "ls", "fix"]);
    let notes = notes["notes"].as_array().unwrap();
    assert_eq!(notes.len(), 1, "{notes:?}");
    assert_eq!(notes[0]["body"], "why?");

    let started = json_of(&fx, &["agent", "start", "fix"]);
    assert_eq!(started["agent"], "claude-code");
    assert_eq!(started["holder"]["status"], "started");
    assert_eq!(started["resumed"], "s-1", "{started}");
    assert_eq!(started["worktree"]["path"], moved.to_str().unwrap());
    assert!(holder_alive(&fx.admin_dir(&moved)));
}
