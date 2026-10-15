//! claude's hooks, driven as claude drives them (seam 1): `agent start` writes the settings with
//! the hooks, and each test plays a recording of what claude 2.1.300 sent
//! (`tests/fixtures/claude-hooks/*.log`, one hook per line: the event, a space, the payload) by
//! running the handler the settings register for each event, exactly as written (command and
//! arguments, the payload on stdin, the worktree as the directory), then reads `worktree ls
//! --json`. The fake claude itself sends no hook. Also the guards: a hook exits 0 and prints
//! nothing whatever it is given, and never makes a command busy.
//!
//! The recordings had their absolute paths replaced by `{{worktree}}`, `{{transcripts}}` and
//! `{{denied}}`, which are written back here with paths of the test's own.

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

use std::fs::{self, File};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use fixture::AgentRepo;
use serde_json::{Value, json};

/// Where the recordings are.
const FIXTURES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/claude-hooks");

/// A worktree whose agent runs, with the settings it was started with.
struct Played<'a> {
    fx: &'a AgentRepo,
    name: String,
    worktree: PathBuf,
    admin: PathBuf,
    settings: Value,
}

impl<'a> Played<'a> {
    /// Creates the worktree `name` and starts its agent.
    fn start(fx: &'a AgentRepo, name: &str) -> Self {
        let worktree = fx.new_worktree(name);
        let admin = fx.admin_dir(&worktree);
        fx.ok(&["agent", "start", name]);
        let settings =
            serde_json::from_slice(&fs::read(admin.join("claude-settings.json")).unwrap()).unwrap();
        Self {
            fx,
            name: name.to_owned(),
            worktree,
            admin,
            settings,
        }
    }

