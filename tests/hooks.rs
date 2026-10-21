//! claude's hooks, driven as claude drives them (seam 1): `agent start` writes the settings with
//! the hooks, and each test plays a recording of what claude 2.1.300 sent
//! (`tests/fixtures/claude-hooks/*.log`, one hook per line: the event, a space, the payload) by
//! running the handler the settings register for each event, exactly as written (command and
//! arguments, the payload on stdin, the worktree as the directory), then reads `worktree ls
//! --json`. The fake claude itself sends no hook. Also the guards: a hook exits 0 and prints
//! nothing whatever it is given, and never makes a command busy.
//!
//! The recordings had their absolute paths replaced by `{{worktree}}`, `{{transcripts}}`,
//! `{{denied}}` and `{{tasks}}`, which are written back here with paths of the test's own.

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

use fixture::{AgentRepo, holder_alive};
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
                    .replace("{{denied}}", tmp.join("denied").to_str().unwrap())
                    .replace("{{tasks}}", tmp.join("tasks").to_str().unwrap());
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

    /// Plays the hooks of the recording `fixture` from the hook `from` (counted from 0) up to,
    /// not including, the hook `to`.
    fn play_part(&self, fixture: &str, from: usize, to: usize) {
        for (event, payload) in &self.recording(fixture)[from..to] {
            self.fire(event, payload);
        }
    }

    /// `worktree detail --json` of the worktree.
    fn detail(&self) -> Value {
        serde_json::from_str(&self.fx.ok(&["worktree", "detail", &self.name, "--json"])).unwrap()
    }

    /// How many denials `worktree ls --json` counts on the worktree.
    fn denial_count(&self) -> u64 {
        self.fx.listed(&self.name)["denials"].as_u64().unwrap()
    }

    /// Where the recordings' `{{denied}}` directory is, in the test's own directory.
    fn denied(&self, file: &str) -> String {
        self.fx
            .repo
            .tmp
            .path()
            .join("denied")
            .join(file)
            .to_str()
            .unwrap()
            .to_owned()
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

    /// Whether `worktree ls --json` shows the worktree unseen.
    fn unseen(&self) -> bool {
        self.fx.listed(&self.name)["unseen"].as_bool().unwrap()
    }

    /// Runs `worktree <command> <name>` (`seen` or `unseen`) and returns the line it prints.
    fn mark(&self, command: &str) -> String {
        self.fx.ok(&["worktree", command, &self.name])
    }

    /// `seen.json`, parsed.
    fn seen_file(&self) -> Value {
        serde_json::from_slice(&fs::read(self.admin.join("seen.json")).unwrap()).unwrap()
    }

    /// Where the recording `fixture` says its session's transcript is, in the test's own
    /// directory (`{{transcripts}}/<session>.jsonl`). Nothing creates it.
    fn transcript_of(&self, fixture: &str) -> PathBuf {
        let session = self.session_of(fixture);
        self.fx
            .repo
            .tmp
            .path()
            .join("transcripts")
            .join(format!("{session}.jsonl"))
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

#[test]
fn a_turn_that_ends_makes_the_worktree_unseen_until_the_person_marks_it_seen() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    assert!(!played.unseen(), "no turn has ended");
    played.play("p-hello-2.1.300.log");
    assert!(played.unseen(), "turn 1 finished");
    assert!(
        !played.admin.join("seen.json").exists(),
        "hooks and listing write no seen record"
    );

    assert_eq!(played.mark("seen"), "fix: seen\n");
    assert!(!played.unseen());
    assert_eq!(
        played.seen_file(),
        json!({"schema": 1, "seen_through": 1, "marked": false})
    );
    assert_eq!(played.mark("seen"), "fix: seen\n", "idempotent");

    played.play("tui-esc-2.1.300.log");
    assert!(played.unseen(), "turn 3 finished after the review");
    played.mark("seen");
    assert_eq!(played.seen_file()["seen_through"], 3);

    let out = fx.ok(&["worktree", "unseen", "fix", "--json"]);
    let doc: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(doc["contract"], 1);
    assert_eq!(doc["name"], "fix");
    assert_eq!(doc["path"], json!(played.worktree));
    assert_eq!(doc["unseen"], true);
    assert!(played.unseen(), "marked by hand with no new turn");
    assert_eq!(
        played.seen_file(),
        json!({"schema": 1, "seen_through": 3, "marked": true}),
        "the mark keeps what was seen"
    );
    assert_eq!(played.mark("unseen"), "fix: unseen\n", "idempotent");

    // A fresh session reads idle, and keeps the mark: it is independent of the state.
    let (event, payload) = &played.recording("tui-exit-2.1.300.log")[0];
    played.fire(event, payload);
    assert_eq!(played.shown().0, "idle");
    assert!(played.unseen());

    let doc: Value = serde_json::from_str(&fx.ok(&["worktree", "seen", "fix", "--json"])).unwrap();
    assert_eq!(doc["unseen"], false);
    assert!(!played.unseen());
    let table = fx.ok(&["worktree", "ls"]);
    assert!(!table.contains("unseen"), "{table}");
    played.mark("unseen");
    let table = fx.ok(&["worktree", "ls"]);
    assert!(table.contains("idle (unseen)"), "{table}");

    let help = fx.ok(&["worktree", "--help"]);
    assert!(
        help.contains("seen") && help.contains("unseen"),
        "public commands: {help}"
    );
}

#[test]
fn a_failed_turn_and_an_interrupted_one_are_unseen() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    played.play("p-stopfailure-2.1.300.log");
    assert_eq!(played.shown().0, "failed");
    assert!(played.unseen());
    played.mark("seen");
    assert!(!played.unseen());

    played.play("tui-kill-2.1.300.log");
    assert_eq!(played.shown().0, "interrupted");
    assert!(played.unseen(), "a hang-up mid-turn ended it");
}

