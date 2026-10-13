//! `orca-term agent start`, `attach` and `stop`, driven as a black box against a fake `claude` and
//! fake sandbox tools on an isolated PATH (seam 1). What a fake claude cannot prove, the sandbox
//! boundary itself, waits for the live checks (#44).

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
mod fixture;
#[expect(
    dead_code,
    reason = "the fixture is shared between suites, and this one leaves part of it unused"
)]
mod support;

use std::fs::{self, File};
use std::os::unix::process::CommandExt as _;
use std::process::{Output, Stdio};
use std::time::Instant;

use fixture::{AgentRepo, holder_alive, holder_pid, lock_held, state_file, wait_until};
use rustix::process::{Pid, Signal};
use serde_json::{Value, json};

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap()
}

/// The YAML of `sandbox.writable` for `orca-term.yaml`.
fn writable_yaml(paths: &[&str]) -> String {
    let items: Vec<String> = paths.iter().map(|path| format!("    - {path}")).collect();
    format!("sandbox:\n  writable:\n{}\n", items.join("\n"))
}

#[test]
fn start_returns_at_once_with_the_holder_detached_as_a_session_leader() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    assert!(!holder_alive(&admin));

    let started = Instant::now();
    let out = fx.run(&["agent", "start", "fix"]);

    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        started.elapsed().as_secs() < 10,
        "start returned, not waiting for the agent"
    );
    let report = stdout(&out);
    assert!(
        report.starts_with("started agent claude-code in worktree fix\n"),
        "{report}"
    );
    assert!(
        report.contains("attach: orca-term agent attach fix"),
        "{report}"
    );
    assert_eq!(
        stderr(&out),
        "sandbox: no extra writable paths (built-in default)\n"
    );
    assert!(holder_alive(&admin), "agent.lock is held by the holder");
    let pid = holder_pid(&admin);
    assert!(report.contains(&format!("holder: pid {pid}")), "{report}");
    let holder = Pid::from_raw(i32::try_from(pid).unwrap()).unwrap();
    assert_eq!(
        rustix::process::getsid(Some(holder)).unwrap(),
        holder,
        "the holder leads a session of its own"
    );
    assert_eq!(
        state_file(&admin)["agent"],
        "claude-code",
        "the agent is recorded in worktree.json"
    );
    let entry = fx.listed("fix");
    assert_eq!(entry["state"], "idle");
    assert_eq!(entry["agent"], "claude-code");
    assert_eq!(entry["holder"], true);
    let table = fx.ok(&["worktree", "ls"]);
    assert!(table.contains("idle"), "{table}");
    assert!(table.contains("claude-code (running)"), "{table}");

    fx.wait_for_output("fix", "fake claude ready");
    let again = fx.run(&["agent", "start", "fix", "--json"]);
    assert!(again.status.success(), "{}", stderr(&again));
    let doc = json(&again);
    assert_eq!(doc["holder"]["status"], "already_running");
    assert_eq!(doc["holder"]["pid"], pid);
    assert_eq!(doc["writable"], Value::Null);
    assert_eq!(fx.starts(), 1, "a worktree runs one agent process");
    let text = fx.run(&["agent", "start", "fix"]);
    assert!(stdout(&text).starts_with("agent claude-code is already running in worktree fix\n"));
}

#[test]
fn a_start_while_the_worktree_lock_is_held_is_busy() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    let lock = File::open(admin.join("lock")).unwrap();
    lock.try_lock().unwrap();

    let out = fx.run(&["agent", "start", "fix", "--json"]);

    assert_eq!(out.status.code(), Some(3));
    assert_eq!(json(&out)["error"]["kind"], "busy");
    assert!(!holder_alive(&admin));
    assert_eq!(fx.starts(), 0);
}

