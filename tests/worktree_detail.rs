//! `orca-term worktree detail`, driven as a black box: the drift from the local default branch,
//! counted with real commits on both sides, a failed setup's whole reason and the end of its log,
//! a worktree whose directory is gone, and the refusals. The denials, which come from claude's
//! hooks, are covered in `tests/hooks.rs`.

#![expect(
    clippy::unwrap_used,
    reason = "clippy.toml relaxes unwrap only inside #[test] fns; fixture helpers panic on failure too"
)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "a black-box test builds real repositories and files around the binary it drives"
)]

#[expect(
    dead_code,
    reason = "the fixture is shared between suites, and this one leaves part of it unused"
)]
mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::{Value, json};
use support::Repo;

/// Runs the binary from the primary checkout with `args` and returns what it did.
fn orca_term(repo: &Repo, args: &[&str]) -> Output {
    repo.orca_term(&repo.root).args(args).output().unwrap()
}

/// `worktree detail <name> --json`, which must succeed.
fn detail(repo: &Repo, name: &str) -> Value {
    let out = orca_term(repo, &["worktree", "detail", name, "--json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

/// Creates the worktree `name` with `worktree new`, approving its setup script, and returns its
/// path.
fn new_worktree(repo: &Repo, name: &str) -> PathBuf {
    orca_term(
        repo,
        &["worktree", "new", name, "--json", "--approve-setup"],
    );
    repo.home.join("orca-term/worktrees/repo").join(name)
}

/// Runs git in the worktree at `worktree` and panics on failure.
fn git_in(repo: &Repo, worktree: &Path, args: &[&str]) -> String {
    let mut all = vec!["-C", worktree.to_str().unwrap()];
    all.extend_from_slice(args);
    repo.git(&all)
}

/// Makes an empty commit in the worktree at `worktree`.
fn commit_in(repo: &Repo, worktree: &Path, message: &str) {
    git_in(
        repo,
        worktree,
        &["commit", "--quiet", "--allow-empty", "-m", message],
    );
}

#[test]
fn drift_counts_the_commits_on_each_side_of_the_local_default_branch() {
    let repo = Repo::new();
    let worktree = new_worktree(&repo, "fix");
    assert_eq!(
        detail(&repo, "fix")["drift"],
        json!({"against": "main", "ahead": 0, "behind": 0})
    );

    commit_in(&repo, &worktree, "one");
    commit_in(&repo, &worktree, "two");
    repo.git(&["commit", "--quiet", "--allow-empty", "-m", "main moves on"]);
    let doc = detail(&repo, "fix");
    assert_eq!(doc["contract"], 1);
    assert_eq!(
        doc["drift"],
        json!({"against": "main", "ahead": 2, "behind": 1})
    );
    assert_eq!(doc["worktree"]["name"], "fix");
    assert_eq!(doc["worktree"]["denials"], 0);
    assert_eq!(doc["detail"], Value::Null);
    assert_eq!(doc["denials"], json!([]));
    assert_eq!(doc["setup"], Value::Null, "no setup script ever ran");

    let text = orca_term(&repo, &["worktree", "detail", "fix"]);
    assert!(text.status.success());
    assert_eq!(
        String::from_utf8(text.stdout).unwrap(),
        "fix: no agent\n2 ahead of main, 1 behind\n"
    );
}

#[test]
fn drift_is_null_without_a_local_default_branch_even_when_origin_has_one() {
    let repo = Repo::new();
    new_worktree(&repo, "fix");
    // origin/HEAD names `main`, which exists only on origin once the local branch is renamed.
    repo.git(&["update-ref", "refs/remotes/origin/main", "main"]);
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/main",
    ]);
    repo.git(&["branch", "--quiet", "-m", "main", "trunk"]);
    assert_eq!(detail(&repo, "fix")["drift"], Value::Null);

    // With no origin either, there is no default branch at all.
    repo.git(&["symbolic-ref", "--delete", "refs/remotes/origin/HEAD"]);
    repo.git(&["update-ref", "-d", "refs/remotes/origin/main"]);
    let doc = detail(&repo, "fix");
    assert_eq!(doc["drift"], Value::Null);
    let text = orca_term(&repo, &["worktree", "detail", "fix"]);
    assert!(
        String::from_utf8(text.stdout)
            .unwrap()
            .ends_with("no drift to show: no local default branch, or a history unrelated to it\n")
    );
}

#[test]
fn drift_is_null_for_a_history_unrelated_to_the_default_branch() {
    let repo = Repo::new();
    let worktree = new_worktree(&repo, "orphan");
    git_in(
        &repo,
        &worktree,
        &["checkout", "--quiet", "--orphan", "elsewhere"],
    );
    commit_in(&repo, &worktree, "unrelated");
    assert_eq!(detail(&repo, "orphan")["drift"], Value::Null);
}

#[test]
fn a_failed_setup_gives_its_whole_reason_and_the_end_of_its_log() {
    let repo = Repo::new();
    let script = "for n in $(seq 1 30); do echo \"step $n\"; done\n\
                  printf '\\033[1;31merror\\033[0m: no lockfile, a reason far longer than a sidebar \
                  is wide and never cut\\n' >&2\n\
                  exit 3";
    repo.write(
        "orca-term.yaml",
        &format!(
            "scripts:\n  setup: |\n{}\n",
            script
                .lines()
                .map(|line| format!("    {line}"))
                .collect::<Vec<_>>()
                .join("\n")
        ),
    );
    repo.commit_all("setup script");
    let worktree = new_worktree(&repo, "fix");

    let doc = detail(&repo, "fix");
    let reason = "setup failed (exit 3): error: no lockfile, a reason far longer than a sidebar \
                  is wide and never cut";
    assert_eq!(doc["worktree"]["state"], "not_ready");
    assert_eq!(doc["detail"], reason, "whole, its colour codes taken out");
    let setup = &doc["setup"];
    assert_eq!(setup["status"], "failed");
    let log = PathBuf::from(setup["log"].as_str().unwrap());
    assert!(log.ends_with("orca-term/setup.log"), "{}", log.display());
    let tail: Vec<&str> = setup["tail"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap())
        .collect();
    assert_eq!(tail.len(), 20);
    assert_eq!(tail[0], "step 12");
    assert_eq!(tail[18], "step 30");
    assert_eq!(tail[19], &reason["setup failed (exit 3): ".len()..]);
    assert_eq!(doc["drift"]["ahead"], 0, "drift still counts: {doc}");

    let text = String::from_utf8(orca_term(&repo, &["worktree", "detail", "fix"]).stdout).unwrap();
    assert!(
        text.starts_with(&format!("fix: not ready\n  {reason}\n")),
        "{text}"
    );
    assert!(
        text.contains(&format!(
            "setup failed (exit 3) (log {}):\n  step 12\n",
            log.display()
        )),
        "{text}"
    );
    assert!(worktree.is_dir());
}

#[test]
fn a_worktree_whose_directory_is_gone_still_gives_its_state() {
    let repo = Repo::new();
    let worktree = new_worktree(&repo, "gone");
    fs::remove_dir_all(&worktree).unwrap();
    let doc = detail(&repo, "gone");
    assert_eq!(doc["worktree"]["state"], "broken");
    assert_eq!(
        doc["detail"],
        "directory missing; run orca-term worktree prune"
    );
    assert_eq!(doc["denials"], json!([]));
    assert_eq!(doc["setup"], Value::Null);
    assert_eq!(doc["drift"], Value::Null);
}

#[test]
fn detail_refuses_the_primary_checkout_and_an_unknown_worktree() {
    let repo = Repo::new();
    new_worktree(&repo, "fix");
    for target in ["nope", ".", repo.root.to_str().unwrap()] {
        let out = orca_term(&repo, &["worktree", "detail", target, "--json"]);
        assert_eq!(out.status.code(), Some(1), "{target}");
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        assert_eq!(doc["error"]["kind"], "other", "{target}");
    }
}

/// Writes `contents` to `file` in the worktree at `worktree` and commits it there.
fn commit_file_in(repo: &Repo, worktree: &Path, file: &str, contents: &str, message: &str) {
    fs::write(worktree.join(file), contents).unwrap();
    git_in(repo, worktree, &["add", file]);
    git_in(repo, worktree, &["commit", "--quiet", "-m", message]);
}

/// Runs git in the worktree at `worktree`, which must stop on a conflict.
fn conflict_in(repo: &Repo, worktree: &Path, args: &[&str]) {
    let mut all = vec!["-C", worktree.to_str().unwrap()];
    all.extend_from_slice(args);
    repo.git_failing(&all);
}

/// A repository whose `a.txt` changed one way on `main` and another way in the worktree `fix`
/// created from it, so that bringing either into the other conflicts. Returns the worktree's path.
fn diverged(repo: &Repo) -> PathBuf {
    repo.write("a.txt", "base\n");
    repo.commit_all("base");
    let worktree = new_worktree(repo, "fix");
    commit_file_in(repo, &worktree, "a.txt", "fix\n", "fix a");
    repo.write("a.txt", "main\n");
    repo.commit_all("main a");
    worktree
}

/// The `operation` of `fix` as `worktree ls --json` and `worktree detail --json` report it, which
/// must agree, and the detail's `operation_step`.
fn operation(repo: &Repo) -> (Value, Value) {
    let out = orca_term(repo, &["worktree", "ls", "--json"]);
    assert!(out.status.success());
    let listed: Value = serde_json::from_slice(&out.stdout).unwrap();
    let doc = detail(repo, "fix");
    assert_eq!(
        listed["worktrees"][0]["operation"],
        doc["worktree"]["operation"]
    );
    (
        doc["worktree"]["operation"].clone(),
        doc["operation_step"].clone(),
    )
}

/// What `worktree <command> ...` printed as text.
fn text_of(repo: &Repo, args: &[&str]) -> String {
    String::from_utf8(orca_term(repo, args).stdout).unwrap()
}

#[test]
fn a_conflicting_merge_reads_as_a_merge_until_it_is_aborted() {
    let repo = Repo::new();
    let worktree = diverged(&repo);
    assert_eq!(operation(&repo), (Value::Null, Value::Null));

    conflict_in(&repo, &worktree, &["merge", "--quiet", "main"]);
    assert_eq!(operation(&repo), (json!("merge"), Value::Null));
    assert_eq!(
        detail(&repo, "fix")["worktree"]["state"],
        "no_agent",
        "an operation is never a state"
    );
    let listed = text_of(&repo, &["worktree", "ls"]);
    assert!(listed.contains("no agent, merging"), "{listed}");
    let text = text_of(&repo, &["worktree", "detail", "fix"]);
    assert!(
        text.starts_with("fix: no agent, merging\nmerge in progress\n"),
        "{text}"
    );

    git_in(&repo, &worktree, &["merge", "--abort"]);
    assert_eq!(operation(&repo), (Value::Null, Value::Null));
}

#[test]
fn a_conflicting_rebase_reads_as_a_rebase_with_its_step() {
    let repo = Repo::new();
    let worktree = diverged(&repo);
    commit_file_in(&repo, &worktree, "b.txt", "more\n", "fix b");

    conflict_in(&repo, &worktree, &["rebase", "main"]);
    assert_eq!(
        operation(&repo),
        (json!("rebase"), json!({"current": 1, "total": 2}))
    );
    let text = text_of(&repo, &["worktree", "detail", "fix"]);
    assert!(text.contains("\nrebase in progress (1/2)\n"), "{text}");

    git_in(&repo, &worktree, &["rebase", "--abort"]);
    assert_eq!(operation(&repo), (Value::Null, Value::Null));
}

#[test]
fn a_conflicting_cherry_pick_and_revert_read_as_themselves() {
    let repo = Repo::new();
    let worktree = diverged(&repo);

    conflict_in(&repo, &worktree, &["cherry-pick", "main"]);
    assert_eq!(operation(&repo), (json!("cherry-pick"), Value::Null));
    git_in(&repo, &worktree, &["cherry-pick", "--abort"]);

    commit_file_in(&repo, &worktree, "a.txt", "fix again\n", "fix a again");
    conflict_in(&repo, &worktree, &["revert", "--no-edit", "HEAD~1"]);
    assert_eq!(operation(&repo), (json!("revert"), Value::Null));
    git_in(&repo, &worktree, &["revert", "--abort"]);
    assert_eq!(operation(&repo), (Value::Null, Value::Null));
}

#[test]
fn reading_an_operation_in_progress_writes_nothing() {
    let repo = Repo::new();
    let worktree = diverged(&repo);
    conflict_in(&repo, &worktree, &["merge", "--quiet", "main"]);
    let git_dir =
        PathBuf::from(git_in(&repo, &worktree, &["rev-parse", "--absolute-git-dir"]).trim());
    let status = || git_in(&repo, &worktree, &["status", "--porcelain=v2", "--branch"]);
    let refs = || repo.git(&["for-each-ref"]);
    let files = || {
        let mut names: Vec<String> = fs::read_dir(&git_dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    let merge_head = fs::read(git_dir.join("MERGE_HEAD")).unwrap();
    let (status_before, refs_before, files_before) = (status(), refs(), files());
    operation(&repo);
    text_of(&repo, &["worktree", "ls"]);
    text_of(&repo, &["worktree", "detail", "fix"]);
    assert_eq!(status(), status_before);
    assert_eq!(refs(), refs_before);
    assert_eq!(files(), files_before);
    assert_eq!(fs::read(git_dir.join("MERGE_HEAD")).unwrap(), merge_head);
}