    /// The hooks of the recording `fixture`, as (event, payload), its paths made the test's own.
    fn recording(&self, fixture: &str) -> Vec<(String, String)> {
        let text = fs::read_to_string(Path::new(FIXTURES).join(fixture)).unwrap();
        let tmp = self.fx.repo.tmp.path();
        text.lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let (event, payload) = line.split_once(' ').unwrap();
                let payload = payload
                    .replace("{{worktree}}", self.worktree.to_str().unwrap())
                    .replace("{{transcripts}}", tmp.join("transcripts").to_str().unwrap())
                    .replace("{{denied}}", tmp.join("denied").to_str().unwrap());
                (event.to_owned(), payload)
            })
            .collect()
    }

    /// Plays every hook of the recording `fixture`.
    fn play(&self, fixture: &str) {
        for (event, payload) in self.recording(fixture) {
            self.fire(&event, &payload);
        }
    }

    /// Plays the recording `fixture` one hook at a time, checking after each the state and the
    /// activity `worktree ls --json` shows: `expected[i]` after the hook `i`.
    fn play_checking(&self, fixture: &str, expected: &[(&str, Value)]) {
        let recording = self.recording(fixture);
        assert_eq!(
            recording.len(),
            expected.len(),
            "{fixture}: one check per hook"
        );
        for ((event, payload), (state, activity)) in recording.iter().zip(expected) {
            self.fire(event, payload);
            assert_eq!(
                self.shown(),
                ((*state).to_owned(), activity.clone()),
                "{fixture}: after {event}"
            );
        }
    }

    /// Runs every handler the settings register for `event` whose matcher takes the payload's
    /// tool, as claude would: the command with its arguments, `payload` on stdin, in the
    /// worktree. None for an event with no hook (`Notification`). Each must exit 0, silent.
    fn fire(&self, event: &str, payload: &str) {
        let tool: Value = serde_json::from_str::<Value>(payload).unwrap()["tool_name"].clone();
        let Some(entries) = self.settings["hooks"][event].as_array() else {
            return;
        };
        for entry in entries {
            let matcher = entry["matcher"].as_str().unwrap_or("*");
            if matcher != "*" && !matcher.is_empty() && tool != matcher {
                continue;
            }
            for handler in entry["hooks"].as_array().unwrap() {
                let args: Vec<&str> = handler["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|arg| arg.as_str().unwrap())
                    .collect();
                let mut command = Command::new(handler["command"].as_str().unwrap());
                command.args(&args).current_dir(&self.worktree).env_clear();
                let out = run_with_stdin(command, payload.as_bytes());
                assert_silent_success(&out, event);
            }
        }
    }

    /// The worktree's state and activity, as `worktree ls --json` shows them.
    fn shown(&self) -> (String, Value) {
        let entry = self.fx.listed(&self.name);
        (
            entry["state"].as_str().unwrap().to_owned(),
            entry["activity"].clone(),
        )
    }

    /// The turn log, parsed, one value per line.
    fn turns(&self) -> Vec<Value> {
        let text = fs::read_to_string(self.admin.join("turns.jsonl")).unwrap_or_default();
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// Each line of the turn log as (turn, kind, outcome), the outcome empty for a start.
    fn turn_summary(&self) -> Vec<(u64, String, String)> {
        self.turns()
            .iter()
            .map(|line| {
                (
                    line["turn"].as_u64().unwrap(),
                    line["kind"].as_str().unwrap().to_owned(),
                    line["outcome"].as_str().unwrap_or_default().to_owned(),
                )
            })
            .collect()
    }

    /// `status.json`, parsed.
    fn status(&self) -> Value {
        serde_json::from_slice(&fs::read(self.admin.join("status.json")).unwrap()).unwrap()
    }

    /// The recording's session id, from its first hook.
    fn session_of(&self, fixture: &str) -> String {
        let (_, payload) = &self.recording(fixture)[0];
        let payload: Value = serde_json::from_str(payload).unwrap();
        payload["session_id"].as_str().unwrap().to_owned()
    }
}

/// Runs `command` with `stdin` written to its standard input, collecting its output.
fn run_with_stdin(mut command: Command, stdin: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // A hook that exits without reading all of stdin closes the pipe: not an error here.
    let _ = child.stdin.take().unwrap().write_all(stdin);
    child.wait_with_output().unwrap()
}

/// What claude needs from a hook: exit 0 and nothing on stdout or stderr.
fn assert_silent_success(out: &Output, what: &str) {
    assert_eq!(
        out.status.code(),
        Some(0),
        "{what}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(out.stdout.is_empty(), "{what}: stdout {:?}", out.stdout);
    assert!(out.stderr.is_empty(), "{what}: stderr {:?}", out.stderr);
}

fn activity(text: &str) -> Value {
    json!(text)
}

const NONE: Value = Value::Null;

fn turn(n: u64, kind: &str, outcome: &str) -> (u64, String, String) {
    (n, kind.to_owned(), outcome.to_owned())
}

#[test]
fn a_turn_that_stops_is_finished_showing_the_agents_last_words() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    assert_eq!(played.shown(), ("idle".to_owned(), NONE));
    played.play_checking(
        "p-hello-2.1.300.log",
        &[
            ("idle", NONE),
            ("working", activity("hello there")),
            ("finished", activity("ok")),
            ("finished", activity("ok")),
        ],
    );
    assert_eq!(
        played.status()["session"],
        json!(played.session_of("p-hello-2.1.300.log")),
        "the session is the worktree's current session"
    );
    assert_eq!(
        played.turn_summary(),
        [turn(1, "start", ""), turn(1, "end", "finished")]
    );
    assert!(
        !fx.repo.home.join(".claude").exists() && !fx.repo.home.join(".claude.json").exists(),
        "nothing is written to claude's global config"
    );
}

#[test]
fn a_turn_ending_on_an_api_error_is_failed() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    played.play_checking(
        "p-stopfailure-2.1.300.log",
        &[
            ("idle", NONE),
            ("working", activity("FAIL now")),
            ("failed", activity("API Error: 400 fake bad request")),
            ("failed", activity("API Error: 400 fake bad request")),
        ],
    );
    assert_eq!(played.turns()[1]["error"], "unknown");
}

#[test]
fn a_command_the_sandbox_denies_shows_as_the_activity_and_the_turn_still_finishes() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    let denied = fx.repo.tmp.path().join("denied/from-bash.txt");
    let touch = activity(&format!("Bash: touch {}", denied.display()));
    played.play_checking(
        "p-sandbox-denied-bash-2.1.300.log",
        &[
            ("idle", NONE),
            ("working", activity("BASHTOUCH please")),
            ("working", touch.clone()),
            ("working", touch),
            ("finished", activity("All done. Second line here.")),
            ("finished", activity("All done. Second line here.")),
        ],
    );
}