#[test]
fn an_open_turn_whose_agent_is_gone_has_ended_and_is_unseen() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    let recording = played.recording("tui-esctool-2.1.300.log");
    for (event, payload) in &recording[..3] {
        played.fire(event, payload);
    }
    assert_eq!(played.shown().0, "working");
    assert!(!played.unseen(), "the turn is still in progress");
    played.mark("seen");
    assert_eq!(played.seen_file()["seen_through"], 0, "nothing had ended");

    fx.ok(&["agent", "stop", "fix"]);
    assert_eq!(played.shown().0, "interrupted");
    assert!(played.unseen());
    played.mark("seen");
    assert_eq!(played.seen_file()["seen_through"], 1, "the open turn ended");
    assert!(!played.unseen());
    assert_eq!(
        played.turn_summary(),
        [turn(1, "start", "")],
        "marking writes no turn"
    );
}

#[test]
fn a_worktree_that_never_ran_a_turn_is_not_unseen_and_can_still_be_marked() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    assert_eq!(fx.listed("fix")["unseen"], false);
    assert_eq!(fx.ok(&["worktree", "seen", "fix"]), "fix: seen\n");
    assert_eq!(fx.listed("fix")["unseen"], false);
    assert_eq!(fx.ok(&["worktree", "unseen", "fix"]), "fix: unseen\n");
    assert_eq!(fx.listed("fix")["unseen"], true, "no agent, marked by hand");
    assert_eq!(fx.listed("fix")["state"], "no_agent");
    assert!(admin.join("seen.json").exists());

    // A worktree made by hand with git, which has no admin directory of ours yet, by its path.
    let by_hand = fx.repo.tmp.path().join("by-hand");
    let by_hand = fx.repo.add_worktree(&by_hand, "by-hand");
    assert_eq!(fx.listed("by-hand")["unseen"], false);
    let target = by_hand.to_str().unwrap();
    assert_eq!(fx.ok(&["worktree", "unseen", target]), "by-hand: unseen\n");
    assert_eq!(fx.listed("by-hand")["unseen"], true);
}

#[test]
fn seen_and_unseen_take_no_lock_so_they_are_never_busy() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    let lock = File::open(admin.join("lock")).unwrap();
    lock.try_lock().unwrap();
    let hook_lock = File::create(admin.join("hook.lock")).unwrap();
    hook_lock.try_lock().unwrap();

    let started = Instant::now();
    assert_eq!(fx.ok(&["worktree", "unseen", "fix"]), "fix: unseen\n");
    assert_eq!(fx.ok(&["worktree", "seen", "fix"]), "fix: seen\n");
    assert!(started.elapsed() < Duration::from_secs(2), "nothing waited");
    let out = fx.run(&["worktree", "setup", "fix", "--json"]);
    assert_eq!(out.status.code(), Some(3), "the worktree's lock is held");
    drop((lock, hook_lock));
}

