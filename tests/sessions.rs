//! A worktree's sessions (#41), `session ls`, `session resume` and `session new` (seam 1): each
//! test builds a worktree's sessions by firing the real hooks as claude does (`support/turns.rs`)
//! and writing opencode's plugin lines through the fake opencode (`support/opencode.rs`), then
//! runs the commands against the fake agents, on an isolated PATH, and reads what the fakes were
//! started with.

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
#[path = "support/turns.rs"]
#[expect(
    dead_code,
    reason = "the turn driver is shared between suites, and this one leaves part of it unused"
)]
mod turns;

use std::fs;
use std::path::PathBuf;
use std::process::Output;

use fixture::{AgentRepo, holder_alive, state_file, wait_until};
use opencode::OpenCodeRepo;
use serde_json::{Value, json};
use turns::Snapped;

/// claude's ids for the sessions of these tests, as real ones read.
const FIRST: &str = "0b983a87-4074-466e-b864-422ce6f063c8";
const SECOND: &str = "5d1c2e9f-8a7b-4c3d-9e2f-1a0b9c8d7e6f";
const THIRD: &str = "9f8e7d6c-5b4a-4321-8fed-cba987654321";

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("not JSON: {}", stdout(out)))
}

/// Where the fixture's claude keeps the transcript of `session`.
fn transcript(fx: &AgentRepo, session: &str) -> PathBuf {
    fx.repo
        .tmp
        .path()
        .join("transcripts")
        .join(format!("{session}.jsonl"))
}

/// Writes the transcript of `session`, as claude does once the session has a turn.
fn write_transcript(fx: &AgentRepo, session: &str) {
    let path = transcript(fx, session);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, "{}\n").unwrap();
}

/// claude's `SessionStart` of `session`, a new process.
fn session_start(fx: &AgentRepo, session: &str) -> Value {
    json!({
        "hook_event_name": "SessionStart",
        "session_id": session,
        "transcript_path": transcript(fx, session),
        "cwd": "/w",
        "source": "startup",
    })
}

/// claude's `UserPromptSubmit` of `prompt` in `session`, as the prompt `prompt_id`.
fn submit(fx: &AgentRepo, session: &str, prompt_id: &str, prompt: &str) -> Value {
    json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": session,
        "transcript_path": transcript(fx, session),
        "prompt_id": prompt_id,
        "prompt": prompt,
    })
}

/// claude's `Stop` of the turn of `prompt_id` in `session`.
fn stop(fx: &AgentRepo, session: &str, prompt_id: &str) -> Value {
    json!({
        "hook_event_name": "Stop",
        "session_id": session,
        "transcript_path": transcript(fx, session),
        "prompt_id": prompt_id,
        "last_assistant_message": "Done.",
    })
}

/// Fires one whole turn of `prompt` in `session`.
fn turn(wt: &Snapped<'_>, session: &str, prompt_id: &str, prompt: &str) {
    wt.fire(&submit(wt.fx, session, prompt_id, prompt));
    wt.fire(&stop(wt.fx, session, prompt_id));
}

/// The sessions `session ls --json` lists for `demo`, by id, agent, turn count and whether
/// current.
fn listed(fx: &AgentRepo) -> Vec<(String, String, u64, bool)> {
    let doc = json(&fx.run(&["session", "ls", "demo", "--json"]));
    doc["sessions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|session| {
            (
                session["id"].as_str().unwrap().to_owned(),
                session["agent"].as_str().unwrap().to_owned(),
                session["turns"].as_u64().unwrap(),
                session["current"].as_bool().unwrap(),
            )
        })
        .collect()
}

fn entry(id: &str, agent: &str, turns: u64, current: bool) -> (String, String, u64, bool) {
    (id.to_owned(), agent.to_owned(), turns, current)
}

/// The worktree `demo` with claude's sessions: `FIRST`, two turns; then `SECOND`, one turn, the
/// current one.
fn two_claude_sessions(fx: &AgentRepo) -> Snapped<'_> {
    let wt = Snapped::new(fx, "demo");
    wt.fire(&session_start(fx, FIRST));
    turn(&wt, FIRST, "p1", "fix the login");
    turn(&wt, FIRST, "p2", "now the logout");
    wt.fire(&session_start(fx, SECOND));
    turn(&wt, SECOND, "p3", "write the docs");
    wt
}

