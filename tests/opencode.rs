//! opencode worktrees (#35), driven as a black box against a fake `opencode` and fake sandbox tools
//! that run what they wrap, on an isolated PATH (seam 1): the launch (its command line inside the
//! sandbox, the config directory holding our plugin, the events pipe), the status from events
//! written to that pipe exactly as the plugin writes them (the same states, turn log and snapshots
//! as claude's hooks give), the session a cold start resumes, `--agent`, `agent swap`, and the
//! refusals. The sandbox itself, for real, is `tests/opencode_sandbox.rs`.

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

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use fixture::{holder_alive, state_file, wait_until};
use opencode::OpenCodeRepo;
use serde_json::{Value, json};

/// The plugin as the source holds it, which the launch must write byte for byte.
const PLUGIN: &str = include_str!("../src/domain/launch/opencode/orca-term.js");

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("not JSON: {}", stdout(out)))
}

/// A worktree whose opencode the test starts.
struct Started<'a> {
    oc: &'a OpenCodeRepo,
    worktree: PathBuf,
    admin: PathBuf,
}

impl<'a> Started<'a> {
    /// Creates the worktree `name` and starts opencode in it.
    fn new(oc: &'a OpenCodeRepo, name: &str) -> Self {
        let worktree = oc.fx.new_worktree(name);
        let admin = oc.fx.admin_dir(&worktree);
        oc.ok(&["agent", "start", name, "--agent", "opencode"]);
        Self {
            oc,
            worktree,
            admin,
        }
    }

    /// The worktree's entry of `worktree ls --json`.
    fn listed(&self) -> Value {
        let name = self.worktree.file_name().unwrap().to_str().unwrap();
        self.oc.fx.listed(name)
    }

    /// Waits until `worktree ls` shows the worktree in `state`, with `activity`.
    fn wait_for_state(&self, state: &str, activity: &str) {
        wait_until(&format!("{state} ({activity})"), || {
            let listed = self.listed();
            listed["state"] == state && listed["activity"] == activity
        });
    }

