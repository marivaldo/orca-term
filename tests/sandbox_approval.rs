//! The approval of a committed `sandbox.writable` list (#58): `agent start` widens claude's sandbox
//! with extra writable paths from a committed `orca-term.yaml` only once the person approved that
//! exact list in this repository, driven as a black box against a fake `claude` on an isolated
//! PATH. Without a terminal it fails as `untrusted`; at a terminal (a real pseudo-terminal here)
//! it asks.

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

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Output;

use fixture::{AgentRepo, holder_alive};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use support::TerminalRun;

/// The list every test commits, as written.
const COMMITTED: &[&str] = &["~/cache-committed", "../shared"];

const QUESTION: &str = "widen the sandbox with these writable paths? [y/N] ";

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

/// The YAML of `sandbox.writable` for `orca-term.yaml`.
fn writable_yaml(paths: &[&str]) -> String {
    let items: Vec<String> = paths.iter().map(|path| format!("    - {path}")).collect();
    format!("sandbox:\n  writable:\n{}\n", items.join("\n"))
}

/// A repository whose committed `orca-term.yaml` holds [`COMMITTED`], with the worktree `fix`.
fn committed_repo() -> (AgentRepo, PathBuf) {
    let fx = AgentRepo::new();
    fx.repo.write("orca-term.yaml", &writable_yaml(COMMITTED));
    fx.repo.commit_all("sandbox config");
    let worktree = fx.new_worktree("fix");
    (fx, worktree)
}

/// The approvals' file, in the git common dir.
fn trust_file(fx: &AgentRepo) -> PathBuf {
    fx.repo.root.join(".git/orca-term-trust.json")
}

/// The sha256 of `text`, in lowercase hex.
fn sha256(text: &str) -> String {
    let mut hex = String::new();
    for byte in Sha256::digest(text.as_bytes()) {
        write!(hex, "{byte:02x}").unwrap();
    }
    hex
}

/// Runs `agent start` with `args` on a pseudo-terminal with the fixture's PATH, typing `answer`
/// at the question.
fn start_at_terminal(fx: &AgentRepo, args: &[&str], answer: &str) -> TerminalRun {
    let command = fx
        .repo
        .pty_command(&fx.repo.root, args)
        .env("PATH", &fx.bin)
        .env("ORCA_TERM_FAKE_CLAUDE_LOG", &fx.log);
    support::run_at_terminal(command, QUESTION, answer)
}

/// Asserts that `out` failed as `untrusted` for the sandbox with `--json`, naming `paths` from the
/// committed file of `worktree`, and that nothing started.
fn assert_untrusted_json(fx: &AgentRepo, out: &Output, worktree: &Path, paths: &[&str]) {
    assert_eq!(out.status.code(), Some(8), "{}", stderr(out));
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"]["kind"], "untrusted");
    assert_eq!(doc["error"]["what"], "sandbox");
    assert_eq!(doc["error"]["writable"], json!(paths));
    let source = worktree.join("orca-term.yaml");
    assert_eq!(doc["error"]["source"], source.display().to_string());
    assert!(doc["error"].get("script").is_none());
    assert_eq!(fx.starts(), 0, "nothing was launched");
    assert!(!holder_alive(&fx.admin_dir(worktree)));
}

#[test]
fn without_a_terminal_committed_writable_paths_start_nothing_and_fail_as_untrusted() {
    let (fx, worktree) = committed_repo();

    let out = fx.run(&["agent", "start", "fix", "--json"]);

    assert_untrusted_json(&fx, &out, &worktree, COMMITTED);
    let text = fx.run(&["agent", "start", "fix"]);
    assert_eq!(text.status.code(), Some(8));
    let message = stderr(&text);
    assert!(
        message.contains(&format!(
            "orca-term: the sandbox's extra writable paths of worktree fix (~/cache-committed, \
             ../shared), from {}, are not approved in this repository, so the agent did not start",
            worktree.join("orca-term.yaml").display()
        )),
        "{message}"
    );
    assert!(message.contains("--approve-sandbox"), "{message}");
    assert!(!message.contains("[y/N]"), "no question without a terminal");
    assert_eq!(fx.starts(), 0);
    assert!(!trust_file(&fx).exists(), "nothing is approved");
    assert_eq!(
        fx.listed("fix")["state"],
        "no_agent",
        "the worktree is untouched"
    );
}

