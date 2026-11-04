//! `orca-term worktree push`, driven as a black box (seam 1) with real git, a bare repository as
//! `origin` and another clone of it that pushes too (#146): a first push setting the upstream, a
//! branch already up to date, new commits, a branch that diverged after `worktree rebase`, refused
//! and then pushed with a lease, the lease refusing after someone else pushed, each refusal, the
//! files left out, and the pull request read again after a push.

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

use fixture::AgentRepo;
use serde_json::{Value, json};

/// A fake `gh` whose `gh pr view` answers pull request 7, open, with no checks, for any branch.
/// Shell builtins only: its PATH holds nothing else.
const FAKE_GH: &str = r#"#!/bin/sh
printf '{"number":7,"title":"Demo","state":"OPEN","isDraft":false,"url":"https://github.com/o/r/pull/7","statusCheckRollup":[],"headRefOid":"x"}\n'
"#;

/// A repository with `main` on a bare repository as `origin`, another clone of `origin` that
/// pushes too, and worktrees placed under the fixture's own directory.
struct PushRepo {
    fixture: AgentRepo,
    origin: PathBuf,
    /// Another clone of `origin`: someone else pushing.
    other: PathBuf,
    base: PathBuf,
}

impl PushRepo {
    fn new() -> Self {
        let fixture = AgentRepo::new();
        fixture.repo.write("shared.txt", "one\n");
        fixture.repo.commit_all("shared");
        let tmp = fixture.repo.tmp.path().canonicalize().unwrap();
        let origin = tmp.join("origin.git");
        let origin_text = origin.to_str().unwrap();
        fixture
            .repo
            .git(&["init", "--quiet", "--bare", origin_text]);
        fixture.repo.git(&["remote", "add", "origin", origin_text]);
        fixture.repo.git(&["push", "--quiet", "origin", "main"]);
        let other = tmp.join("other");
        fixture
            .repo
            .git(&["clone", "--quiet", origin_text, other.to_str().unwrap()]);
        let base = tmp.join("worktrees");
        fs::write(
            fixture.repo.root.join(".git/orca-term.yaml"),
            format!("base: {}\n", base.display()),
        )
        .unwrap();
        Self {
            fixture,
            origin,
            other,
            base,
        }
    }

    /// Creates the worktree `name` and returns its path.
    fn new_worktree(&self, name: &str) -> PathBuf {
        self.fixture.ok(&["worktree", "new", name]);
        self.base.join("repo").join(name).canonicalize().unwrap()
    }

    /// Runs git in `dir` and returns its stdout, trimmed.
    fn git_in(&self, dir: &Path, args: &[&str]) -> String {
        let mut all = vec!["-C", dir.to_str().unwrap()];
        all.extend_from_slice(args);
        self.fixture.repo.git(&all).trim().to_owned()
    }

    /// Writes `contents` to `file` in `dir` and commits it with `message`.
    fn commit_in(&self, dir: &Path, file: &str, contents: &str, message: &str) {
        fs::write(dir.join(file), contents).unwrap();
        self.git_in(dir, &["add", file]);
        self.git_in(dir, &["commit", "--quiet", "-m", message]);
    }

    /// The commit `origin` holds for `branch`, or an empty string when it has no such branch.
    fn on_origin(&self, branch: &str) -> String {
        let full = format!("refs/heads/{branch}");
        self.git_in(
            &self.origin,
            &["for-each-ref", "--format=%(objectname)", &full],
        )
    }

    fn head(&self, dir: &Path) -> String {
        self.git_in(dir, &["rev-parse", "HEAD"])
    }

    fn run(&self, args: &[&str]) -> Output {
        self.fixture.run(args)
    }

    fn ok(&self, args: &[&str]) -> String {
        self.fixture.ok(args)
    }

    /// Runs the binary with `args` and `--json`, failing unless it succeeded, and parses what it
    /// printed.
    fn json_of(&self, args: &[&str]) -> Value {
        let mut all = args.to_vec();
        all.push("--json");
        serde_json::from_str(&self.ok(&all)).unwrap()
    }