#[test]
fn attach_replays_the_scrollback_forwards_input_and_detaches_on_eof() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    fx.ok(&["agent", "start", "fix"]);

    let mut attach = fx.attach("fix");
    attach.wait_for("fake claude ready");
    attach.send("hi\n");
    attach.wait_for("you said: hi");
    let ended = attach.detach();

    assert!(ended.status.success());
    assert_eq!(ended.stderr, "[detached]\n");
    assert!(holder_alive(&admin), "detaching leaves the agent running");

    let replay = fx.wait_for_output("fix", "you said: hi");
    assert!(replay.contains("fake claude ready"), "{replay}");
}

#[test]
fn the_agent_survives_a_killed_attach_and_a_reattach_replays_everything_once_in_order() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");
    fx.ok(&["agent", "start", "fix"]);

    let mut first = fx.attach("fix");
    first.wait_for("fake claude ready");
    first.send("one\n");
    first.wait_for("you said: one");
    first.kill_with(Signal::HUP);
    assert!(!first.wait_ended().status.success());

    let mut second = fx.attach("fix");
    second.wait_for("you said: one");
    second.send("two\n");
    second.wait_for("you said: two");
    second.kill_with(Signal::KILL);
    second.wait_ended();

    let third = fx.wait_for_output("fix", "you said: two");
    let ready = third.find("fake claude ready").unwrap();
    let one = third.find("you said: one").unwrap();
    let two = third.find("you said: two").unwrap();
    assert!(ready < one && one < two, "in order:\n{third}");
    assert_eq!(
        third.matches("you said: one").count(),
        1,
        "no duplicate:\n{third}"
    );
    assert_eq!(
        third.matches("fake claude ready").count(),
        1,
        "no duplicate:\n{third}"
    );
}

#[test]
fn the_holder_survives_a_group_kill_of_its_starter() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);

    // As Neovim runs jobs: in a process group of its own, which it kills as a whole afterwards.
    let mut starter = fx.command(&["agent", "start", "fix"]);
    starter
        .process_group(0)
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let starter = starter.spawn().unwrap();
    let group = Pid::from_raw(i32::try_from(starter.id()).unwrap()).unwrap();
    let mut starter = starter;
    assert!(starter.wait().unwrap().success());
    let _ = rustix::process::kill_process_group(group, Signal::TERM);
    let _ = rustix::process::kill_process_group(group, Signal::HUP);

    assert!(
        holder_alive(&admin),
        "the holder is outside the starter's group"
    );
    let output = fx.wait_for_output("fix", "fake claude ready");
    assert!(output.contains("fake claude ready"));
}

#[test]
fn an_agent_that_exits_on_its_own_ends_the_holder_and_tells_the_attached_client() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    fx.ok(&["agent", "start", "fix"]);

    let mut attach = fx.attach("fix");
    attach.wait_for("fake claude ready");
    attach.send("exit 7\n");
    let ended = attach.wait_ended();

    assert!(ended.status.success());
    assert!(ended.stdout.contains("bye"), "{}", ended.stdout);
    assert_eq!(ended.stderr, "[agent ended: exit 7]\n");
    wait_until("the holder ending", || !holder_alive(&admin));
    wait_until("the socket and record going", || {
        !admin.join("agent.sock").exists() && !admin.join("agent.json").exists()
    });
    assert_eq!(state_file(&admin)["agent"], "claude-code");
    let entry = fx.listed("fix");
    assert_eq!(
        entry["state"], "idle",
        "an agent that ended is still the worktree's agent"
    );
    assert_eq!(entry["holder"], false);
    let table = fx.ok(&["worktree", "ls"]);
    assert!(table.contains("claude-code (stopped)"), "{table}");
}

#[test]
fn stop_ends_the_agent_and_is_idempotent() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    fx.ok(&["agent", "start", "fix"]);
    fx.wait_for_output("fix", "fake claude ready");

    let out = fx.run(&["agent", "stop", "fix"]);

    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stdout(&out), "stopped the agent of worktree fix\n");
    assert!(!holder_alive(&admin));
    assert!(!admin.join("agent.sock").exists());
    assert_eq!(fx.listed("fix")["holder"], false);

    let again = fx.run(&["agent", "stop", "fix"]);
    assert!(again.status.success());
    assert_eq!(stdout(&again), "no agent is running in worktree fix\n");
    let doc = json(&fx.run(&["agent", "stop", "fix", "--json"]));
    assert_eq!(doc["status"], "not_running");
    assert_eq!(doc["worktree"]["agent"], "claude-code");

    // Started again afterwards: a fresh holder, a fresh fake claude.
    fx.ok(&["agent", "start", "fix"]);
    fx.wait_for_output("fix", "fake claude ready");
    assert_eq!(fx.starts(), 2);
}

