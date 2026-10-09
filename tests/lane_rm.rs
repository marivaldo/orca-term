//! `orca-term lane rm` and `orca-term lane prune`, driven as a black box.

#![expect(
    clippy::unwrap_used,
    reason = "clippy.toml relaxes unwrap only inside #[test] fns; fixture helpers panic on failure too"
)]

mod support;

use std::path::{Path, PathBuf};

use predicates::prelude::*;
use support::Repo;

fn git_in(repo: &Repo, dir: &Path, args: &[&str]) -> String {
    let mut full = vec!["-C", dir.to_str().unwrap()];
    full.extend_from_slice(args);
    repo.git(&full)
}

fn has_branch(repo: &Repo, branch: &str) -> bool {
    !repo.git(&["branch", "--list", branch]).trim().is_empty()
}

fn registered(repo: &Repo, path: &Path) -> bool {
    repo.git(&["worktree", "list", "--porcelain"])
        .lines()
        .any(|l| l == format!("worktree {}", path.display()))
}

fn lane(repo: &Repo, rel: &str, branch: &str) -> PathBuf {
    repo.add_worktree(&repo.tmp.path().join(rel), branch)
}

/// Commits a file inside `lane`, so its branch is not merged into `main`.
fn commit_in(repo: &Repo, lane: &Path) {
    std::fs::write(lane.join("work.txt"), "work\n").unwrap();
    git_in(repo, lane, &["add", "work.txt"]);
    git_in(repo, lane, &["commit", "--quiet", "-m", "work"]);
}

fn rm(repo: &Repo, args: &[&str]) -> assert_cmd::assert::Assert {
    let mut full = vec!["lane", "rm"];
    full.extend_from_slice(args);
    repo.orca_term(&repo.root).args(full).assert()
}

#[test]
fn removing_a_lane_with_an_unmerged_commit_keeps_its_branch() {
    let repo = Repo::new();
    let fix = lane(&repo, "lanes/fix", "fix");
    commit_in(&repo, &fix);

    rm(&repo, &["fix"])
        .success()
        .stdout(predicate::str::contains("removed lane fix"))
        .stdout(predicate::str::contains(
            "kept branch fix: not merged into main",
        ));
    assert!(!fix.exists());
    assert!(!registered(&repo, &fix));
    assert!(has_branch(&repo, "fix"));
}

#[test]
fn removing_a_lane_with_no_new_commits_deletes_its_branch() {
    let repo = Repo::new();
    let docs = lane(&repo, "lanes/docs", "docs");

    rm(&repo, &["docs"])
        .success()
        .stdout(predicate::str::contains(
            "deleted branch docs (merged into main)",
        ));
    assert!(!docs.exists());
    assert!(!has_branch(&repo, "docs"));
}

#[test]
fn a_branch_merged_into_main_is_deleted() {
    let repo = Repo::new();
    let fix = lane(&repo, "lanes/fix", "fix");
    commit_in(&repo, &fix);
    repo.git(&["merge", "--quiet", "--ff-only", "fix"]);

    rm(&repo, &["fix"])
        .success()
        .stdout(predicate::str::contains(
            "deleted branch fix (merged into main)",
        ));
    assert!(!has_branch(&repo, "fix"));
}

#[test]
fn a_dirty_lane_needs_force() {
    let repo = Repo::new();
    let wip = lane(&repo, "lanes/wip", "wip");
    std::fs::write(wip.join("scratch.txt"), "unsaved\n").unwrap();

    rm(&repo, &["wip"])
        .failure()
        .stderr(predicate::str::contains("--force"))
        .stderr(predicate::str::contains("untracked"));
    assert!(wip.join("scratch.txt").exists());
    assert!(registered(&repo, &wip));

    rm(&repo, &["wip", "--force"]).success();
    assert!(!wip.exists());
    assert!(!registered(&repo, &wip));
}

#[test]
fn force_deletes_an_unmerged_branch() {
    let repo = Repo::new();
    let fix = lane(&repo, "lanes/fix", "fix");
    commit_in(&repo, &fix);

    rm(&repo, &["--force", "fix"])
        .success()
        .stdout(predicate::str::contains("deleted branch fix (--force)"));
    assert!(!has_branch(&repo, "fix"));
}

#[test]
fn a_detached_lane_has_no_branch_to_delete() {
    let repo = Repo::new();
    let look = repo.tmp.path().join("lanes/look");
    repo.git(&[
        "worktree",
        "add",
        "--quiet",
        "--detach",
        look.to_str().unwrap(),
    ]);

    rm(&repo, &["look"])
        .success()
        .stdout(predicate::str::contains("no branch to delete"));
    assert!(!look.exists());
}

#[test]
fn a_broken_lane_is_refused_and_points_to_prune() {
    let repo = Repo::new();
    let gone = lane(&repo, "lanes/gone", "gone");
    std::fs::remove_dir_all(&gone).unwrap();

    rm(&repo, &["gone"])
        .failure()
        .stderr(predicate::str::contains("orca-term lane prune"));
    rm(&repo, &["--force", "gone"])
        .failure()
        .stderr(predicate::str::contains("orca-term lane prune"));
    assert!(registered(&repo, &gone));
    assert!(has_branch(&repo, "gone"));
}

