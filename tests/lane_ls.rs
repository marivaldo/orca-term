//! `orca-term lane ls`, driven as a black box.

#![expect(
    clippy::unwrap_used,
    reason = "clippy.toml relaxes unwrap only inside #[test] fns; fixture helpers panic on failure too"
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
        .args(["lane", "ls", "--json"])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn lane_paths(doc: &Value) -> Vec<String> {
    doc["lanes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["path"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn a_repository_without_lanes_has_an_empty_fleet() {
    let repo = Repo::new();
    repo.orca_term(&repo.root)
        .args(["lane", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::starts_with("no lanes in "));
    let doc = ls_json(&repo, &repo.root);
    assert_eq!(doc["lanes"], Value::Array(vec![]));
}

#[test]
fn json_carries_the_contract_and_version() {
    let repo = Repo::new();
    let doc = ls_json(&repo, &repo.root);
    assert_eq!(doc["contract"], 1);
    assert_eq!(doc["version"], env!("CARGO_PKG_VERSION"));
}

#[test]
fn a_hand_made_worktree_is_a_lane_and_the_primary_checkout_is_not() {
    let repo = Repo::new();
    let lane = repo.add_worktree(&repo.tmp.path().join("lanes/fix-login"), "fix-login");

    let doc = ls_json(&repo, &repo.root);
    let lanes = doc["lanes"].as_array().unwrap();
    assert_eq!(lanes.len(), 1);
    assert_eq!(lanes[0]["name"], "fix-login");
    assert_eq!(lanes[0]["branch"], "fix-login");
    assert_eq!(lanes[0]["path"], lane.to_str().unwrap());
    assert_eq!(lanes[0]["state"], "no_agent");
    assert_eq!(doc["primary"]["path"], repo.root.to_str().unwrap());
    assert!(!lane_paths(&doc).contains(&repo.root.to_str().unwrap().to_owned()));

    repo.orca_term(&repo.root)
        .args(["lane", "ls"])
        .assert()
        .success()
        .stdout(
            predicate::str::contains("fix-login")
                .and(predicate::str::contains("no agent"))
                .and(predicate::str::contains(lane.to_str().unwrap())),
        )
        .stdout(predicate::str::contains(format!("{}\n", repo.root.display())).not());
}

#[test]
fn lanes_sharing_a_basename_are_told_apart() {
    let repo = Repo::new();
    let a = repo.add_worktree(&repo.tmp.path().join("a/api"), "api-a");
    let b = repo.add_worktree(&repo.tmp.path().join("b/api"), "api-b");

    let doc = ls_json(&repo, &repo.root);
    let mut paths = lane_paths(&doc);
    paths.sort();
    assert_eq!(
        paths,
        vec![
            a.to_str().unwrap().to_owned(),
            b.to_str().unwrap().to_owned()
        ]
    );
    assert!(
        doc["lanes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|l| l["name"] == "api")
    );
}

#[test]
fn the_fleet_is_the_same_from_inside_a_lane() {
    let repo = Repo::new();
    let lane = repo.add_worktree(&repo.tmp.path().join("lanes/docs"), "docs");
    assert_eq!(ls_json(&repo, &lane), ls_json(&repo, &repo.root));
}

#[test]
fn listing_never_writes_to_the_repository() {
    let repo = Repo::new();
    repo.add_worktree(&repo.tmp.path().join("lanes/docs"), "docs");
    let admin = repo.root.join(".git/worktrees/docs/orca-term");
    let before = repo.git(&["status", "--porcelain", "--ignored"]);
    let refs_before = repo.git(&["for-each-ref"]);
    ls_json(&repo, &repo.root);
    assert_eq!(repo.git(&["status", "--porcelain", "--ignored"]), before);
    assert_eq!(repo.git(&["for-each-ref"]), refs_before);
    assert!(
        !admin.exists(),
        "reading a lane's state never creates its admin dir"
    );
}

#[test]
fn a_lane_state_of_a_newer_schema_still_lists_and_is_left_untouched() {
    let repo = Repo::new();
    repo.add_worktree(&repo.tmp.path().join("lanes/docs"), "docs");
    let newer = r#"{"schema":9,"agent":null,"future":{"x":1}}"#;
    let file = repo.write(".git/worktrees/docs/orca-term/lane.json", newer);
    let doc = ls_json(&repo, &repo.root);
    assert_eq!(doc["lanes"][0]["state"], "no_agent");
    assert_eq!(std::fs::read_to_string(file).unwrap(), newer);
}

#[test]
fn outside_a_repository_it_fails_with_a_reason() {
    let repo = Repo::new();
    let outside = repo.tmp.path().join("plain");
    std::fs::create_dir(&outside).unwrap();
    repo.orca_term(&outside)
        .args(["lane", "ls"])
        .env("GIT_CEILING_DIRECTORIES", repo.tmp.path())
        .assert()
        .failure()
        .stderr(predicate::str::contains("not inside a git repository"));
}