    /// Runs the binary with `args` and `--json`, expecting it to fail with `code`, and returns the
    /// error object it printed.
    fn error_of(&self, args: &[&str], code: i32) -> Value {
        let mut all = args.to_vec();
        all.push("--json");
        let out = self.run(&all);
        assert_eq!(out.status.code(), Some(code), "{out:?}");
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        doc["error"].clone()
    }

    /// Pushes `demo` once, so `origin/demo` exists, and returns the worktree.
    fn pushed_demo(&self) -> PathBuf {
        let demo = self.new_worktree("demo");
        self.commit_in(&demo, "a.txt", "a\n", "Add a");
        self.ok(&["worktree", "push", "demo"]);
        demo
    }
}

#[test]
fn a_first_push_sends_the_branch_and_sets_its_upstream() {
    let repo = PushRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit_in(&demo, "a.txt", "a\n", "Add a");

    let text = repo.ok(&["worktree", "push", "demo"]);

    assert_eq!(text, "demo: pushed to origin/demo\n");
    assert_eq!(repo.on_origin("demo"), repo.head(&demo));
    assert_eq!(
        repo.git_in(&demo, &["rev-parse", "--abbrev-ref", "demo@{upstream}"]),
        "origin/demo"
    );
}

#[test]
fn a_branch_origin_already_has_is_up_to_date() {
    let repo = PushRepo::new();
    let demo = repo.pushed_demo();

    assert_eq!(
        repo.ok(&["worktree", "push", "demo"]),
        "demo: origin/demo is already up to date\n"
    );
    let doc = repo.json_of(&["worktree", "push", "demo"]);
    assert_eq!(doc["outcome"], "up_to_date");
    assert_eq!(repo.on_origin("demo"), repo.head(&demo));
}

#[test]
fn new_commits_are_pushed_files_not_committed_are_left_out_and_the_pr_is_read_again() {
    let repo = PushRepo::new();
    repo.fixture.write_tool("gh", FAKE_GH);
    let demo = repo.pushed_demo();
    repo.commit_in(&demo, "b.txt", "b\n", "Add b");
    fs::write(demo.join("scratch.txt"), "not committed\n").unwrap();

    let doc = repo.json_of(&["worktree", "push", "demo"]);

    assert_eq!(doc["contract"], 1);
    assert_eq!(doc["name"], "demo");
    assert_eq!(doc["path"], demo.to_str().unwrap());
    assert_eq!(doc["branch"], "demo");
    assert_eq!(doc["remote"], "origin/demo");
    assert_eq!(doc["outcome"], "pushed");
    assert_eq!(doc["with_lease"], false);
    assert_eq!(doc["uncommitted"], 1);
    assert_eq!(doc["pr"]["number"], 7);
    assert_eq!(doc["pr"]["checks"], "none");
    assert_eq!(repo.on_origin("demo"), repo.head(&demo));
    let listed = repo.json_of(&["worktree", "ls"]);
    assert_eq!(listed["worktrees"][0]["pr"]["number"], 7, "pr.json is kept");
    assert_eq!(listed["has_origin"], true);

    repo.commit_in(&demo, "c.txt", "c\n", "Add c");
    assert_eq!(
        repo.ok(&["worktree", "push", "demo"]),
        "demo: pushed to origin/demo\n1 file with uncommitted changes was left out of the push\n"
    );
}