#[test]
fn esc_then_a_new_prompt_interrupts_the_first_turn() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    played.play_checking(
        "tui-esc-2.1.300.log",
        &[
            ("idle", NONE),
            ("working", activity("SLOW please")),
            ("working", activity("hello again")),
            ("finished", activity("ok")),
            ("finished", activity("ok")),
        ],
    );
    assert_eq!(
        played.turn_summary(),
        [
            turn(1, "start", ""),
            turn(1, "end", "interrupted"),
            turn(2, "start", ""),
            turn(2, "end", "finished"),
        ]
    );
}

#[test]
fn a_queued_prompt_run_as_its_own_turn_makes_two_finished_turns() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    played.play("tui-queue-2.1.300.log");
    assert_eq!(played.shown(), ("finished".to_owned(), activity("ok")));
    assert_eq!(
        played.turn_summary(),
        [
            turn(1, "start", ""),
            turn(1, "end", "finished"),
            turn(2, "start", ""),
            turn(2, "end", "finished"),
        ]
    );
    let turns = played.turns();
    assert_eq!(turns[0]["prompt"], "SLOW please");
    assert_eq!(
        turns[2]["prompt"], "hello queued",
        "named after the queued prompt"
    );
}

#[test]
fn a_queued_prompt_folded_into_the_running_turn_makes_one_turn() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    played.play_checking(
        "tui-queuetool-2.1.300.log",
        &[
            ("idle", NONE),
            ("working", activity("SLEEP please")),
            ("working", activity("Bash: sleep 6")),
            ("working", activity("Bash: sleep 6")),
            ("working", activity("Bash: sleep 6")),
            ("finished", activity("All done. Second line here.")),
            ("finished", activity("All done. Second line here.")),
        ],
    );
    assert_eq!(
        played.turn_summary(),
        [turn(1, "start", ""), turn(1, "end", "finished")]
    );
}

#[test]
fn a_queued_prompt_then_esc_interrupts_the_first_turn() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    played.play("tui-queueesc-2.1.300.log");
    assert_eq!(played.shown(), ("finished".to_owned(), activity("ok")));
    assert_eq!(
        played.turn_summary(),
        [
            turn(1, "start", ""),
            turn(1, "end", "interrupted"),
            turn(2, "start", ""),
            turn(2, "end", "finished"),
        ]
    );
    assert_eq!(played.turns()[2]["prompt"], "hello queued");
}

#[test]
fn a_permission_prompt_waits_then_works_then_finishes() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    let ls = activity("Bash: FOO=bar ls");
    played.play_checking(
        "tui-perm-2.1.300.log",
        &[
            ("idle", NONE),
            ("working", activity("PERM please")),
            ("working", ls.clone()),
            ("waiting", ls.clone()),
            // Notification: no hook is registered for it.
            ("waiting", ls.clone()),
            // Esc on the dialog sent nothing; the next prompt interrupts the first turn.
            ("working", activity("PERM again")),
            ("working", ls.clone()),
            ("waiting", ls.clone()),
            ("working", ls),
            ("finished", activity("All done. Second line here.")),
            ("finished", activity("All done. Second line here.")),
        ],
    );
    assert_eq!(played.turn_summary()[1], turn(1, "end", "interrupted"));
}

#[test]
fn a_question_waits_showing_the_question_then_finishes() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    let done = activity("All done. Second line here.");
    played.play_checking(
        "tui-ask-2.1.300.log",
        &[
            ("idle", NONE),
            ("working", activity("ASK please")),
            ("working", activity("AskUserQuestion")),
            ("waiting", activity("Which colour?")),
            ("working", activity("Which colour?")),
            ("finished", done),
            ("working", activity("ASK again")),
            ("working", activity("AskUserQuestion")),
            ("waiting", activity("Which colour?")),
            // The session ended (SIGHUP) while the question was shown.
            ("interrupted", activity("ASK again")),
        ],
    );
}

#[test]
fn a_web_fetch_waits_for_permission_until_the_session_ends() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    let fetch = activity("WebFetch: https://example.org/");
    played.play_checking(
        "tui-web-2.1.300.log",
        &[
            ("idle", NONE),
            ("working", activity("WEB please")),
            ("working", fetch.clone()),
            ("waiting", fetch.clone()),
            ("waiting", fetch),
            ("interrupted", activity("WEB please")),
        ],
    );
}

#[test]
fn a_hang_up_mid_turn_is_interrupted() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    played.play_checking(
        "tui-kill-2.1.300.log",
        &[
            ("idle", NONE),
            ("working", activity("SLOW please")),
            ("interrupted", activity("SLOW please")),
        ],
    );
    played.play("tui-esctool-2.1.300.log");
    assert_eq!(
        played.shown(),
        ("interrupted".to_owned(), activity("SLEEP please")),
        "Esc during a tool, then a hang-up"
    );
}