#[test]
fn approve_sandbox_records_the_approval_and_the_settings_carry_the_paths() {
    let (fx, worktree) = committed_repo();

    let out = fx.run(&["agent", "start", "fix", "--approve-sandbox"]);

    assert!(out.status.success(), "{}", stderr(&out));
    fx.wait_for_output("fix", "fake claude ready");
    let home = fx.repo.home.display();
    let shared = worktree.parent().unwrap().join("shared");
    assert_eq!(
        fx.settings()
            .pointer("/sandbox/filesystem/allowWrite")
            .cloned(),
        Some(json!([
            format!("{home}/cache-committed"),
            shared.display().to_string()
        ]))
    );
    let trust: Value = serde_json::from_slice(&std::fs::read(trust_file(&fx)).unwrap()).unwrap();
    assert_eq!(trust["schema"], 1);
    assert_eq!(
        trust["approved"][0],
        json!({
            "kind": "sandbox",
            "sha256": sha256("~/cache-committed\n../shared"),
            "source": "orca-term.yaml",
            "approved_at": trust["approved"][0]["approved_at"],
        })
    );

    // Approved, the same list starts again (a cold start) without asking.
    fx.ok(&["agent", "stop", "fix"]);
    let out = fx.run(&["agent", "start", "fix", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(fx.starts(), 2);
}

#[test]
fn a_changed_list_asks_again() {
    let (fx, worktree) = committed_repo();
    fx.ok(&["agent", "start", "fix", "--approve-sandbox"]);
    fx.ok(&["agent", "stop", "fix"]);

    let changed = ["~/cache-committed", "../shared", "~/more"];
    std::fs::write(worktree.join("orca-term.yaml"), writable_yaml(&changed)).unwrap();
    let starts = fx.starts();
    let out = fx.run(&["agent", "start", "fix", "--json"]);

    assert_eq!(
        out.status.code(),
        Some(8),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"]["what"], "sandbox");
    assert_eq!(doc["error"]["writable"], json!(changed));
    assert_eq!(fx.starts(), starts, "nothing more was launched");
}

#[test]
fn a_list_from_the_local_override_needs_no_approval() {
    let (fx, _worktree) = committed_repo();
    fx.repo
        .write(".git/orca-term.yaml", &writable_yaml(&["~/cache-local"]));

    let out = fx.run(&["agent", "start", "fix", "--json"]);

    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["writable"]["source"], ".git/orca-term.yaml");
    assert!(!trust_file(&fx).exists(), "nothing needed approving");
}

#[test]
fn at_a_terminal_yes_records_the_approval_and_starts_the_agent() {
    let (fx, worktree) = committed_repo();

    let run = start_at_terminal(&fx, &["agent", "start", "fix"], "y");

    assert_eq!(run.code, Some(0), "{}", run.screen);
    let shown = format!(
        "the sandbox's extra writable paths of worktree fix come from {}, committed to the \
         repository, and are not approved yet:\n\n    ~/cache-committed\n    ../shared\n\n\
         {QUESTION}",
        worktree.join("orca-term.yaml").display()
    );
    assert!(run.screen.contains(&shown), "{}", run.screen);
    assert!(
        run.screen
            .contains("started agent claude-code in worktree fix"),
        "{}",
        run.screen
    );
    assert_eq!(fx.starts(), 1);
    let trust = std::fs::read_to_string(trust_file(&fx)).unwrap();
    assert!(trust.contains(r#""kind":"sandbox""#), "{trust}");
}

#[test]
fn at_a_terminal_anything_but_yes_starts_nothing() {
    let (fx, worktree) = committed_repo();

    let run = start_at_terminal(&fx, &["agent", "start", "fix"], "no");

    assert_eq!(run.code, Some(8), "{}", run.screen);
    assert!(
        run.screen.contains("are not approved in this repository"),
        "{}",
        run.screen
    );
    assert_eq!(fx.starts(), 0);
    assert!(!holder_alive(&fx.admin_dir(&worktree)));
    assert!(!trust_file(&fx).exists());
}