#[test]
fn the_primary_checkout_is_refused() {
    let repo = Repo::new();
    rm(&repo, &[repo.root.to_str().unwrap()])
        .failure()
        .stderr(predicate::str::contains("primary checkout"));
    rm(&repo, &["."])
        .failure()
        .stderr(predicate::str::contains("primary checkout"));
    rm(&repo, &["repo"])
        .failure()
        .stderr(predicate::str::contains("primary checkout"));
    assert!(repo.root.join(".git").exists());
}

#[test]
fn an_unknown_lane_is_refused() {
    let repo = Repo::new();
    let elsewhere = repo.tmp.path().join("plain");
    std::fs::create_dir(&elsewhere).unwrap();
    rm(&repo, &["nope"])
        .failure()
        .stderr(predicate::str::contains("no lane named `nope`"));
    rm(&repo, &[elsewhere.to_str().unwrap()])
        .failure()
        .stderr(predicate::str::contains("is not a lane"));
    assert!(elsewhere.exists());
}

#[test]
fn an_ambiguous_name_is_refused_and_a_path_disambiguates() {
    let repo = Repo::new();
    let a = lane(&repo, "a/api", "api-a");
    let b = lane(&repo, "b/api", "api-b");

    rm(&repo, &["api"])
        .failure()
        .stderr(predicate::str::contains(a.to_str().unwrap()))
        .stderr(predicate::str::contains(b.to_str().unwrap()));
    assert!(a.exists() && b.exists());

    rm(&repo, &[a.to_str().unwrap()]).success();
    assert!(!a.exists());
    assert!(b.exists());
}

#[test]
fn prune_cleans_a_broken_lane_and_reports_it() {
    let repo = Repo::new();
    let gone = lane(&repo, "lanes/gone", "gone");
    let kept = lane(&repo, "lanes/kept", "kept");
    std::fs::remove_dir_all(&gone).unwrap();

    repo.orca_term(&repo.root)
        .args(["lane", "prune"])
        .assert()
        .success()
        .stdout(predicate::str::diff(format!("pruned {}\n", gone.display())));
    assert!(!registered(&repo, &gone));
    assert!(registered(&repo, &kept));
    assert!(has_branch(&repo, "gone"), "pruning leaves branches alone");
}

#[test]
fn prune_with_nothing_broken_says_so() {
    let repo = Repo::new();
    lane(&repo, "lanes/kept", "kept");
    repo.orca_term(&repo.root)
        .args(["lane", "prune"])
        .assert()
        .success()
        .stdout(predicate::str::diff("nothing to prune\n"));
}

#[test]
fn no_other_command_prunes_a_broken_lane() {
    let repo = Repo::new();
    let gone = lane(&repo, "lanes/gone", "gone");
    let other = lane(&repo, "lanes/other", "other");
    std::fs::remove_dir_all(&gone).unwrap();

    repo.orca_term(&repo.root)
        .args(["lane", "ls"])
        .assert()
        .success();
    repo.orca_term(&repo.root)
        .args(["lane", "ls", "--json"])
        .assert()
        .success();
    repo.orca_term(&repo.root)
        .args(["lane", "new", "fresh"])
        .assert()
        .success();
    rm(&repo, &["other"]).success();
    assert!(!other.exists());

    assert!(
        registered(&repo, &gone),
        "only `lane prune` cleans a broken lane"
    );
}

#[test]
fn a_lane_whose_state_is_unreadable_can_still_be_removed() {
    let repo = Repo::new();
    let docs = lane(&repo, "lanes/docs", "docs");
    repo.write(".git/worktrees/docs/orca-term/lane.json", "{not json");
    repo.write("unrelated.txt", "main moves on\n");
    repo.commit_all("unrelated");

    rm(&repo, &["docs"]).success();
    assert!(!docs.exists());
    assert!(!repo.root.join(".git/worktrees/docs").exists());
}

#[test]
fn the_default_branch_is_never_deleted_even_with_force() {
    for force in [false, true] {
        let repo = Repo::new();
        repo.git(&["switch", "--quiet", "-c", "side"]);
        let trunk = repo.tmp.path().join("lanes/trunk");
        repo.git(&[
            "worktree",
            "add",
            "--quiet",
            trunk.to_str().unwrap(),
            "main",
        ]);

        let args: &[&str] = if force {
            &["--force", "trunk"]
        } else {
            &["trunk"]
        };
        rm(&repo, args).success().stdout(predicate::str::contains(
            "kept branch main: it is the default branch",
        ));
        assert!(!trunk.exists(), "force={force}");
        assert!(has_branch(&repo, "main"), "force={force}");
    }
}

#[test]
fn a_branch_checked_out_in_another_worktree_is_kept() {
    let repo = Repo::new();
    let one = lane(&repo, "lanes/one", "shared");
    let two = repo.tmp.path().join("lanes/two");
    repo.git(&[
        "worktree",
        "add",
        "--quiet",
        "--force",
        two.to_str().unwrap(),
        "shared",
    ]);

    rm(&repo, &["--force", one.to_str().unwrap()])
        .success()
        .stdout(predicate::str::contains(
            "kept branch shared: could not delete it",
        ));
    assert!(!one.exists());
    assert!(two.exists());
    assert!(has_branch(&repo, "shared"));
}