#[test]
fn session_ls_lists_every_session_the_most_recent_first_the_current_marked() {
    let fx = AgentRepo::new();
    let wt = two_claude_sessions(&fx);
    wt.fire(&session_start(&fx, THIRD));

    let doc = json(&fx.run(&["session", "ls", "demo", "--json"]));
    assert_eq!(doc["contract"], 1);
    assert_eq!(doc["worktree"]["name"], "demo");
    let sessions = doc["sessions"].as_array().unwrap();
    assert_eq!(
        listed(&fx),
        [
            entry(THIRD, "claude-code", 0, true),
            entry(SECOND, "claude-code", 1, false),
            entry(FIRST, "claude-code", 2, false),
        ],
        "the current one, with no turn yet, is the most recently active"
    );
    assert_eq!(sessions[0]["first_prompt"], Value::Null);
    assert_eq!(sessions[2]["first_prompt"], "fix the login");
    let activity: Vec<u64> = sessions
        .iter()
        .map(|session| session["last_activity"].as_u64().unwrap())
        .collect();
    assert!(
        activity[0] >= activity[1] && activity[1] > activity[2],
        "{activity:?}"
    );
    let keys: Vec<&str> = sessions[1]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "agent",
            "current",
            "first_prompt",
            "id",
            "last_activity",
            "turns"
        ],
        "every key, sorted"
    );
}

/// Now, in Unix milliseconds.
fn now_ms() -> u64 {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    u64::try_from(since.as_millis()).unwrap()
}

/// `last_activity` of `demo` in `worktree ls --json` and `worktree detail --json`, which must
/// agree.
fn worktree_activity(fx: &AgentRepo) -> Value {
    let listed = fx.listed("demo")["last_activity"].clone();
    let detail = json(&fx.run(&["worktree", "detail", "demo", "--json"]));
    assert_eq!(detail["worktree"]["last_activity"], listed);
    listed
}

#[test]
fn a_worktrees_last_activity_is_its_current_sessions_in_both_documents() {
    let fx = AgentRepo::new();
    let before = now_ms();
    let wt = Snapped::new(&fx, "demo");
    assert_eq!(worktree_activity(&fx), Value::Null, "no session yet");

    wt.fire(&session_start(&fx, FIRST));
    turn(&wt, FIRST, "p1", "fix the login");
    let after = now_ms();
    let activity = worktree_activity(&fx).as_u64().unwrap();
    assert!(before <= activity && activity <= after, "{activity}");
    let doc = json(&fx.run(&["session", "ls", "demo", "--json"]));
    let session = doc["sessions"][0]["last_activity"].as_u64().unwrap();
    assert!(
        activity >= session,
        "the latest of the session's turns and the hooks' last report"
    );

    fx.ok(&["session", "new", "demo"]);
    assert_eq!(
        worktree_activity(&fx),
        Value::Null,
        "no current session once it is discarded"
    );
}

#[test]
fn session_ls_prints_a_table_like_turn_ls_or_says_there_is_none() {
    let fx = AgentRepo::new();
    let wt = two_claude_sessions(&fx);
    wt.fire(&session_start(&fx, THIRD));

    let text = fx.ok(&["session", "ls", "demo"]);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(
        lines[0],
        format!(
            "   SESSION{}AGENT        TURNS  LAST ACTIVITY  FIRST PROMPT",
            " ".repeat(31)
        )
    );
    assert_eq!(
        lines[1],
        format!("*  {THIRD}  claude-code  0      just now       -")
    );
    assert_eq!(
        lines[3],
        format!("   {FIRST}  claude-code  2      just now       fix the login")
    );

    let empty = AgentRepo::new();
    empty.new_worktree("demo");
    assert_eq!(
        empty.ok(&["session", "ls", "demo"]),
        "no sessions in worktree demo\n"
    );
}

#[test]
fn resume_relaunches_claude_with_the_session_then_only_reports_it_while_current() {
    let fx = AgentRepo::new();
    let wt = two_claude_sessions(&fx);
    write_transcript(&fx, FIRST);
    write_transcript(&fx, SECOND);
    fx.ok(&["agent", "start", "demo"]);
    fx.wait_for_output("demo", "fake claude ready");
    assert_eq!(fx.argv()[4..], ["--resume", SECOND]);

    let doc = json(&fx.run(&["session", "resume", "demo", FIRST, "--json"]));
    assert_eq!(doc["session"], FIRST);
    assert_eq!(doc["resumed"], FIRST);
    assert_eq!(doc["agent"], "claude-code");
    assert_eq!(doc["previous"], "claude-code");
    assert_eq!(doc["stopped"], true);
    assert_eq!(doc["holder"]["status"], "started");
    fx.wait_for_output("demo", "fake claude ready");
    assert_eq!(fx.argv()[4..], ["--resume", FIRST], "the holder relaunched");
    assert_eq!(fx.starts(), 2);
    assert_eq!(wt.status()["session"], FIRST);
    assert_eq!(
        listed(&fx),
        [
            entry(SECOND, "claude-code", 1, false),
            entry(FIRST, "claude-code", 2, true),
        ]
    );

    let text = fx.ok(&["session", "resume", "demo", FIRST]);
    assert!(
        text.starts_with(&format!(
            "session {FIRST} is already current in worktree demo\nagent claude-code is already \
             running in worktree demo\n"
        )),
        "{text}"
    );
    assert_eq!(fx.starts(), 2, "nothing restarted");

    fx.ok(&["agent", "stop", "demo"]);
    let text = fx.ok(&["session", "resume", "demo", SECOND]);
    assert!(
        text.starts_with(&format!(
            "started agent claude-code in worktree demo, resuming session {SECOND}\n"
        )),
        "{text}"
    );
    fx.wait_for_output("demo", "fake claude ready");
    assert_eq!(fx.argv()[4..], ["--resume", SECOND], "the holder started");
}