#[test]
fn stop_escalates_to_a_kill_for_an_agent_that_ignores_hangup_and_terminate() {
    let fx = AgentRepo::new();
    fx.write_claude("2.1.296 (Claude Code)", "trap '' HUP TERM");
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    fx.ok(&["agent", "start", "fix"]);
    fx.wait_for_output("fix", "fake claude ready");

    let started = Instant::now();
    let out = fx.run(&["agent", "stop", "fix"]);

    assert!(out.status.success(), "{}", stderr(&out));
    let took = started.elapsed();
    assert!(
        took.as_secs_f64() >= 5.5,
        "hang-up, terminate, kill: {took:?}"
    );
    assert!(took.as_secs() < 10, "{took:?}");
    assert!(!holder_alive(&admin));
}

#[test]
fn without_the_sandbox_tools_the_agent_refuses_to_start() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    for tool in ["sandbox-exec", "bwrap", "socat"] {
        fx.remove_tool(tool);
    }

    let out = fx.run(&["agent", "start", "fix", "--json"]);

    assert_eq!(out.status.code(), Some(5), "{}", stdout(&out));
    let doc = json(&out);
    assert_eq!(doc["contract"], 1);
    assert_eq!(doc["error"]["kind"], "no_sandbox");
    let message = doc["error"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("claude's sandbox is not available: "),
        "{message}"
    );
    assert!(message.contains("not found on PATH"), "{message}");
    let text = fx.run(&["agent", "start", "fix"]);
    assert_eq!(text.status.code(), Some(5));
    assert!(stderr(&text).starts_with("orca-term: claude's sandbox is not available"));
    assert_eq!(fx.starts(), 0, "nothing was launched");
    assert!(!holder_alive(&admin));
    assert_eq!(state_file(&admin)["agent"], Value::Null);
    assert_eq!(fx.listed("fix")["state"], "no_agent");
}

#[test]
fn the_launch_carries_the_settings_file_accept_edits_and_no_forbidden_flag() {
    let fx = AgentRepo::new();
    fx.repo
        .write("orca-term.yaml", &writable_yaml(&["~/cache-committed"]));
    fx.repo.commit_all("sandbox config");
    let fix = fx.new_worktree("fix");
    let docs = fx.new_worktree("docs");
    fx.repo.write(
        ".git/orca-term.yaml",
        &writable_yaml(&["~/cache-local", "/opt/shared"]),
    );
    let common = fx.repo.root.join(".git").canonicalize().unwrap();

    let out = fx.run(&["agent", "start", "fix"]);

    assert!(out.status.success(), "{}", stderr(&out));
    let home = fx.repo.home.display();
    assert_eq!(
        stderr(&out),
        format!(
            "sandbox: extra writable paths {home}/cache-local, /opt/shared (from \
             .git/orca-term.yaml, overriding orca-term.yaml)\n"
        )
    );
    fx.wait_for_output("fix", "fake claude ready");

    let argv = fx.argv();
    let admin = fx.admin_dir(&fix).canonicalize().unwrap();
    let settings_file = admin.join("claude-settings.json");
    assert_eq!(
        argv,
        [
            "--settings".to_owned(),
            settings_file.display().to_string(),
            "--permission-mode".to_owned(),
            "acceptEdits".to_owned(),
        ]
    );
    let cwd = fs::read_to_string(fx.log.with_extension("cwd")).unwrap();
    assert_eq!(cwd.trim(), fix.display().to_string());

    let settings = fx.settings();
    assert_sandbox_policy(&settings);
    let at = |pointer: &str| settings.pointer(pointer).cloned().unwrap_or(Value::Null);
    assert_eq!(
        at("/sandbox/filesystem/allowWrite"),
        json!([format!("{home}/cache-local"), "/opt/shared"]),
        "the highest layer's list, replacing the committed one"
    );
    let common_text = common.display();
    let deny_write = at("/sandbox/filesystem/denyWrite");
    let deny_write = deny_write.as_array().unwrap();
    for worktree in [&fix, &docs] {
        let admin = fx.admin_dir(worktree).canonicalize().unwrap();
        assert!(
            deny_write.contains(&json!(admin.display().to_string())),
            "{admin:?} in {deny_write:?}"
        );
    }
    for expected in [
        format!("{common_text}/worktrees/*/orca-term"),
        format!("{common_text}/refs/orca-term"),
        format!("{common_text}/orca-term.yaml"),
    ] {
        assert!(
            deny_write.contains(&json!(expected)),
            "{expected} in {deny_write:?}"
        );
    }
    assert_eq!(
        at("/permissions/deny"),
        json!([
            format!("Edit(/{common_text}/worktrees/*/orca-term/**)"),
            format!("Edit(/{common_text}/refs/orca-term/**)"),
            format!("Edit(/{common_text}/orca-term.yaml)"),
        ])
    );
    assert!(
        common.join("refs/orca-term").is_dir(),
        "pre-created before the launch"
    );
}