#[test]
fn seen_and_unseen_refuse_the_primary_checkout_and_an_unknown_worktree() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");
    let root = fx.repo.root.to_str().unwrap().to_owned();
    for command in ["seen", "unseen"] {
        for (target, says) in [
            ("repo", "is the primary checkout"),
            (root.as_str(), "is the primary checkout"),
            ("nope", "no worktree named `nope`"),
        ] {
            let out = fx.run(&["worktree", command, target, "--json"]);
            assert_eq!(out.status.code(), Some(1), "{command} {target}");
            let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
            assert_eq!(doc["error"]["kind"], "other");
            let message = doc["error"]["message"].as_str().unwrap();
            assert!(message.contains(says), "{message}");
            let out = fx.run(&["worktree", command, target]);
            assert_eq!(out.status.code(), Some(1));
            assert!(String::from_utf8_lossy(&out.stderr).contains(says));
        }
    }
    assert!(!fx.repo.root.join(".git/orca-term/seen.json").exists());
}

#[test]
fn seen_never_writes_over_a_newer_schema_and_a_record_it_cannot_use_reads_as_nothing_seen() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    played.play("p-hello-2.1.300.log");
    let path = played.admin.join("seen.json");

    let newer = br#"{"schema":2,"seen_through":9,"marked":false}"#;
    fs::write(&path, newer).unwrap();
    assert!(played.unseen(), "a newer record reads as nothing seen");
    for command in ["seen", "unseen"] {
        let out = fx.run(&["worktree", command, "fix"]);
        assert_eq!(out.status.code(), Some(1));
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("schema 2"), "{stderr}");
        assert_eq!(fs::read(&path).unwrap(), newer, "left as it was");
    }

    fs::write(&path, b"{corrupt").unwrap();
    assert!(played.unseen(), "a corrupt record reads as nothing seen");
    assert_eq!(played.mark("seen"), "fix: seen\n", "and is written over");
    assert!(!played.unseen());
}

/// Runs git in the worktree at `worktree` and returns what it printed, trimmed.
fn git_in(fx: &AgentRepo, worktree: &Path, args: &[&str]) -> String {
    let mut all = vec!["-C", worktree.to_str().unwrap()];
    all.extend_from_slice(args);
    fx.repo.git(&all).trim().to_owned()
}

/// Runs `worktree review <name> --json` and returns its document.
fn review(fx: &AgentRepo, name: &str) -> Value {
    serde_json::from_str(&fx.ok(&["worktree", "review", name, "--json"])).unwrap()
}

#[test]
fn review_gives_the_commit_the_worktree_forked_from_and_marks_it_seen() {
    let fx = AgentRepo::new();
    let fork = fx.repo.git(&["rev-parse", "main"]).trim().to_owned();
    let played = Played::start(&fx, "demo");
    played.play("p-hello-2.1.300.log");
    assert!(played.unseen(), "turn 1 finished");

    // A commit of the worktree's own, then a change not committed yet.
    fs::write(played.worktree.join("feature.txt"), "one\n").unwrap();
    git_in(&fx, &played.worktree, &["add", "feature.txt"]);
    git_in(
        &fx,
        &played.worktree,
        &["commit", "--quiet", "-m", "feature"],
    );
    fs::write(played.worktree.join("feature.txt"), "two\n").unwrap();

    let doc = review(&fx, "demo");
    assert_eq!(doc["contract"], 1);
    assert_eq!(doc["base"], json!(fork));
    assert_eq!(doc["against"], "main");
    assert_eq!(doc["worktree"]["name"], "demo");
    assert_eq!(doc["worktree"]["path"], json!(played.worktree));
    assert_eq!(
        doc["worktree"]["unseen"], false,
        "the worktree as it is now"
    );
    assert!(!played.unseen(), "reviewing marks it seen");
    assert_eq!(
        played.seen_file(),
        json!({"schema": 1, "seen_through": 1, "marked": false}),
        "as `worktree seen` marks it"
    );

    // The default branch moves on: the review still starts where the worktree forked.
    fx.repo.write("main.txt", "main\n");
    fx.repo.commit_all("main moves on");
    let tip = fx.repo.git(&["rev-parse", "main"]).trim().to_owned();
    assert_ne!(tip, fork);
    assert_eq!(review(&fx, "demo")["base"], json!(fork));

    let text = fx.ok(&["worktree", "review", "demo"]);
    assert_eq!(
        text,
        format!("demo: review against main ({})\n", &fork[..7])
    );

    let help = fx.ok(&["worktree", "--help"]);
    assert!(help.contains("review"), "public command: {help}");
}