#[test]
fn a_branch_rebased_after_its_push_is_refused_then_pushed_with_a_lease() {
    let repo = PushRepo::new();
    let demo = repo.pushed_demo();
    let pushed = repo.head(&demo);
    repo.commit_in(&repo.other, "c.txt", "c\n", "upstream change");
    repo.git_in(&repo.other, &["push", "--quiet", "origin", "main"]);
    repo.ok(&["worktree", "rebase", "demo"]);
    let rebased = repo.head(&demo);
    assert_ne!(rebased, pushed);

    let out = repo.run(&["worktree", "push", "demo"]);
    assert_eq!(out.status.code(), Some(21), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "orca-term: branch demo of worktree demo diverged from origin/demo (after a rebase?), so \
         git refused the push; `orca-term worktree push demo --force-with-lease` would replace \
         origin/demo with it (nothing was pushed)\n"
    );
    let error = repo.error_of(&["worktree", "push", "demo"], 21);
    assert_eq!(error["kind"], "push_diverged");
    assert_eq!(repo.on_origin("demo"), pushed, "a plain push never forces");

    let doc = repo.json_of(&["worktree", "push", "demo", "--force-with-lease"]);
    assert_eq!(doc["outcome"], "pushed");
    assert_eq!(doc["with_lease"], true);
    assert_eq!(repo.on_origin("demo"), rebased);
}

#[test]
fn the_lease_refuses_when_someone_else_pushed_since() {
    let repo = PushRepo::new();
    let demo = repo.pushed_demo();
    // Someone else pushes a commit onto demo, which this repository has not fetched.
    repo.git_in(&repo.other, &["fetch", "--quiet", "origin", "demo:demo"]);
    repo.git_in(&repo.other, &["switch", "--quiet", "demo"]);
    repo.commit_in(&repo.other, "theirs.txt", "theirs\n", "Their change");
    repo.git_in(&repo.other, &["push", "--quiet", "origin", "demo"]);
    let theirs = repo.head(&repo.other);
    // This branch is rewritten meanwhile.
    repo.git_in(
        &demo,
        &["commit", "--quiet", "--amend", "-m", "Add a, reworded"],
    );

    let out = repo.run(&["worktree", "push", "demo", "--force-with-lease"]);

    assert_eq!(out.status.code(), Some(12), "{out:?}");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.starts_with("orca-term: could not push branch demo of worktree demo to origin: "),
        "{stderr}"
    );
    assert!(stderr.contains("(stale info)"), "{stderr}");
    let error = repo.error_of(&["worktree", "push", "demo", "--force-with-lease"], 12);
    assert_eq!(error["kind"], "push_failed");
    assert_eq!(repo.on_origin("demo"), theirs, "their commit stays");
}

#[test]
fn a_worktree_on_no_branch_or_in_a_rebase_is_refused_and_nothing_is_pushed() {
    let repo = PushRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit_in(&demo, "shared.txt", "mine\n", "mine");
    repo.commit_in(&repo.other, "shared.txt", "theirs\n", "theirs");
    repo.git_in(&repo.other, &["push", "--quiet", "origin", "main"]);
    let out = repo.run(&["worktree", "rebase", "demo"]);
    assert_eq!(out.status.code(), Some(19), "{out:?}");

    let error = repo.error_of(&["worktree", "push", "demo"], 1);
    assert_eq!(
        error,
        json!({
            "kind": "other",
            "message": "worktree demo is in the middle of a rebase, so its branch is not pushed; \
                        finish or abort the rebase first (nothing was pushed)"
        })
    );
    repo.ok(&["worktree", "rebase", "demo", "--abort"]);
    repo.git_in(&demo, &["switch", "--quiet", "--detach"]);

    let error = repo.error_of(&["worktree", "push", "demo"], 1);
    assert_eq!(
        error["message"],
        "worktree demo is on no branch, so there is nothing to push"
    );
    assert_eq!(repo.on_origin("demo"), "");
}

#[test]
fn a_repository_with_no_origin_is_refused() {
    let repo = PushRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit_in(&demo, "a.txt", "a\n", "Add a");
    repo.fixture.repo.git(&["remote", "remove", "origin"]);

    let error = repo.error_of(&["worktree", "push", "demo"], 1);

    assert_eq!(
        error["message"],
        "worktree demo cannot be pushed: this repository has no `origin` remote (nothing was \
         pushed)"
    );
    assert_eq!(repo.json_of(&["worktree", "ls"])["has_origin"], false);
    assert_eq!(repo.on_origin("demo"), "");
}