/// The sandbox policy of ADR 0007, key by key: on, failing closed, network open, every weakening
/// key pinned, no bypass mode anywhere.
fn assert_sandbox_policy(settings: &Value) {
    let at = |pointer: &str| settings.pointer(pointer).cloned().unwrap_or(Value::Null);
    assert_eq!(at("/sandbox/enabled"), json!(true));
    assert_eq!(at("/sandbox/failIfUnavailable"), json!(true));
    assert_eq!(at("/sandbox/autoAllowBashIfSandboxed"), json!(true));
    assert_eq!(at("/sandbox/allowUnsandboxedCommands"), json!(false));
    assert_eq!(at("/sandbox/enableWeakerNestedSandbox"), json!(false));
    assert_eq!(at("/sandbox/allowAppleEvents"), json!(false));
    assert_eq!(at("/sandbox/filesystem/disabled"), json!(false));
    assert_eq!(at("/sandbox/network/allowedDomains"), json!(["*"]));
    assert_eq!(at("/sandbox/network/allowAllUnixSockets"), json!(false));
    assert_eq!(
        at("/permissions/disableBypassPermissionsMode"),
        json!("disable")
    );
    assert_eq!(at("/permissions/disableAutoMode"), json!("disable"));
    assert_eq!(at("/permissions/defaultMode"), Value::Null);
    let text = serde_json::to_string(settings).unwrap();
    for forbidden in [
        "--dangerously-skip-permissions",
        "--allow-dangerously-skip-permissions",
        "--auto",
        "bypassPermissions",
    ] {
        assert!(
            !text.contains(&format!("\"{forbidden}\"")),
            "{forbidden} in {text}"
        );
    }
}

#[test]
fn no_argument_of_the_launch_bypasses_permissions() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");
    fx.ok(&["agent", "start", "fix"]);
    fx.wait_for_output("fix", "fake claude ready");

    let argv = fx.argv();
    for forbidden in [
        "--dangerously-skip-permissions",
        "--allow-dangerously-skip-permissions",
        "--auto",
        "bypassPermissions",
    ] {
        assert!(
            !argv.iter().any(|arg| arg == forbidden),
            "{forbidden} in {argv:?}"
        );
    }
    let mode = argv
        .iter()
        .position(|arg| arg == "--permission-mode")
        .unwrap();
    assert_eq!(argv[mode + 1], "acceptEdits");
    assert_sandbox_policy(&fx.settings());
}