    /// The turn log, one value per line.
    fn turns(&self) -> Vec<Value> {
        let text = fs::read_to_string(self.admin.join("turns.jsonl")).unwrap_or_default();
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// `status.json`, parsed.
    fn status(&self) -> Value {
        serde_json::from_slice(&fs::read(self.admin.join("status.json")).unwrap()).unwrap()
    }

    /// Runs git in the worktree and returns its stdout, trimmed.
    fn git(&self, args: &[&str]) -> String {
        let mut full = vec!["-C", self.worktree.to_str().unwrap()];
        full.extend_from_slice(args);
        self.oc.fx.repo.git(&full).trim_end().to_owned()
    }
}

/// The plugin's line of the person's prompt `text` in the session `ses_1`.
fn prompt(text: &str) -> Value {
    json!({"type": "prompt", "session": "ses_1", "parent": null, "text": text})
}

/// The plugin's line of the session `ses_1` going idle once the agent finished on its own, its
/// last words `text`.
fn idle(text: &str) -> Value {
    json!({
        "type": "idle", "session": "ses_1", "parent": null, "text": text,
        "finish": "stop", "error": null
    })
}

/// Checks the fake sandbox tool was given the generated profile (macOS) or arguments (Linux) for
/// the worktree at `worktree`, whose admin directory is `admin`, then the fake opencode.
fn check_sandbox_wrapper(oc: &OpenCodeRepo, worktree: &Path, admin: &Path) {
    let program = oc.fx.bin.join("opencode");
    let sandbox = oc.sandbox_argv();
    if cfg!(target_os = "macos") {
        let profile = admin.join("opencode-sandbox.sb");
        assert_eq!(
            sandbox,
            ["-f", profile.to_str().unwrap(), program.to_str().unwrap()]
        );
        let text = fs::read_to_string(&profile).unwrap();
        for rule in [
            format!("(subpath \"{}\")", worktree.display()),
            format!("(subpath \"{}\")", admin.display()),
            "(deny network-outbound (remote unix-socket))".to_owned(),
        ] {
            assert!(text.contains(&rule), "{rule} not in {text}");
        }
    } else {
        assert_eq!(sandbox.last().unwrap(), program.to_str().unwrap());
        let words = sandbox.join(" ");
        let (worktree, admin) = (worktree.display(), admin.display());
        for expected in [
            "--unshare-user".to_owned(),
            format!("--bind-try {worktree} {worktree}"),
            format!("--ro-bind {admin} {admin}"),
        ] {
            assert!(words.contains(&expected), "{expected} not in {words}");
        }
        assert!(
            !words.contains("--unshare-net"),
            "the network is kept: {words}"
        );
    }
}

/// Checks opencode was given the config directory in `admin`, holding our plugin byte for byte,
/// and the events pipe.
fn check_config_dir(oc: &OpenCodeRepo, admin: &Path) {
    let config = admin.join("opencode");
    assert_eq!(oc.config_dir(), config);
    assert_eq!(
        fs::read_to_string(config.join("plugins/orca-term.js")).unwrap(),
        PLUGIN
    );
    assert!(config.join(".gitignore").is_file());
    let names = oc.env_names();
    for name in ["OPENCODE_CONFIG_DIR", "ORCA_TERM_EVENTS_FD", "HOME", "PATH"] {
        assert!(names.iter().any(|n| n == name), "{name} not in {names:?}");
    }
}

#[test]
fn opencode_starts_in_its_sandbox_with_our_plugin_and_an_events_pipe() {
    let oc = OpenCodeRepo::new();
    let worktree = oc.fx.new_worktree("demo");
    let admin = oc.fx.admin_dir(&worktree);

    let out = oc.run(&["agent", "start", "demo", "--agent", "opencode", "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let doc = json(&out);
    assert_eq!(doc["agent"], "opencode");
    assert_eq!(doc["holder"]["status"], "started");
    assert_eq!(doc["resumed"], Value::Null);
    oc.driver("demo").detach();

    check_sandbox_wrapper(&oc, &worktree, &admin);
    assert!(oc.argv().is_empty(), "a new session: {:?}", oc.argv());
    assert_eq!(oc.cwd(), worktree);
    check_config_dir(&oc, &admin);

    assert_eq!(state_file(&admin)["agent"], "opencode");
    let listed = oc.fx.listed("demo");
    assert_eq!(listed["agent"], "opencode");
    assert_eq!(listed["holder"], true);
    assert_eq!(listed["state"], "idle");
    let table = oc.ok(&["worktree", "ls"]);
    assert!(table.contains("opencode (running)"), "{table}");

    let common = oc.fx.repo.root.join(".git");
    assert_eq!(
        fs::read_to_string(common.join("orca-term-trust.json")).unwrap(),
        r#"{"schema":1,"approved":[]}"#,
        "an approvals' file approving nothing, which the sandbox can keep the agent from writing"
    );
    let home = &oc.fx.repo.home;
    assert!(home.join(".local/share/opencode").is_dir());
    assert!(
        fs::read_dir(oc.fx.repo.xdg.join("opencode"))
            .unwrap()
            .next()
            .is_none(),
        "opencode's global config directory is made, and nothing is written in it"
    );
}

#[test]
fn events_written_to_the_pipe_give_the_states_turns_and_snapshots_claudes_hooks_give() {
    let oc = OpenCodeRepo::new();
    let wt = Started::new(&oc, "demo");
    let mut driver = oc.driver("demo");

    driver.event(&prompt("fix the login"));
    wt.wait_for_state("working", "fix the login");
    driver.event(&json!({
        "type": "tool", "session": "ses_1", "parent": null, "tool": "bash",
        "input": {"command": "npm test"}
    }));
    wt.wait_for_state("working", "Bash: npm test");
    driver.event(&json!({
        "type": "question", "session": "ses_1", "parent": null, "question": "Which file?"
    }));
    wt.wait_for_state("waiting", "Which file?");
    driver.event(&json!({"type": "replied", "session": "ses_1", "parent": null}));
    wt.wait_for_state("working", "Which file?");
    fs::write(wt.worktree.join("login.rs"), "fn login() {}\n").unwrap();
    driver.event(&idle("All done."));
    wt.wait_for_state("finished", "All done.");

    let status = wt.status();
    assert_eq!(status["session"], "ses_1");
    assert_eq!(status["agent"], "opencode");
    let turns = wt.turns();
    assert_eq!(turns.len(), 2, "{turns:?}");
    assert_eq!(turns[0]["kind"], "start");
    assert_eq!(turns[0]["agent"], "opencode");
    assert_eq!(turns[0]["prompt"], "fix the login");
    assert_eq!(turns[1]["kind"], "end");
    assert_eq!(turns[1]["outcome"], "finished");
    assert_eq!(turns[1]["end_observed"], true);
    let begin = turns[0]["begin"].as_str().unwrap();
    let end = turns[1]["end"].as_str().unwrap();
    assert_eq!(
        wt.git(&["log", "-1", "--format=%an <%ae>", end]),
        "opencode <opencode@orca-term.invalid>"
    );
    assert_eq!(
        wt.git(&["diff", "--name-only", begin, end]),
        "login.rs",
        "the turn's diff runs from its begin snapshot to its end snapshot"
    );
    driver.detach();
}

#[test]
fn an_error_fails_the_turn_and_esc_interrupts_it() {
    let oc = OpenCodeRepo::new();
    let wt = Started::new(&oc, "demo");
    let mut driver = oc.driver("demo");
    driver.event(&prompt("fix the login"));
    driver.event(&idle("All done."));
    wt.wait_for_state("finished", "All done.");

    driver.event(&prompt("now the logout"));
    wt.wait_for_state("working", "now the logout");
    driver.event(&json!({
        "type": "error", "session": "ses_1", "parent": null, "name": "APIError",
        "message": "rate limited"
    }));
    wt.wait_for_state("failed", "rate limited");
    driver.event(&idle(""));
    driver.event(&prompt("try again"));
    wt.wait_for_state("working", "try again");
    driver.event(&json!({
        "type": "error", "session": "ses_1", "parent": null, "name": "MessageAbortedError"
    }));
    wt.wait_for_state("interrupted", "try again");
    driver.detach();

    let listed: Value = serde_json::from_str(&oc.ok(&["turn", "ls", "demo", "--json"])).unwrap();
    let outcomes: Vec<&Value> = listed["turns"]
        .as_array()
        .unwrap()
        .iter()
        .map(|turn| &turn["outcome"])
        .collect();
    assert_eq!(
        outcomes,
        [&json!("finished"), &json!("failed"), &json!("interrupted")]
    );
    assert!(
        listed["turns"]
            .as_array()
            .unwrap()
            .iter()
            .all(|turn| turn["agent"] == "opencode")
    );
}

#[test]
fn esc_while_opencode_retries_its_provider_interrupts_the_turn_as_the_real_lines_say() {
    let oc = OpenCodeRepo::new();
    let wt = Started::new(&oc, "demo");
    let mut driver = oc.driver("demo");
    // The lines the plugin wrote for the real opencode 1.18.0, its provider unreachable: the
    // prompt, nothing while opencode retried the provider, then the idle line of Esc twice.
    let session = "ses_ea08cf1b5ffeXKYazd5gX3BvI5";
    driver.event(&json!({
        "type": "prompt", "session": session, "parent": null, "text": "say hi in three words"
    }));
    wt.wait_for_state("working", "say hi in three words");
    std::thread::sleep(std::time::Duration::from_millis(500));
    assert_eq!(
        wt.listed()["state"],
        "working",
        "a retried provider stays working"
    );
    driver.event(&json!({
        "type": "idle", "session": session, "parent": null, "text": null, "finish": null,
        "error": null
    }));
    wt.wait_for_state("interrupted", "say hi in three words");
    driver.detach();
    let turns = wt.turns();
    assert_eq!(turns.len(), 2, "{turns:?}");
    assert_eq!(turns[1]["outcome"], "interrupted");
    assert_eq!(turns[1]["end_observed"], false);
    assert!(turns[1]["end"].is_string(), "it still ends on a snapshot");
}

#[test]
fn a_subagents_events_make_the_turn_wait_but_never_open_or_end_one() {
    let oc = OpenCodeRepo::new();
    let wt = Started::new(&oc, "demo");
    let mut driver = oc.driver("demo");
    driver.event(&prompt("look around"));
    wt.wait_for_state("working", "look around");
    driver.event(&json!({"type": "prompt", "session": "ses_2", "parent": "ses_1", "text": "x"}));
    driver.event(&json!({
        "type": "permission", "session": "ses_2", "parent": "ses_1", "permission": "bash",
        "input": {"command": "git push"}
    }));
    wt.wait_for_state("waiting", "Bash: git push");
    driver.event(&json!({"type": "idle", "session": "ses_2", "parent": "ses_1"}));
    driver.event(&json!({"type": "replied", "session": "ses_2", "parent": "ses_1"}));
    wt.wait_for_state("working", "Bash: git push");
    driver.detach();
    assert_eq!(wt.turns().len(), 1, "only the person's turn started");
    assert_eq!(wt.status()["session"], "ses_1");
}

#[test]
fn the_events_opencode_writes_as_it_exits_are_recorded_before_the_holder_ends() {
    let oc = OpenCodeRepo::new();
    let wt = Started::new(&oc, "demo");
    let mut driver = oc.driver("demo");
    driver.event(&prompt("fix the login"));
    wt.wait_for_state("working", "fix the login");
    driver.event(&idle("Done."));
    driver.send("exit 0\n");
    driver.wait_ended();
    wait_until("the holder to end", || !holder_alive(&wt.admin));
    let listed = wt.listed();
    assert_eq!(listed["state"], "finished", "{listed}");
    assert_eq!(listed["holder"], false);
}

#[test]
fn a_cold_start_resumes_only_a_session_opencode_recorded() {
    let oc = OpenCodeRepo::new();
    let wt = Started::new(&oc, "demo");
    let mut driver = oc.driver("demo");
    driver.event(&prompt("fix the login"));
    driver.event(&idle("Done."));
    wt.wait_for_state("finished", "Done.");
    driver.detach();
    oc.ok(&["agent", "stop", "demo"]);

    let doc = json(&oc.run(&["agent", "start", "demo", "--json"]));
    assert_eq!(doc["agent"], "opencode", "the agent the worktree recorded");
    assert_eq!(doc["resumed"], "ses_1");
    oc.driver("demo").detach();
    assert_eq!(oc.argv(), ["-s", "ses_1"]);
    oc.ok(&["agent", "stop", "demo"]);

    oc.ok(&["agent", "start", "demo", "--fresh"]);
    oc.driver("demo").detach();
    assert!(oc.argv().is_empty(), "{:?}", oc.argv());
    assert_eq!(oc.starts(), 3);
}

#[test]
fn starting_another_agent_while_one_runs_is_busy_naming_it() {
    let oc = OpenCodeRepo::new();
    oc.fx.new_worktree("demo");
    oc.ok(&["agent", "start", "demo"]);
    oc.fx.wait_for_output("demo", "fake claude ready");

    let out = oc.run(&["agent", "start", "demo", "--agent", "opencode", "--json"]);
    assert_eq!(out.status.code(), Some(3), "{}", stdout(&out));
    let error = &json(&out)["error"];
    assert_eq!(error["kind"], "busy");
    let message = error["message"].as_str().unwrap();
    assert!(
        message.contains("its agent claude-code is running"),
        "{message}"
    );
    assert!(
        message.contains("orca-term agent swap demo opencode"),
        "{message}"
    );
    assert_eq!(oc.starts(), 0, "opencode never started");
    let again = json(&oc.run(&["agent", "start", "demo", "--agent", "claude-code", "--json"]));
    assert_eq!(again["holder"]["status"], "already_running");
}

#[test]
fn swap_is_busy_mid_turn_then_starts_the_other_agent_fresh_and_records_it() {
    let oc = OpenCodeRepo::new();
    let wt = Started::new(&oc, "demo");
    let mut driver = oc.driver("demo");
    driver.event(&prompt("fix the login"));
    wt.wait_for_state("working", "fix the login");

    let busy = oc.run(&["agent", "swap", "demo", "claude-code", "--json"]);
    assert_eq!(busy.status.code(), Some(3), "{}", stdout(&busy));
    let error = &json(&busy)["error"];
    assert_eq!(error["kind"], "busy");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains("its agent is in a turn"),
        "{error}"
    );
    assert!(holder_alive(&wt.admin), "opencode is left running");

    driver.event(&idle("Done."));
    wt.wait_for_state("finished", "Done.");
    driver.detach();
    let doc = json(&oc.run(&["agent", "swap", "demo", "claude-code", "--json"]));
    assert_eq!(doc["agent"], "claude-code");
    assert_eq!(doc["previous"], "opencode");
    assert_eq!(doc["stopped"], true);
    assert_eq!(
        doc["resumed"],
        Value::Null,
        "claude does not resume opencode's session"
    );
    oc.fx.wait_for_output("demo", "fake claude ready");
    assert!(
        !oc.fx.argv().contains(&"--resume".to_owned()),
        "{:?}",
        oc.fx.argv()
    );
    assert_eq!(state_file(&wt.admin)["agent"], "claude-code");
    let listed = wt.listed();
    assert_eq!(listed["agent"], "claude-code");
    assert_eq!(
        listed["state"], "idle",
        "the session shown is opencode's, not claude's"
    );
    assert_eq!(wt.turns().len(), 2, "the turns stay on the worktree");

    let text = oc.ok(&["agent", "swap", "demo", "opencode"]);
    assert!(
        text.starts_with("stopped agent claude-code in worktree demo\nstarted agent opencode"),
        "{text}"
    );
    assert!(text.contains("resuming session ses_1"), "{text}");
    oc.driver("demo").detach();
    assert_eq!(
        oc.argv(),
        ["-s", "ses_1"],
        "opencode's own session, resumed"
    );
    assert_eq!(wt.listed()["state"], "finished");
}

#[test]
fn starting_opencode_refuses_one_missing_or_too_old_and_a_missing_sandbox() {
    let oc = OpenCodeRepo::new();
    let worktree = oc.fx.new_worktree("demo");
    let admin = oc.fx.admin_dir(&worktree);

    oc.write_opencode("1.17.9", "");
    let out = oc.run(&["agent", "start", "demo", "--agent", "opencode", "--json"]);
    assert_eq!(out.status.code(), Some(9), "{}", stdout(&out));
    let error = &json(&out)["error"];
    assert_eq!(error["kind"], "prerequisite");
    assert_eq!(
        error["message"],
        "opencode 1.17.9: orca-term needs 1.18.0 or later; run orca-term doctor for the full \
         report"
    );

    oc.fx.remove_tool("opencode");
    let out = oc.run(&["agent", "start", "demo", "--agent", "opencode"]);
    assert_eq!(out.status.code(), Some(9));
    assert!(
        String::from_utf8_lossy(&out.stderr)
            .contains("opencode: not found on PATH; install opencode (https://opencode.ai)"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    oc.write_opencode("1.18.0", "");
    let tool = if cfg!(target_os = "macos") {
        "sandbox-exec"
    } else {
        "bwrap"
    };
    oc.fx.remove_tool(tool);
    let out = oc.run(&["agent", "start", "demo", "--agent", "opencode", "--json"]);
    assert_eq!(out.status.code(), Some(5), "{}", stdout(&out));
    let error = &json(&out)["error"];
    assert_eq!(error["kind"], "no_sandbox");
    assert!(
        error["message"].as_str().unwrap().starts_with(&format!(
            "opencode's sandbox is not available: {tool} not found on PATH"
        )),
        "{error}"
    );

    assert_eq!(oc.starts(), 0, "nothing was launched");
    assert!(!holder_alive(&admin));
    assert_eq!(
        state_file(&admin)["agent"],
        Value::Null,
        "no agent recorded"
    );
}

#[test]
fn swap_with_opencode_missing_leaves_the_running_agent_alone() {
    let oc = OpenCodeRepo::new();
    let worktree = oc.fx.new_worktree("demo");
    let admin = oc.fx.admin_dir(&worktree);
    oc.ok(&["agent", "start", "demo"]);
    oc.fx.wait_for_output("demo", "fake claude ready");
    oc.fx.remove_tool("opencode");

    let out = oc.run(&["agent", "swap", "demo", "opencode", "--json"]);
    assert_eq!(out.status.code(), Some(9), "{}", stdout(&out));
    assert!(holder_alive(&admin), "claude still runs");
    assert_eq!(state_file(&admin)["agent"], "claude-code");
}