#[test]
fn resume_of_the_other_agents_session_swaps_the_agent() {
    let oc = OpenCodeRepo::new();
    let fx = &oc.fx;
    let wt = Snapped::new(fx, "demo");
    wt.fire(&session_start(fx, FIRST));
    turn(&wt, FIRST, "p1", "fix the login");
    write_transcript(fx, FIRST);
    oc.ok(&["agent", "start", "demo", "--agent", "opencode"]);
    let mut driver = oc.driver("demo");
    driver
        .event(&json!({"type": "prompt", "session": "ses_1", "parent": null, "text": "add tests"}));
    driver.event(&json!({
        "type": "idle", "session": "ses_1", "parent": null, "text": "Added.",
        "finish": "stop", "error": null
    }));
    wait_until("opencode's turn to end", || {
        fx.listed("demo")["state"] == "finished"
    });
    driver.detach();
    assert_eq!(
        listed(fx),
        [
            entry("ses_1", "opencode", 1, true),
            entry(FIRST, "claude-code", 1, false),
        ]
    );

    let doc = json(&oc.run(&["session", "resume", "demo", FIRST, "--json"]));
    assert_eq!(doc["agent"], "claude-code");
    assert_eq!(doc["previous"], "opencode");
    assert_eq!(doc["stopped"], true);
    assert_eq!(doc["resumed"], FIRST);
    fx.wait_for_output("demo", "fake claude ready");
    assert_eq!(fx.argv()[4..], ["--resume", FIRST]);
    assert_eq!(state_file(&wt.admin)["agent"], "claude-code");

    oc.list_sessions(&json!([{"id": "ses_0"}, {"id": "ses_1", "title": "add tests"}]));
    let text = oc.ok(&["session", "resume", "demo", "ses_1"]);
    assert!(
        text.starts_with(
            "stopped agent claude-code in worktree demo\nstarted agent opencode in worktree \
             demo, resuming session ses_1\n"
        ),
        "{text}"
    );
    oc.driver("demo").detach();
    assert_eq!(oc.argv(), ["-s", "ses_1"]);
    assert_eq!(
        oc.session_list_argv().unwrap(),
        ["session", "list", "--format", "json", "--pure"],
        "opencode was asked for its sessions first"
    );
    assert_eq!(state_file(&wt.admin)["agent"], "opencode");
    assert_eq!(wt.turns().len(), 4, "every turn stays");
}

#[test]
fn changing_the_session_is_busy_while_the_agent_is_in_a_turn() {
    let fx = AgentRepo::new();
    let wt = two_claude_sessions(&fx);
    write_transcript(&fx, FIRST);
    write_transcript(&fx, SECOND);
    fx.ok(&["agent", "start", "demo"]);
    fx.wait_for_output("demo", "fake claude ready");
    wt.fire(&submit(&fx, SECOND, "p4", "a long job"));
    assert_eq!(fx.listed("demo")["state"], "working");

    for args in [
        &["session", "resume", "demo", FIRST, "--json"][..],
        &["session", "new", "demo", "--json"],
    ] {
        let out = fx.run(args);
        assert_eq!(out.status.code(), Some(3), "{args:?}: {}", stdout(&out));
        let error = &json(&out)["error"];
        assert_eq!(error["kind"], "busy");
        assert!(
            error["message"]
                .as_str()
                .unwrap()
                .contains("its agent is in a turn, working or waiting for you; change its session"),
            "{error}"
        );
    }
    assert!(holder_alive(&wt.admin), "the agent is left running");
    assert_eq!(fx.starts(), 1);
    assert_eq!(wt.status()["session"], SECOND);
}

