//! `worktree new --agent` (#93), driven as a black box against a fake `claude`, a fake `opencode`
//! and fake sandbox tools on an isolated PATH (seam 1): the agent is recorded and started in its
//! holder once the setup succeeded, with `agent start`'s refusals (no sandbox, untrusted,
//! prerequisite) and exit codes. A failed setup still records the agent but starts nothing, and
//! the `agent start` after a later successful `worktree setup` starts it (#119). Without `--agent`
//! nothing changes.

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

use std::path::PathBuf;
use std::process::Output;

use fixture::{AgentRepo, holder_alive, state_file};
use opencode::OpenCodeRepo;
use serde_json::Value;

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout).unwrap_or_else(|_| panic!("not JSON: {}", stdout(out)))
}

/// Where the fixture's worktree `name` is made.
fn worktree_path(fx: &AgentRepo, name: &str) -> PathBuf {
    fx.repo.home.join("orca-term/worktrees/repo").join(name)
}

/// Checks that `worktree new` made the worktree `name`, ready, recorded `agent` as its agent and
/// left no holder alive: what a refused start leaves.
fn assert_ready_with_agent_not_started(fx: &AgentRepo, name: &str, agent: &str) {
    let admin = fx.admin_dir(&worktree_path(fx, name));
    assert!(!holder_alive(&admin));
    assert_eq!(state_file(&admin)["agent"], agent);
    let listed = fx.listed(name);
    assert_eq!(listed["agent"], agent, "{listed}");
    assert_eq!(listed["holder"], false, "{listed}");
    assert_ne!(listed["state"], "not_ready", "{listed}");
}

#[test]
fn with_agent_claude_code_the_new_worktree_records_and_starts_claude() {
    let fx = AgentRepo::new();

    let out = fx.run(&[
        "worktree",
        "new",
        "fix",
        "--agent",
        "claude-code",
        "--rows",
        "30",
        "--cols",
        "100",
        "--json",
    ]);

    assert!(out.status.success(), "{}{}", stdout(&out), stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["contract"], 1);
    assert_eq!(doc["worktree"]["name"], "fix");
    assert_eq!(doc["setup"]["status"], "none");
    assert_eq!(doc["started"]["agent"], "claude-code");
    assert_eq!(doc["started"]["holder"]["status"], "started");
    assert_eq!(doc["started"]["worktree"]["holder"], true);
    let worktree = worktree_path(&fx, "fix").canonicalize().unwrap();
    let admin = fx.admin_dir(&worktree);
    assert!(holder_alive(&admin));
    assert_eq!(state_file(&admin)["agent"], "claude-code");
    assert_eq!(fx.starts(), 1);
    let listed = fx.listed("fix");
    assert_eq!(listed["agent"], "claude-code");
    assert_eq!(listed["holder"], true);
    assert!(
        fx.wait_for_output("fix", "fake claude ready")
            .contains("fake claude ready")
    );
}

#[test]
fn as_text_it_prints_what_it_made_then_the_agent_it_started() {
    let fx = AgentRepo::new();

    let text = fx.ok(&["worktree", "new", "fix", "--agent", "claude-code"]);

    let created = text.find("created worktree fix").unwrap();
    let started = text
        .find("started agent claude-code in worktree fix")
        .unwrap_or_else(|| panic!("{text}"));
    assert!(created < started, "{text}");
}

#[test]
fn with_agent_opencode_the_new_worktree_records_and_starts_opencode() {
    let oc = OpenCodeRepo::new();

    let doc: Value =
        serde_json::from_str(&oc.ok(&["worktree", "new", "fix", "--agent", "opencode", "--json"]))
            .unwrap();

    assert_eq!(doc["started"]["agent"], "opencode");
    assert_eq!(doc["started"]["holder"]["status"], "started");
    let worktree = worktree_path(&oc.fx, "fix").canonicalize().unwrap();
    let admin = oc.fx.admin_dir(&worktree);
    assert!(holder_alive(&admin));
    assert_eq!(state_file(&admin)["agent"], "opencode");
    let listed = oc.fx.listed("fix");
    assert_eq!(listed["agent"], "opencode");
    assert_eq!(listed["holder"], true);
    assert_eq!(oc.fx.starts(), 0, "claude must not start");
}

#[test]
fn a_failed_setup_records_the_agent_but_starts_nothing_until_a_later_setup_succeeds() {
    let oc = OpenCodeRepo::new();
    let fx = &oc.fx;
    fx.repo.write(
        "orca-term.yaml",
        "scripts:\n  setup: \"if [ ! -f ready.txt ]; then echo 'missing ready.txt'; exit 3; fi\"\n",
    );
    fx.repo.commit_all("setup that needs ready.txt");

    let out = oc.run(&[
        "worktree",
        "new",
        "fix",
        "--approve-setup",
        "--agent",
        "opencode",
        "--json",
    ]);

    assert_eq!(out.status.code(), Some(1), "{}", stdout(&out));
    let doc = json(&out);
    assert_eq!(doc["setup"]["status"], "failed");
    assert!(doc.get("started").is_none(), "{doc}");
    assert_eq!(doc["worktree"]["agent"], "opencode", "{doc}");
    let worktree = worktree_path(fx, "fix");
    let admin = fx.admin_dir(&worktree);
    assert!(!holder_alive(&admin));
    assert_eq!(state_file(&admin)["agent"], "opencode");
    assert_eq!(oc.starts(), 0);
    let listed = fx.listed("fix");
    assert_eq!(listed["state"], "not_ready", "{listed}");
    assert_eq!(listed["agent"], "opencode", "{listed}");
    assert_eq!(listed["holder"], false);

    // Fixed and set up again: `worktree setup` itself starts nothing.
    std::fs::write(worktree.join("ready.txt"), "").unwrap();
    let set_up: Value =
        serde_json::from_str(&oc.ok(&["worktree", "setup", "fix", "--json"])).unwrap();
    assert_eq!(set_up["setup"]["status"], "succeeded");
    assert_eq!(set_up["worktree"]["agent"], "opencode", "{set_up}");
    assert!(!holder_alive(&admin));
    assert_eq!(oc.starts(), 0);

    // `agent start` with no `--agent` starts the agent chosen at creation, not claude-code.
    let started: Value =
        serde_json::from_str(&oc.ok(&["agent", "start", "fix", "--json"])).unwrap();
    assert_eq!(started["agent"], "opencode");
    assert_eq!(started["holder"]["status"], "started");
    assert!(holder_alive(&admin));
    assert_eq!(fx.starts(), 0, "claude must not start");

    let text = oc.run(&[
        "worktree",
        "new",
        "other",
        "--approve-setup",
        "--agent",
        "opencode",
    ]);
    assert_eq!(text.status.code(), Some(1));
    assert!(!stdout(&text).contains("started"), "{}", stdout(&text));
}