#[test]
fn start_json_reports_the_holder_and_the_writable_paths() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");
    fx.repo.write(
        ".git/orca-term.yaml",
        &writable_yaml(&["~/cache-local", "/opt/shared"]),
    );
    let home = fx.repo.home.display();

    let doc = json(&fx.run(&["agent", "start", "fix", "--json"]));

    assert_eq!(doc["contract"], 1);
    assert_eq!(doc["holder"]["status"], "started");
    assert!(doc["holder"]["pid"].is_u64());
    assert_eq!(doc["agent"], "claude-code");
    assert_eq!(
        doc["writable"]["paths"],
        json!([format!("{home}/cache-local"), "/opt/shared"])
    );
    assert_eq!(doc["writable"]["source"], ".git/orca-term.yaml");
    assert_eq!(doc["worktree"]["state"], "idle");
    assert_eq!(doc["worktree"]["agent"], "claude-code");
    assert_eq!(doc["worktree"]["holder"], true);
    let doc = json(&fx.run(&["agent", "start", "fix", "--json"]));
    assert_eq!(doc["holder"]["status"], "already_running");
}

#[test]
fn a_socket_path_longer_than_a_socket_address_holds_still_works() {
    let fx = AgentRepo::new();
    let name = "a-rather-long-worktree-name-that-pushes-the-socket-path-past-the-address-limit";
    let worktree = fx.new_worktree(name);
    let admin = fx.admin_dir(&worktree);
    assert!(
        admin.join("agent.sock").as_os_str().len() > 104,
        "{} is not longer than a unix socket address",
        admin.display()
    );

    fx.ok(&["agent", "start", name]);

    assert!(holder_alive(&admin));
    let output = fx.wait_for_output(name, "fake claude ready");
    assert!(output.contains("fake claude ready"));
    assert_eq!(state_file(&admin)["agent"], "claude-code");
}