#[test]
fn exit_after_a_finished_turn_stays_finished() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    played.play("tui-exit-2.1.300.log");
    assert_eq!(played.shown(), ("finished".to_owned(), activity("ok")));
    assert_eq!(played.turn_summary().len(), 2, "/exit opens no turn");
}

#[test]
fn an_open_turn_whose_agent_is_gone_is_interrupted() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    let recording = played.recording("tui-esctool-2.1.300.log");
    for (event, payload) in &recording[..3] {
        played.fire(event, payload);
    }
    assert_eq!(
        played.shown(),
        ("working".to_owned(), activity("Bash: sleep 6"))
    );
    fx.ok(&["agent", "stop", "fix"]);
    let entry = fx.listed("fix");
    assert_eq!(entry["holder"], false);
    assert_eq!(entry["state"], "interrupted");
    assert_eq!(entry["activity"], "SLEEP please", "the turn's prompt");
    assert_eq!(
        played.turn_summary(),
        [turn(1, "start", "")],
        "reading wrote nothing"
    );
}

#[test]
fn a_fresh_session_after_a_finished_turn_reads_idle() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    played.play("p-hello-2.1.300.log");
    assert_eq!(played.shown().0, "finished");
    let (event, payload) = &played.recording("tui-exit-2.1.300.log")[0];
    assert_eq!(event, "SessionStart");
    played.fire(event, payload);
    assert_eq!(played.shown(), ("idle".to_owned(), NONE));
    assert_eq!(
        played.status()["session"],
        json!(played.session_of("tui-exit-2.1.300.log"))
    );
}

#[test]
fn turns_are_numbered_across_sessions_and_the_log_only_grows() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    played.play("p-hello-2.1.300.log");
    let first = fs::read(played.admin.join("turns.jsonl")).unwrap();
    played.play("tui-esc-2.1.300.log");
    let numbers: Vec<u64> = played.turn_summary().iter().map(|line| line.0).collect();
    assert_eq!(numbers, [1, 1, 2, 2, 3, 3]);
    let sessions: Vec<Value> = played
        .turns()
        .iter()
        .map(|l| l["session"].clone())
        .collect();
    assert_eq!(sessions[0], json!(played.session_of("p-hello-2.1.300.log")));
    assert_eq!(sessions[2], json!(played.session_of("tui-esc-2.1.300.log")));
    let after = fs::read(played.admin.join("turns.jsonl")).unwrap();
    assert!(
        after.starts_with(&first),
        "earlier lines are never rewritten"
    );
    assert_eq!(played.status()["latest"]["number"], 3);
}

/// The first `UserPromptSubmit` of a recording, as a valid payload for the guard tests.
fn prompt_payload() -> String {
    let text = fs::read_to_string(Path::new(FIXTURES).join("p-hello-2.1.300.log")).unwrap();
    let line = text
        .lines()
        .find(|line| line.starts_with("UserPromptSubmit "))
        .unwrap();
    line.split_once(' ').unwrap().1.to_owned()
}

/// Runs `orca-term hook` with `args` and `stdin`, from the primary checkout.
fn hook(fx: &AgentRepo, args: &[&str], stdin: &[u8]) -> Output {
    let mut command = fx.repo.command(&fx.repo.root);
    command.arg("hook").args(args);
    run_with_stdin(command, stdin)
}

/// `hook.log` in `admin`, empty when missing.
fn hook_log(admin: &Path) -> String {
    fs::read_to_string(admin.join("hook.log")).unwrap_or_default()
}