#[test]
fn review_refuses_the_primary_checkout_an_unknown_worktree_and_a_broken_one() {
    let fx = AgentRepo::new();
    let gone = fx.new_worktree("gone");
    let gone_admin = fx.admin_dir(&gone);
    fs::remove_dir_all(&gone).unwrap();
    let root = fx.repo.root.to_str().unwrap().to_owned();
    for (target, says) in [
        ("repo", "is the primary checkout"),
        (root.as_str(), "is the primary checkout"),
        ("nope", "no worktree named `nope`"),
        ("gone", "nothing to review"),
    ] {
        let out = fx.run(&["worktree", "review", target, "--json"]);
        assert_eq!(out.status.code(), Some(1), "{target}");
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["error"]["kind"], "other");
        let message = doc["error"]["message"].as_str().unwrap();
        assert!(message.contains(says), "{message}");
        let out = fx.run(&["worktree", "review", target]);
        assert_eq!(out.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&out.stderr).contains(says));
    }
    assert!(!fx.repo.root.join(".git/orca-term/seen.json").exists());
    assert!(!gone_admin.join("seen.json").exists());
}

#[test]
fn review_of_a_history_unrelated_to_the_default_branch_fails_and_marks_nothing() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("orphan");
    let admin = fx.admin_dir(&worktree);
    git_in(
        &fx,
        &worktree,
        &["checkout", "--quiet", "--orphan", "elsewhere"],
    );
    git_in(
        &fx,
        &worktree,
        &["commit", "--quiet", "--allow-empty", "-m", "unrelated"],
    );

    let out = fx.run(&["worktree", "review", "orphan", "--json"]);
    assert_eq!(out.status.code(), Some(1));
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    let message = doc["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("shares no commit with the default branch `main`"),
        "{message}"
    );
    assert!(
        !admin.join("seen.json").exists(),
        "a failed review marks nothing"
    );
}

/// Writes an empty file at `path`, creating its directory: a transcript claude kept.
fn create_file(path: &Path) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"").unwrap();
}

/// Runs `agent start` with `args` and the error document `--json` printed, checking it failed
/// as `session_gone` with exit code 4.
fn session_gone(fx: &AgentRepo, args: &[&str]) -> String {
    let out = fx.run(args);
    assert_eq!(out.status.code(), Some(4), "{args:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"]["kind"], "session_gone", "{doc}");
    doc["error"]["message"].as_str().unwrap().to_owned()
}

#[test]
fn a_cold_start_resumes_the_recorded_session() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    assert!(
        !fx.argv().contains(&"--resume".to_owned()),
        "the first start has nothing to resume: {:?}",
        fx.argv()
    );
    played.play("p-hello-2.1.300.log");
    let session = played.session_of("p-hello-2.1.300.log");
    let transcript = played.transcript_of("p-hello-2.1.300.log");
    assert_eq!(played.status()["transcript"], json!(transcript));
    create_file(&transcript);
    fx.ok(&["agent", "stop", "fix"]);

    let doc: Value = serde_json::from_str(&fx.ok(&["agent", "start", "fix", "--json"])).unwrap();
    assert_eq!(doc["holder"]["status"], "started");
    assert_eq!(doc["resumed"], json!(session));
    let argv = fx.argv();
    assert_eq!(
        argv[argv.len() - 2..],
        ["--resume".to_owned(), session.clone()]
    );
    assert_eq!(argv[2..4], ["--permission-mode", "acceptEdits"]);
    assert_eq!(fx.starts(), 2);

    fx.ok(&["agent", "stop", "fix"]);
    let text = fx.ok(&["agent", "start", "fix"]);
    assert!(
        text.starts_with(&format!(
            "started agent claude-code in worktree fix, resuming session {session}\n"
        )),
        "{text}"
    );
}

#[test]
fn a_cold_start_without_a_recorded_session_starts_a_new_one() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");
    let doc: Value = serde_json::from_str(&fx.ok(&["agent", "start", "fix", "--json"])).unwrap();
    assert_eq!(doc["holder"]["status"], "started");
    assert_eq!(doc["resumed"], NONE);
    assert!(
        !fx.argv().contains(&"--resume".to_owned()),
        "{:?}",
        fx.argv()
    );
    fx.ok(&["agent", "stop", "fix"]);
    let text = fx.ok(&["agent", "start", "fix"]);
    assert!(
        text.starts_with("started agent claude-code in worktree fix\n"),
        "{text}"
    );
    assert!(
        !fx.argv().contains(&"--resume".to_owned()),
        "{:?}",
        fx.argv()
    );
}