#[test]
fn a_session_whose_transcript_is_gone_cannot_be_resumed_and_nothing_launches() {
    let fx = AgentRepo::new();
    let wt = two_claude_sessions(&fx);

    let out = fx.run(&["session", "resume", "demo", FIRST, "--json"]);
    assert_eq!(out.status.code(), Some(4), "{}", stdout(&out));
    let error = &json(&out)["error"];
    assert_eq!(error["kind"], "session_gone");
    let gone = transcript(&fx, FIRST);
    assert_eq!(
        error["message"],
        format!(
            "worktree demo cannot resume its session {FIRST}: its transcript {} is gone; start a \
             fresh session with `orca-term agent start demo --fresh`",
            gone.display()
        )
    );
    assert_eq!(fx.starts(), 0, "nothing was launched");
    assert_eq!(wt.status()["session"], SECOND, "the current session stays");

    let out = fx.run(&["session", "resume", "demo", "nope"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "orca-term: worktree demo has no session nope; `orca-term session ls demo` lists its \
         sessions\n"
    );
}

#[test]
fn an_opencode_session_its_own_list_no_longer_holds_is_gone() {
    let oc = OpenCodeRepo::new();
    let wt = Snapped::new(&oc.fx, "demo");
    oc.ok(&["agent", "start", "demo", "--agent", "opencode"]);
    let mut driver = oc.driver("demo");
    driver.event(&json!({"type": "prompt", "session": "ses_1", "parent": null, "text": "go"}));
    driver.event(&json!({
        "type": "idle", "session": "ses_1", "parent": null, "text": "Gone.",
        "finish": "stop", "error": null
    }));
    driver.detach();
    oc.ok(&["agent", "stop", "demo"]);
    oc.list_sessions(&json!([{"id": "ses_other"}]));

    let out = oc.run(&["session", "resume", "demo", "ses_1", "--json"]);
    assert_eq!(out.status.code(), Some(4), "{}", stdout(&out));
    let error = &json(&out)["error"];
    assert_eq!(error["kind"], "session_gone");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains(": opencode no longer lists it in `opencode session list`; "),
        "{error}"
    );
    assert_eq!(oc.starts(), 1, "only the first start");
    assert!(!holder_alive(&wt.admin));
}

#[test]
fn session_new_keeps_every_session_and_its_turns_and_resume_brings_the_old_one_back() {
    let fx = AgentRepo::new();
    let wt = two_claude_sessions(&fx);
    write_transcript(&fx, SECOND);
    let lines = wt.turns().len();

    let doc = json(&fx.run(&["session", "new", "demo", "--json"]));
    assert_eq!(doc["discarded"], SECOND);
    assert_eq!(doc["started"], Value::Null);
    assert_eq!(doc["worktree"]["name"], "demo");
    assert_eq!(wt.status()["session"], Value::Null);
    assert_eq!(wt.turns().len(), lines, "the turn log is untouched");
    assert_eq!(
        listed(&fx),
        [
            entry(SECOND, "claude-code", 1, false),
            entry(FIRST, "claude-code", 2, false),
        ]
    );
    let text = fx.ok(&["session", "new", "demo"]);
    assert_eq!(
        text,
        "worktree demo had no current session\nthe next start of its agent begins a new session\n"
    );

    let doc = json(&fx.run(&["session", "resume", "demo", SECOND, "--json"]));
    assert_eq!(
        doc["resumed"], SECOND,
        "the holder started on the old session"
    );
    assert_eq!(doc["stopped"], false);
    fx.wait_for_output("demo", "fake claude ready");
    assert_eq!(fx.argv()[4..], ["--resume", SECOND]);
    assert_eq!(
        listed(&fx),
        [
            entry(SECOND, "claude-code", 1, true),
            entry(FIRST, "claude-code", 2, false),
        ]
    );
}

#[test]
fn session_new_restarts_a_running_agent_on_a_new_session() {
    let fx = AgentRepo::new();
    let wt = two_claude_sessions(&fx);
    write_transcript(&fx, SECOND);
    fx.ok(&["agent", "start", "demo"]);
    fx.wait_for_output("demo", "fake claude ready");
    assert_eq!(fx.argv()[4..], ["--resume", SECOND]);

    let text = fx.ok(&["session", "new", "demo"]);
    assert!(
        text.starts_with(&format!(
            "discarded session {SECOND} of worktree demo; `orca-term session resume demo \
             {SECOND}` brings it back\nstarted agent claude-code in worktree demo\n"
        )),
        "{text}"
    );
    fx.wait_for_output("demo", "fake claude ready");
    assert_eq!(fx.starts(), 2, "restarted");
    assert!(
        !fx.argv().contains(&"--resume".to_owned()),
        "{:?}",
        fx.argv()
    );
    wt.fire(&session_start(&fx, THIRD));
    turn(&wt, THIRD, "p5", "a new start");
    assert_eq!(
        listed(&fx),
        [
            entry(THIRD, "claude-code", 1, true),
            entry(SECOND, "claude-code", 1, false),
            entry(FIRST, "claude-code", 2, false),
        ]
    );
}