#[test]
fn a_hook_exits_0_silently_whatever_it_is_given() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    let dir = admin.to_str().unwrap();
    let missing = fx.repo.tmp.path().join("no/such/admin");
    let gone = missing.to_str().unwrap();
    let payload = prompt_payload();
    let cases: [(&[&str], &[u8]); 9] = [
        (&["claude", "--admin", dir, "UserPromptSubmit"], b""),
        (
            &["claude", "--admin", dir, "UserPromptSubmit"],
            b"{not json",
        ),
        (
            &["claude", "--admin", dir, "Nope"],
            br#"{"hook_event_name":"Nope","session_id":"s"}"#,
        ),
        (&["claude", "--bogus", "-x", "--admin", dir, "Stop"], b"{}"),
        (&["claude", "--admin", gone, "Stop"], payload.as_bytes()),
        (&["opencode", "--admin", dir, "Stop"], payload.as_bytes()),
        (&["claude", "--admin"], payload.as_bytes()),
        (&[], b""),
        (&["--help"], b""),
    ];
    for (args, stdin) in cases {
        let out = hook(&fx, args, stdin);
        assert_silent_success(&out, &format!("hook {args:?}"));
    }
    assert!(
        !missing.exists(),
        "nothing is created for a missing admin dir"
    );
    let log = hook_log(&admin);
    assert!(log.contains("not JSON"), "{log}");
    assert!(log.contains("hook_event_name"), "{log}");
    assert!(log.contains("opencode"), "{log}");
    assert_eq!(log.lines().count(), 4, "one line per failure: {log}");
}

#[test]
fn a_hook_gives_up_on_a_held_lock_within_two_seconds() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    let dir = admin.to_str().unwrap();
    let lock = File::create(admin.join("hook.lock")).unwrap();
    lock.try_lock().unwrap();

    let started = Instant::now();
    let out = hook(
        &fx,
        &["claude", "--admin", dir, "UserPromptSubmit"],
        prompt_payload().as_bytes(),
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "{:?}",
        started.elapsed()
    );
    assert_silent_success(&out, "a held hook.lock");
    assert!(!admin.join("status.json").exists(), "nothing was recorded");
    assert!(
        hook_log(&admin).contains("hook.lock"),
        "{}",
        hook_log(&admin)
    );

    drop(lock);
    let out = hook(
        &fx,
        &["claude", "--admin", dir, "UserPromptSubmit"],
        prompt_payload().as_bytes(),
    );
    assert_silent_success(&out, "a free hook.lock");
    assert!(
        admin.join("status.json").exists(),
        "recorded once the lock is free"
    );
}

#[test]
fn a_hook_never_writes_over_a_newer_schema_and_ls_reads_it_as_idle() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    let dir = played.admin.to_str().unwrap();
    let newer = br#"{"schema":2,"turn":{"elsewhere":true}}"#;
    fs::write(played.admin.join("status.json"), newer).unwrap();
    let out = hook(
        &fx,
        &["claude", "--admin", dir, "UserPromptSubmit"],
        prompt_payload().as_bytes(),
    );
    assert_silent_success(&out, "a newer status.json");
    assert_eq!(fs::read(played.admin.join("status.json")).unwrap(), newer);
    assert!(
        hook_log(&played.admin).contains("schema 2"),
        "{}",
        hook_log(&played.admin)
    );
    assert_eq!(
        played.shown(),
        ("idle".to_owned(), NONE),
        "ls never fails on it"
    );

    fs::remove_file(played.admin.join("status.json")).unwrap();
    let newer_log = b"{\"schema\":3,\"kind\":\"start\",\"turn\":1}\n";
    fs::write(played.admin.join("turns.jsonl"), newer_log).unwrap();
    let out = hook(
        &fx,
        &["claude", "--admin", dir, "UserPromptSubmit"],
        prompt_payload().as_bytes(),
    );
    assert_silent_success(&out, "a newer turns.jsonl");
    assert_eq!(
        fs::read(played.admin.join("turns.jsonl")).unwrap(),
        newer_log
    );
    assert!(!played.admin.join("status.json").exists());

    fs::remove_file(played.admin.join("turns.jsonl")).unwrap();
    fs::write(played.admin.join("status.json"), b"{corrupt").unwrap();
    assert_eq!(
        played.shown(),
        ("idle".to_owned(), NONE),
        "a corrupt status reads idle"
    );
    played.play("p-hello-2.1.300.log");
    assert_eq!(
        played.shown(),
        ("finished".to_owned(), activity("ok")),
        "and is rewritten"
    );
    assert!(
        hook_log(&played.admin).contains("starts afresh"),
        "{}",
        hook_log(&played.admin)
    );
}

#[test]
fn setup_and_rm_are_never_busy_because_of_a_hook() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    let lock = File::create(admin.join("hook.lock")).unwrap();
    lock.try_lock().unwrap();

    let out = fx.run(&["worktree", "setup", "fix"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = fx.run(&["worktree", "rm", "fix"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!worktree.exists());
    assert!(!admin.exists(), "the admin dir goes with the worktree");
    drop(lock);
}