#[test]
fn a_session_whose_transcript_is_gone_is_refused_and_fresh_starts_a_new_one() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    played.play("p-hello-2.1.300.log");
    let session = played.session_of("p-hello-2.1.300.log");
    let transcript = played.transcript_of("p-hello-2.1.300.log");
    create_file(&transcript);
    fx.ok(&["agent", "stop", "fix"]);
    fs::remove_file(&transcript).unwrap();

    let message = session_gone(&fx, &["agent", "start", "fix", "--json"]);
    assert_eq!(
        message,
        format!(
            "worktree fix cannot resume its session {session}: its transcript {} is gone; start \
             a fresh session with `orca-term agent start fix --fresh`",
            transcript.display()
        )
    );
    let out = fx.run(&["agent", "start", "fix"]);
    assert_eq!(out.status.code(), Some(4));
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        format!("orca-term: {message}\n")
    );
    assert_eq!(fx.starts(), 1, "nothing was launched");
    assert!(!holder_alive(&played.admin));

    let text = fx.ok(&["agent", "start", "fix", "--fresh"]);
    assert!(
        text.starts_with("started agent claude-code in worktree fix\n"),
        "{text}"
    );
    assert!(
        !fx.argv().contains(&"--resume".to_owned()),
        "{:?}",
        fx.argv()
    );
    assert_eq!(fx.starts(), 2);
}

#[test]
fn a_recorded_session_id_that_is_a_flag_is_never_launched() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    fx.ok(&["agent", "stop", "fix"]);
    let transcript = fx.repo.tmp.path().join("transcripts/x.jsonl");
    create_file(&transcript);
    for id in ["--dangerously-skip-permissions", "-p", "a b", ""] {
        let status = json!({"schema": 1, "session": id, "transcript": transcript});
        fs::write(played.admin.join("status.json"), status.to_string()).unwrap();
        let message = session_gone(&fx, &["agent", "start", "fix", "--json"]);
        assert!(
            message.contains(": its id is not one an agent makes; "),
            "{id:?}: {message}"
        );
        assert!(message.contains("agent start fix --fresh"), "{message}");
    }
    assert_eq!(fx.starts(), 1, "never launched");
    assert!(!holder_alive(&played.admin));
}

#[test]
fn a_live_holder_with_fresh_is_reported_as_live() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    played.play("p-hello-2.1.300.log");
    let doc: Value =
        serde_json::from_str(&fx.ok(&["agent", "start", "fix", "--fresh", "--json"])).unwrap();
    assert_eq!(doc["holder"]["status"], "already_running");
    assert_eq!(doc["resumed"], NONE);
    assert_eq!(fx.starts(), 1, "no second agent");
}

#[test]
fn every_launch_registers_the_batch_hook_that_reports_denials() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    let entries = played.settings["hooks"]["PostToolBatch"]
        .as_array()
        .unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].get("matcher"), None, "every batch");
}

#[test]
fn denials_are_counted_on_the_latest_turn_and_detailed_one_per_line() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    assert_eq!(played.denial_count(), 0);
    played.play("tui-deny-multi-2.1.303.log");
    assert_eq!(played.shown().0, "finished", "a denial is never a state");
    assert_eq!(played.denial_count(), 3);
    let doc = played.detail();
    let write =
        |file: &str| json!({"tool": "Write", "text": format!("Write: {}", played.denied(file))});
    assert_eq!(
        doc["denials"],
        json!([write("a.txt"), write("b.txt"), write("c.txt")])
    );
    assert_eq!(doc["worktree"]["denials"], 3);
    let end = &played.turns()[1];
    assert_eq!(
        end["denials"],
        json!([write("a.txt"), write("b.txt"), write("c.txt")]),
        "the turn's end line keeps them, without the call ids"
    );

    let text = fx.ok(&["worktree", "detail", "fix"]);
    assert!(
        text.starts_with(&format!(
            "fix: finished (unseen)\ndenied 3 tool calls in the latest turn:\n  Write: {}\n",
            played.denied("a.txt")
        )),
        "{text}"
    );
}

