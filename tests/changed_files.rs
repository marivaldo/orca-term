//! `changed_files` in `worktree ls --json` and `worktree detail --json`, driven as a black box
//! (#92): how many files a worktree changed against its merge base with the default branch,
//! committed, uncommitted and untracked, ignored files left out, counted with real git.

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

use serde_json::{Value, json};
use support::Repo;

/// The stdout of the binary run from the primary checkout with `args`, which must succeed.
fn stdout_of(repo: &Repo, args: &[&str]) -> String {
    let out = repo.orca_term(&repo.root).args(args).output().unwrap();
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// `changed_files` of `name` in `worktree ls --json` and `worktree detail --json`, which must
/// agree.
fn changed_files(repo: &Repo, name: &str) -> Value {
    let listed: Value =
        serde_json::from_str(&stdout_of(repo, &["worktree", "ls", "--json"])).unwrap();
    let listed = listed["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worktree| worktree["name"] == name)
        .unwrap()["changed_files"]
        .clone();
    let detail: Value =
        serde_json::from_str(&stdout_of(repo, &["worktree", "detail", name, "--json"])).unwrap();
    assert_eq!(detail["worktree"]["changed_files"], listed, "{name}");
    listed
}

/// A repository whose `main` holds `a.txt`, `b.txt` and a `.gitignore` ignoring `*.log`, with the
/// worktree `demo` made by `worktree new`; returns the worktree's path.
fn repo_with_demo(repo: &Repo) -> PathBuf {
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");
    repo.write(".gitignore", "*.log\n");
    repo.commit_all("base");
    stdout_of(repo, &["worktree", "new", "demo", "--json"]);
    repo.home.join("orca-term/worktrees/repo/demo")
}

/// Runs git in the worktree at `worktree` and panics on failure.
fn git_in(repo: &Repo, worktree: &Path, args: &[&str]) -> String {
    let mut all = vec!["-C", worktree.to_str().unwrap()];
    all.extend_from_slice(args);
    repo.git(&all)
}

#[test]
fn committed_uncommitted_and_untracked_files_count_and_ignored_ones_do_not() {
    let repo = Repo::new();
    let demo = repo_with_demo(&repo);
    assert_eq!(changed_files(&repo, "demo"), json!(0), "no changes yet");
    assert_eq!(
        stdout_of(&repo, &["worktree", "detail", "demo"]),
        "demo: no agent\n0 ahead of main, 0 behind\n",
        "no changes: no line for them"
    );

    fs::write(demo.join("a.txt"), "a, changed\n").unwrap();
    git_in(
        &repo,
        &demo,
        &["commit", "--quiet", "--all", "-m", "change a"],
    );
    assert_eq!(changed_files(&repo, "demo"), json!(1), "committed");
    assert_eq!(
        stdout_of(&repo, &["worktree", "detail", "demo"]),
        "demo: no agent\n1 file changed\n1 ahead of main, 0 behind\n"
    );

    fs::write(demo.join("b.txt"), "b, changed\n").unwrap();
    assert_eq!(changed_files(&repo, "demo"), json!(2), "uncommitted");

    fs::write(demo.join("new.txt"), "new\n").unwrap();
    assert_eq!(changed_files(&repo, "demo"), json!(3), "untracked");

    fs::write(demo.join("build.log"), "ignored\n").unwrap();
    assert_eq!(changed_files(&repo, "demo"), json!(3), "ignored");
    assert_eq!(
        stdout_of(&repo, &["worktree", "detail", "demo"]),
        "demo: no agent\n3 files changed\n1 ahead of main, 0 behind\n"
    );
    let table = stdout_of(&repo, &["worktree", "ls"]);
    assert!(
        !table.contains("changed"),
        "the table is unchanged: {table}"
    );
}

#[test]
fn the_count_is_against_the_merge_base_so_main_moving_on_does_not_count() {
    let repo = Repo::new();
    let demo = repo_with_demo(&repo);
    fs::write(demo.join("a.txt"), "a, changed\n").unwrap();
    git_in(
        &repo,
        &demo,
        &["commit", "--quiet", "--all", "-m", "change a"],
    );
    repo.write("b.txt", "b on main\n");
    repo.write("c.txt", "c on main\n");
    repo.commit_all("main moves on");
    assert_eq!(changed_files(&repo, "demo"), json!(1));

    git_in(&repo, &demo, &["rm", "--quiet", "--cached", "b.txt"]);
    assert_eq!(
        changed_files(&repo, "demo"),
        json!(2),
        "a file removed from the index but kept on disk counts once"
    );
}

#[test]
fn the_count_is_null_when_it_cannot_be_computed() {
    let repo = Repo::new();
    let demo = repo_with_demo(&repo);
    git_in(
        &repo,
        &demo,
        &["checkout", "--quiet", "--orphan", "elsewhere"],
    );
    git_in(&repo, &demo, &["commit", "--quiet", "-m", "unrelated"]);
    assert_eq!(
        changed_files(&repo, "demo"),
        Value::Null,
        "no merge base with main"
    );

    let doomed = repo.add_worktree(&repo.tmp.path().join("worktrees/doomed"), "doomed");
    fs::write(doomed.join("new.txt"), "new\n").unwrap();
    assert_eq!(changed_files(&repo, "doomed"), json!(1));
    fs::remove_dir_all(&doomed).unwrap();
    assert_eq!(changed_files(&repo, "doomed"), Value::Null, "broken");
}

#[test]
fn counting_the_changes_writes_nothing() {
    let repo = Repo::new();
    let demo = repo_with_demo(&repo);
    fs::write(demo.join("b.txt"), "b, changed\n").unwrap();
    fs::write(demo.join("new.txt"), "new\n").unwrap();
    let status = || git_in(&repo, &demo, &["status", "--porcelain=v2", "--ignored"]);
    let refs = || repo.git(&["for-each-ref"]);
    let (status_before, refs_before) = (status(), refs());
    assert_eq!(changed_files(&repo, "demo"), json!(2));
    assert_eq!(status(), status_before);
    assert_eq!(refs(), refs_before);
}
