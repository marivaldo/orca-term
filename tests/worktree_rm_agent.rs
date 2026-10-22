//! `worktree rm` on a worktree whose agent runs (#94), driven as a black box against a fake
//! `claude` and fake sandbox tools on an isolated PATH (seam 1): the agent is stopped as
//! `agent stop` stops it, then the worktree is removed, its unmerged branch kept. A refused
//! removal (dirty, or a held lock) never stops the agent; files the agent writes while stopping
//! refuse the removal as `dirty`, the agent staying stopped.

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
use std::path::Path;
use std::process::Output;

use fixture::{AgentRepo, holder_alive};
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

/// Starts the agent of the worktree `name` and waits until the fake claude is ready.
fn start_agent(fx: &AgentRepo, name: &str) {
    fx.ok(&["agent", "start", name]);
    fx.wait_for_output(name, "fake claude ready");
}

/// Commits a new file in `worktree`, so its branch is not merged into main.
fn commit_in(fx: &AgentRepo, worktree: &Path) {
    fs::write(worktree.join("work.txt"), "work\n").unwrap();
    let dir = worktree.to_str().unwrap();
    fx.repo.git(&["-C", dir, "add", "work.txt"]);
    fx.repo.git(&["-C", dir, "commit", "-q", "-m", "work"]);
}

fn has_branch(fx: &AgentRepo, branch: &str) -> bool {
    !fx.repo.git(&["branch", "--list", branch]).trim().is_empty()
}

#[test]
fn removing_a_worktree_whose_agent_runs_stops_it_then_removes_it_keeping_an_unmerged_branch() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    commit_in(&fx, &worktree);
    start_agent(&fx, "fix");
    assert_eq!(fx.listed("fix")["holder"], true);

    let out = fx.run(&["worktree", "rm", "fix"]);

    assert!(out.status.success(), "{}", stderr(&out));
    let report = stdout(&out);
    assert!(
        report.starts_with("stopped the agent of worktree fix\nremoved worktree fix\n"),
        "{report}"
    );
    assert!(
        report.ends_with("kept branch fix: not merged into main\n"),
        "{report}"
    );
    assert!(!worktree.exists(), "the worktree is gone");
    assert!(!holder_alive(&admin), "the holder is gone");
    assert!(has_branch(&fx, "fix"), "the unmerged branch is kept");
    let fleet: Value = serde_json::from_str(&fx.ok(&["worktree", "ls", "--json"])).unwrap();
    assert_eq!(fleet["worktrees"].as_array().unwrap().len(), 0);
}

#[test]
fn with_json_the_removal_says_whether_it_stopped_the_agent() {
    let fx = AgentRepo::new();
    let running = fx.new_worktree("fix");
    fx.new_worktree("idle");
    start_agent(&fx, "fix");

    let out = fx.run(&["worktree", "rm", "fix", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let doc = json(&out);
    assert_eq!(doc["name"], "fix");
    assert_eq!(doc["stopped_agent"], true);
    assert!(!running.exists());

    let out = fx.run(&["worktree", "rm", "idle", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(
        json(&out)["stopped_agent"],
        false,
        "no holder, nothing stopped"
    );
}

#[test]
fn a_refused_removal_never_stops_the_agent() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    start_agent(&fx, "fix");

    fs::write(worktree.join("wip.txt"), "wip\n").unwrap();
    let out = fx.run(&["worktree", "rm", "fix", "--json"]);
    assert_eq!(out.status.code(), Some(6), "{}", stdout(&out));
    let doc = json(&out);
    assert_eq!(doc["error"]["kind"], "dirty");
    let message = doc["error"]["message"].as_str().unwrap();
    assert!(!message.contains("stopped"), "{message}");
    assert!(
        holder_alive(&admin),
        "a dirty refusal leaves the agent running"
    );
    assert!(worktree.is_dir());

    fs::remove_file(worktree.join("wip.txt")).unwrap();
    let lock = File::open(admin.join("lock")).unwrap();
    lock.try_lock().unwrap();
    for args in [
        vec!["worktree", "rm", "fix"],
        vec!["worktree", "rm", "fix", "--force"],
    ] {
        let out = fx.run(&args);
        assert_eq!(out.status.code(), Some(3), "{args:?}: {}", stderr(&out));
        assert!(
            stderr(&out).contains("is busy: its setup or another orca-term command is running"),
            "{args:?}: {}",
            stderr(&out)
        );
        assert!(
            holder_alive(&admin),
            "{args:?}: a held lock leaves the agent running"
        );
    }
    drop(lock);
    assert!(worktree.is_dir());
    fx.ok(&["agent", "stop", "fix"]);
}

#[test]
fn files_the_agent_writes_while_stopping_refuse_the_removal_as_dirty_leaving_it_stopped() {
    let fx = AgentRepo::new();
    fx.write_claude(
        "2.1.296 (Claude Code)",
        "trap 'echo left > left.txt; exit 129' HUP",
    );
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    start_agent(&fx, "fix");

    let out = fx.run(&["worktree", "rm", "fix"]);

    assert_eq!(out.status.code(), Some(6), "{}", stderr(&out));
    assert!(
        stderr(&out).contains(
            "has modified or untracked files; its agent was stopped, but the worktree was kept; \
             pass --force"
        ),
        "{}",
        stderr(&out)
    );
    assert!(!holder_alive(&admin), "the agent stays stopped");
    assert!(worktree.join("left.txt").is_file());
    assert!(worktree.is_dir(), "the worktree is kept");

    let out = fx.run(&["worktree", "rm", "fix", "--force"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(
        stdout(&out).starts_with("removed worktree fix\n"),
        "nothing left to stop: {}",
        stdout(&out)
    );
}