#[test]
fn rm_and_setup_are_busy_while_the_agent_runs_and_the_recorded_agent_survives_setup() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    fx.ok(&["agent", "start", "fix"]);
    fx.wait_for_output("fix", "fake claude ready");

    for args in [
        vec!["worktree", "rm", "fix"],
        vec!["worktree", "rm", "fix", "--force"],
        vec!["worktree", "setup", "fix"],
    ] {
        let out = fx.run(&args);
        assert_eq!(out.status.code(), Some(3), "{args:?}: {}", stderr(&out));
        assert!(
            stderr(&out).contains(
                "is busy: its agent is running; stop it first with `orca-term agent stop fix`"
            ),
            "{args:?}: {}",
            stderr(&out)
        );
    }
    let doc = json(&fx.run(&["worktree", "setup", "fix", "--json"]));
    assert_eq!(doc["error"]["kind"], "busy");
    assert!(worktree.is_dir(), "the worktree stays");
    assert!(holder_alive(&admin));

    fx.ok(&["agent", "stop", "fix"]);
    fx.repo
        .write(".git/orca-term.yaml", "scripts:\n  setup: echo set up\n");
    let out = fx.run(&["worktree", "setup", "fix", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["setup"]["status"], "succeeded");
    assert_eq!(doc["worktree"]["state"], "idle");
    assert_eq!(doc["worktree"]["agent"], "claude-code");
    let state = state_file(&admin);
    assert_eq!(
        state["agent"], "claude-code",
        "setup keeps the agent: {state}"
    );
    assert_eq!(state["setup"]["status"], "succeeded");
    assert_eq!(fx.listed("fix")["state"], "idle");

    let out = fx.run(&["worktree", "rm", "fix", "--force"]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn the_agents_environment_loses_only_the_claude_code_session_variables() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");

    let out = fx
        .command(&["agent", "start", "fix"])
        .env("CLAUDECODE", "1")
        .env("CLAUDE_CODE_ENTRYPOINT", "cli")
        .env("CLAUDE_CODE_SESSION_ID", "abc")
        .env("CLAUDE_CODE_CHILD_SESSION", "1")
        .env("ORCA_TERM_TEST_KEPT", "1")
        .env("ANTHROPIC_SOMETHING", "kept")
        .output()
        .unwrap();

    assert!(out.status.success(), "{}", stderr(&out));
    fx.wait_for_output("fix", "fake claude ready");
    let names: Vec<String> = fs::read_to_string(fx.log.with_extension("env"))
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect();
    for stripped in [
        "CLAUDECODE",
        "CLAUDE_CODE_ENTRYPOINT",
        "CLAUDE_CODE_SESSION_ID",
        "CLAUDE_CODE_CHILD_SESSION",
    ] {
        assert!(
            !names.iter().any(|n| n == stripped),
            "{stripped} in {names:?}"
        );
    }
    for kept in ["ORCA_TERM_TEST_KEPT", "ANTHROPIC_SOMETHING", "HOME", "PATH"] {
        assert!(names.iter().any(|n| n == kept), "{kept} not in {names:?}");
    }
}

#[test]
fn start_refuses_a_worktree_that_is_not_ready_and_a_claude_that_is_old_or_missing() {
    let fx = AgentRepo::new();
    fx.repo
        .write("orca-term.yaml", "scripts:\n  setup: exit 3\n");
    fx.repo.commit_all("failing setup");
    let out = fx.run(&["worktree", "new", "fix"]);
    assert_eq!(out.status.code(), Some(1));
    let worktree = fx.repo.home.join("orca-term/worktrees/repo/fix");
    let admin = fx.admin_dir(&worktree);

    let out = fx.run(&["agent", "start", "fix"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("cannot start an agent in worktree fix")
            && stderr(&out).contains("it is not ready (setup failed (exit 3))"),
        "{}",
        stderr(&out)
    );

    fx.repo
        .write(".git/orca-term.yaml", "scripts:\n  setup: ''\n");
    fx.ok(&["worktree", "setup", "fix"]);
    fx.write_claude("2.1.284 (Claude Code)", "");
    let out = fx.run(&["agent", "start", "fix"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("claude 2.1.284 is too old: orca-term needs 2.1.285 or newer"),
        "{}",
        stderr(&out)
    );

    fx.remove_tool("claude");
    let out = fx.run(&["agent", "start", "fix"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        stderr(&out).contains("claude is not on PATH"),
        "{}",
        stderr(&out)
    );
    assert!(!holder_alive(&admin));
    assert_eq!(state_file(&admin)["agent"], Value::Null);

    let primary = fx.run(&["agent", "start", "."]);
    assert_eq!(primary.status.code(), Some(1));
    assert!(
        stderr(&primary).contains("is the primary checkout"),
        "{}",
        stderr(&primary)
    );
}

#[test]
fn attach_and_stop_without_a_running_agent_say_so() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");

    let out = fx.run(&["agent", "attach", "fix"]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(
        stderr(&out),
        "orca-term: no agent is running in worktree fix; start one with `orca-term agent start fix`\n"
    );

    let out = fx.run(&["agent", "stop", "fix"]);
    assert!(out.status.success());
    assert_eq!(stdout(&out), "no agent is running in worktree fix\n");
    assert_eq!(fx.listed("fix")["state"], "no_agent");
}

#[test]
fn an_agent_that_exits_at_once_is_reported_by_start_and_not_recorded() {
    let fx = AgentRepo::new();
    fx.write_claude(
        "2.1.296 (Claude Code)",
        "echo 'Error: invalid settings'; exit 1",
    );
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);

    let out = fx.run(&["agent", "start", "fix", "--json"]);

    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let doc = json(&out);
    assert_eq!(doc["error"]["kind"], "other");
    let message = doc["error"]["message"].as_str().unwrap();
    assert!(
        message.contains("exited at once (exit 1): Error: invalid settings"),
        "{message}"
    );
    assert!(!holder_alive(&admin));
    assert!(
        !lock_held(&admin.join("lock")),
        "the worktree lock is released"
    );
    assert!(!admin.join("agent.sock").exists());
    assert_eq!(state_file(&admin)["agent"], Value::Null);
    assert_eq!(fx.listed("fix")["state"], "no_agent");
}