#[test]
fn a_bash_deny_rule_shows_while_the_turn_runs_and_the_turn_still_finishes() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    let ls = activity("Bash: FOO=bar ls");
    played.play_checking(
        "tui-deny-rule-bash-2.1.303.log",
        &[
            ("idle", NONE),
            ("working", activity("PERM please")),
            ("working", ls.clone()),
            // PostToolBatch: the denial, with no change of state or activity.
            ("working", ls),
            ("finished", activity("All done. Second line here.")),
            ("finished", activity("All done. Second line here.")),
        ],
    );
    assert_eq!(played.denial_count(), 1);
    assert_eq!(
        played.detail()["denials"],
        json!([{"tool": "Bash", "text": "Bash: FOO=bar ls"}])
    );
}

#[test]
fn an_open_turn_counts_its_denials_and_a_new_turn_starts_from_none() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    // SessionStart, UserPromptSubmit, PostToolBatch: the turn is open.
    played.play_part("tui-deny-rule-2.1.303.log", 0, 3);
    assert_eq!(played.shown().0, "working");
    assert_eq!(played.denial_count(), 1);
    assert_eq!(
        played.status()["turn"]["denials"][0]["text"],
        json!(format!("Write: {}", played.denied("from-write.txt")))
    );
    played.play_part("tui-deny-rule-2.1.303.log", 3, 5);
    assert_eq!(played.denial_count(), 1, "the ended turn keeps them");

    // A denial, then Esc and a new prompt: the interrupted turn keeps it, the new one has none.
    let other = Played::start(&fx, "esc");
    other.play("tui-deny-esc-2.1.303.log");
    assert_eq!(other.denial_count(), 0);
    let ends: Vec<Value> = other
        .turns()
        .into_iter()
        .filter(|line| line["kind"] == "end")
        .collect();
    assert_eq!(ends[0]["outcome"], "interrupted");
    assert_eq!(ends[0]["denials"].as_array().unwrap().len(), 1);
    assert_eq!(ends[1].get("denials"), None);
}

#[test]
fn a_subagents_denial_counts_on_the_main_turn() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    // Up to the main turn's Stop.
    played.play_part("tui-deny-sub-2.1.303.log", 0, 7);
    assert_eq!(played.denial_count(), 1);
    assert_eq!(
        played.detail()["denials"],
        json!([{"tool": "Write", "text": format!("Write: {}", played.denied("from-write.txt"))}])
    );
    // The subagent's notification is a turn of its own, with none.
    played.play_part("tui-deny-sub-2.1.303.log", 7, 10);
    assert_eq!(played.denial_count(), 0);
}

#[test]
fn a_refusal_a_sandbox_block_and_a_lost_denial_count_nothing() {
    let fx = AgentRepo::new();
    for (name, fixture) in [
        ("tell", "tui-deny-tell-2.1.303.log"),
        ("no", "tui-deny-2.1.303.log"),
        ("mixed", "tui-deny-mixed-no-2.1.303.log"),
        ("sandbox", "tui-sandbox-2.1.303.log"),
    ] {
        let played = Played::start(&fx, name);
        played.play(fixture);
        assert_eq!(played.denial_count(), 0, "{fixture}");
        assert_eq!(played.detail()["denials"], json!([]), "{fixture}");
        assert!(
            played
                .turns()
                .iter()
                .all(|line| line.get("denials").is_none()),
            "{fixture}"
        );
    }
}

#[test]
fn a_failed_turn_gives_its_whole_error_in_the_detail() {
    let fx = AgentRepo::new();
    let played = Played::start(&fx, "fix");
    let session = r#""session_id":"s1","transcript_path":"/t/s1.jsonl""#;
    played.fire(
        "UserPromptSubmit",
        &format!(
            r#"{{{session},"hook_event_name":"UserPromptSubmit","prompt_id":"p1","prompt":"go"}}"#
        ),
    );
    let body = "x".repeat(400);
    let message = format!("API Error: 400\n{{\"error\":\"{body}\"}}");
    played.fire(
        "StopFailure",
        &json!({
            "session_id": "s1",
            "hook_event_name": "StopFailure",
            "prompt_id": "p1",
            "error": "invalid_request",
            "last_assistant_message": message,
        })
        .to_string(),
    );
    assert_eq!(
        played.shown(),
        ("failed".to_owned(), activity("API Error: 400"))
    );
    let doc = played.detail();
    assert_eq!(doc["detail"], json!(message), "every line, never cut");
    assert_eq!(doc["worktree"]["activity"], "API Error: 400");
    let text = fx.ok(&["worktree", "detail", "fix"]);
    assert!(
        text.starts_with(&format!(
            "fix: failed (unseen)\n  API Error: 400\n  {{\"error\":\"{body}\"}}\n"
        )),
        "{text}"
    );
}
