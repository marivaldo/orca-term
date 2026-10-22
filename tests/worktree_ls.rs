//! `orca-term worktree ls`, driven as a black box.

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

use predicates::prelude::*;
use serde_json::Value;
use support::Repo;

fn ls_json(repo: &Repo, dir: &std::path::Path) -> Value {
    let out = repo
        .orca_term(dir)
        .args(["worktree", "ls", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn worktree_paths(doc: &Value) -> Vec<String> {
    doc["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["path"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_repository_without_worktrees_has_an_empty_fleet() {
    let repo = Repo::new();
    repo.orca_term(&repo.root)
        .args(["worktree", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("no worktrees in "));
    let doc = ls_json(&repo, &repo.root);
    assert_eq!(doc["worktrees"], Value::Array(vec![]));
}

#[test]
fn wt_is_an_alias_of_worktree() {
    let repo = Repo::new();
    repo.add_worktree(&repo.tmp.path().join("worktrees/docs"), "docs");
    let run = |command: &str| {
        let out = repo
            .orca_term(&repo.root)
            .args([command, "ls"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    };
    let listed = run("worktree");
    assert!(listed.contains("docs"), "{listed}");
    assert_eq!(run("wt"), listed);
}

#[test]
fn json_carries_the_contract_and_version() {
    let repo = Repo::new();
    let doc = ls_json(&repo, &repo.root);
    assert_eq!(doc["contract"], 1);
    assert_eq!(doc["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn a_hand_made_worktree_is_listed_and_the_primary_checkout_is_not() {
    let repo = Repo::new();
    let worktree = repo.add_worktree(&repo.tmp.path().join("worktrees/fix-login"), "fix-login");

    let doc = ls_json(&repo, &repo.root);
    let worktrees = doc["worktrees"].as_array().unwrap();
    assert_eq!(worktrees.len(), 1);
    assert_eq!(worktrees[0]["name"], "fix-login");
    assert_eq!(worktrees[0]["branch"], "fix-login");
    assert_eq!(worktrees[0]["path"], worktree.to_str().unwrap());
    assert_eq!(worktrees[0]["state"], "no_agent");
    assert_eq!(doc["primary"]["path"], repo.root.to_str().unwrap());
    assert!(!worktree_paths(&doc).contains(&repo.root.to_str().unwrap().to_owned()));

    repo.orca_term(&repo.root)
        .args(["worktree", "ls"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("fix-login")
                .and(predicate::str::contains("no agent"))
                .and(predicate::str::contains(worktree.to_str().unwrap())),
        )
        .stdout(predicate::str::contains(format!("{}\n", repo.root.display())).not());
}

#[test]
fn worktrees_sharing_a_basename_are_told_apart() {
    let repo = Repo::new();
    let a = repo.add_worktree(&repo.tmp.path().join("a/api"), "api-a");
    let b = repo.add_worktree(&repo.tmp.path().join("b/api"), "api-b");

    let doc = ls_json(&repo, &repo.root);
    let mut paths = worktree_paths(&doc);
    paths.sort();
    assert_eq!(
        paths,
        vec![
            a.to_str().unwrap().to_owned(),
            b.to_str().unwrap().to_owned()
        ]
    );
    assert!(
        doc["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .all(|l| l["name"] == "api")
    );
}

#[test]
fn the_fleet_is_the_same_from_inside_a_worktree() {
    let repo = Repo::new();
    let worktree = repo.add_worktree(&repo.tmp.path().join("worktrees/docs"), "docs");
    assert_eq!(ls_json(&repo, &worktree), ls_json(&repo, &repo.root));
}

#[test]
fn listing_never_writes_to_the_repository() {
    let repo = Repo::new();
    repo.add_worktree(&repo.tmp.path().join("worktrees/docs"), "docs");
    let admin = repo.root.join(".git/worktrees/docs/orca-term");
    let before = repo.git(&["status", "--porcelain", "--ignored"]);
    let refs_before = repo.git(&["for-each-ref"]);
    ls_json(&repo, &repo.root);
    assert_eq!(repo.git(&["status", "--porcelain", "--ignored"]), before);
    assert_eq!(repo.git(&["for-each-ref"]), refs_before);
    assert!(
        !admin.exists(),
        "reading a worktree's state never creates its admin dir"
    );
}

/// The git commands `worktree ls` runs from `repo`'s primary checkout, one line of arguments each,
/// through a `git` on PATH that logs its arguments to a file in the fixture and runs the real one.
fn git_runs_of_ls(repo: &Repo) -> Vec<String> {
    let real = std::process::Command::new("sh")
        .args(["-c", "command -v git"])
        .output()
        .unwrap();
    let real = String::from_utf8(real.stdout).unwrap().trim().to_owned();
    let bin = repo.tmp.path().join("logging-bin");
    std::fs::create_dir_all(&bin).unwrap();
    let log = repo.tmp.path().join("git-runs.log");
    let _ = std::fs::remove_file(&log);
    let wrapper = bin.join("git");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\necho \"$*\" >> '{}'\nexec '{real}' \"$@\"\n",
            log.display()
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&wrapper).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut permissions, 0o755);
    std::fs::set_permissions(&wrapper, permissions).unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    repo.orca_term(&repo.root)
        .args(["worktree", "ls", "--json"])
        .env("PATH", path)
        .assert()
        .success();
    std::fs::read_to_string(&log)
        .unwrap()
        .lines()
        .map(str::to_owned)
        .collect()
}

#[test]
fn an_operation_in_progress_is_read_without_running_git_again() {
    let repo = Repo::new();
    repo.write("a.txt", "base\n");
    repo.commit_all("base");
    let worktree = repo.add_worktree(&repo.tmp.path().join("worktrees/fix"), "fix");
    let wt = worktree.to_str().unwrap();
    std::fs::write(worktree.join("a.txt"), "fix\n").unwrap();
    repo.git(&["-C", wt, "commit", "--quiet", "--all", "-m", "fix a"]);
    repo.write("a.txt", "main\n");
    repo.commit_all("main a");
    let quiet = git_runs_of_ls(&repo);

    repo.git_failing(&["-C", wt, "merge", "--quiet", "main"]);
    assert_eq!(
        ls_json(&repo, &repo.root)["worktrees"][0]["operation"],
        "merge"
    );
    assert_eq!(
        git_runs_of_ls(&repo),
        quiet,
        "a merge in progress adds no git run to the listing"
    );
}

#[test]
fn a_worktree_state_of_a_newer_schema_still_lists_and_is_left_untouched() {
    let repo = Repo::new();
    repo.add_worktree(&repo.tmp.path().join("worktrees/docs"), "docs");
    let newer = r#"{"schema":9,"agent":null,"future":{"x":1}}"#;
    let file = repo.write(".git/worktrees/docs/orca-term/worktree.json", newer);
    let doc = ls_json(&repo, &repo.root);
    assert_eq!(doc["worktrees"][0]["state"], "no_agent");
    assert_eq!(std::fs::read_to_string(file).unwrap(), newer);
}

#[test]
fn outside_a_repository_it_fails_with_a_reason() {
    let repo = Repo::new();
    let outside = repo.tmp.path().join("plain");
    std::fs::create_dir(&outside).unwrap();
    repo.orca_term(&outside)
        .args(["worktree", "ls"])
        .env("GIT_CEILING_DIRECTORIES", repo.tmp.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("not inside a git repository"));
}

fn registered_worktrees(repo: &Repo) -> String {
    repo.git(&["worktree", "list", "--porcelain"])
}

#[test]
fn a_worktree_whose_directory_is_gone_lists_as_broken_and_is_not_pruned() {
    let repo = Repo::new();
    let worktree = repo.add_worktree(&repo.tmp.path().join("worktrees/gone"), "gone");
    std::fs::remove_dir_all(&worktree).unwrap();

    let doc = ls_json(&repo, &repo.root);
    assert_eq!(doc["worktrees"][0]["path"], worktree.to_str().unwrap());
    assert_eq!(doc["worktrees"][0]["state"], "broken");
    let detail = doc["worktrees"][0]["detail"].as_str().unwrap();
    assert!(detail.contains("directory missing"), "{detail}");
    assert!(detail.contains("orca-term worktree prune"), "{detail}");

    repo.orca_term(&repo.root)
        .args(["worktree", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("broken"));
    assert!(
        registered_worktrees(&repo).contains(&format!("worktree {}", worktree.display())),
        "listing never prunes"
    );
    assert!(
        !repo.root.join(".git/worktrees/gone/orca-term").exists(),
        "listing never creates the admin dir"
    );
}

#[test]
fn a_worktree_whose_state_cannot_be_parsed_lists_as_broken_without_failing_the_fleet() {
    let repo = Repo::new();
    repo.add_worktree(&repo.tmp.path().join("worktrees/docs"), "docs");
    repo.add_worktree(&repo.tmp.path().join("worktrees/fine"), "fine");
    let corrupt = repo.write(".git/worktrees/docs/orca-term/worktree.json", "{not json");

    let doc = ls_json(&repo, &repo.root);
    let worktrees = doc["worktrees"].as_array().unwrap();
    let docs = worktrees.iter().find(|l| l["name"] == "docs").unwrap();
    let fine = worktrees.iter().find(|l| l["name"] == "fine").unwrap();
    assert_eq!(docs["state"], "broken");
    let detail = docs["detail"].as_str().unwrap();
    assert!(detail.contains("could not parse"), "{detail}");
    assert!(detail.contains("worktree.json"), "{detail}");
    assert_eq!(fine["state"], "no_agent");
    assert_eq!(fine["detail"], Value::Null);
    assert_eq!(std::fs::read_to_string(corrupt).unwrap(), "{not json");
}