#[test]
fn a_setup_script_not_approved_still_records_the_agent() {
    let oc = OpenCodeRepo::new();
    oc.fx
        .repo
        .write("orca-term.yaml", "scripts:\n  setup: echo setting up\n");
    oc.fx.repo.commit_all("setup script");

    let out = oc.run(&["worktree", "new", "fix", "--agent", "opencode", "--json"]);

    assert_eq!(out.status.code(), Some(8), "{}", stdout(&out));
    assert_eq!(json(&out)["error"]["kind"], "untrusted");
    let listed = oc.fx.listed("fix");
    assert_eq!(listed["state"], "not_ready", "{listed}");
    assert_eq!(listed["agent"], "opencode", "{listed}");
    assert_eq!(listed["holder"], false);
    assert_eq!(oc.starts(), 0);
}

#[test]
fn without_the_sandbox_tools_the_start_is_refused_but_the_worktree_is_ready_and_records_it() {
    let fx = AgentRepo::new();
    for tool in ["sandbox-exec", "bwrap", "socat"] {
        fx.remove_tool(tool);
    }

    let out = fx.run(&["worktree", "new", "fix", "--agent", "claude-code", "--json"]);

    assert_eq!(out.status.code(), Some(5), "{}", stdout(&out));
    assert_eq!(json(&out)["error"]["kind"], "no_sandbox");
    assert_ready_with_agent_not_started(&fx, "fix", "claude-code");

    let text = fx.run(&["worktree", "new", "other", "--agent", "claude-code"]);
    assert_eq!(text.status.code(), Some(5));
    assert!(
        stdout(&text).contains("created worktree other"),
        "{}",
        stdout(&text)
    );
    assert!(
        stderr(&text).contains("orca-term: claude's sandbox is not available"),
        "{}",
        stderr(&text)
    );
}

#[test]
fn a_missing_agent_is_refused_as_prerequisite_but_the_worktree_is_ready_and_records_it() {
    let fx = AgentRepo::new();
    fx.remove_tool("claude");

    let out = fx.run(&["worktree", "new", "fix", "--agent", "claude-code", "--json"]);

    assert_eq!(out.status.code(), Some(9), "{}", stdout(&out));
    assert_eq!(json(&out)["error"]["kind"], "prerequisite");
    assert_ready_with_agent_not_started(&fx, "fix", "claude-code");

    let oc = OpenCodeRepo::new();
    oc.fx.remove_tool("opencode");
    let out = oc.run(&["worktree", "new", "fix", "--agent", "opencode", "--json"]);
    assert_eq!(out.status.code(), Some(9), "{}", stdout(&out));
    assert_eq!(json(&out)["error"]["kind"], "prerequisite");
    assert_ready_with_agent_not_started(&oc.fx, "fix", "opencode");
}

#[test]
fn unapproved_writable_paths_are_refused_as_untrusted_but_the_worktree_is_ready_and_records_it() {
    let fx = AgentRepo::new();
    fx.repo
        .write("orca-term.yaml", "sandbox:\n  writable:\n    - ../shared\n");
    fx.repo.commit_all("sandbox config");

    let out = fx.run(&["worktree", "new", "fix", "--agent", "claude-code", "--json"]);

    assert_eq!(out.status.code(), Some(8), "{}", stdout(&out));
    let doc = json(&out);
    assert_eq!(doc["error"]["kind"], "untrusted");
    assert_eq!(doc["error"]["what"], "sandbox");
    assert_ready_with_agent_not_started(&fx, "fix", "claude-code");

    let started = fx.ok(&["agent", "start", "fix", "--approve-sandbox", "--json"]);
    let started: Value = serde_json::from_str(&started).unwrap();
    assert_eq!(started["agent"], "claude-code");
    assert_eq!(started["holder"]["status"], "started");
}

#[test]
fn without_agent_nothing_changes_and_the_size_needs_an_agent() {
    let fx = AgentRepo::new();

    let doc: Value = serde_json::from_str(&fx.ok(&["worktree", "new", "fix", "--json"])).unwrap();

    assert!(doc.get("started").is_none(), "{doc}");
    assert_eq!(doc["worktree"]["agent"], Value::Null);
    assert_eq!(fx.starts(), 0);
    let admin = fx.admin_dir(&worktree_path(&fx, "fix"));
    assert!(!holder_alive(&admin));

    let out = fx.run(&["worktree", "new", "other", "--rows", "30"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
    let out = fx.run(&["worktree", "new", "other", "--agent", "codex"]);
    assert_eq!(out.status.code(), Some(2), "{}", stderr(&out));
}
