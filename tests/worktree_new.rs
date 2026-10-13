//! `orca-term worktree new`, driven as a black box.

#![expect(
    clippy::unwrap_used,
    reason = "clippy.toml relaxes unwrap only inside #[test] fns; fixture helpers panic on failure too"
)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "a black-box test builds real repositories and files around the binary it drives"
)]

mod support;

use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::Command;

use predicates::prelude::*;
use serde_json::Value;
use support::Repo;

/// Runs `worktree new <name>` from the primary checkout and returns its stdout, panicking on failure.
fn worktree_new(repo: &Repo, name: &str) -> String {
    let out = repo
        .orca_term(&repo.root)
        .args(["worktree", "new", name])
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
}

/// Where a worktree lands under the built-in default base.
fn default_worktree_path(repo: &Repo, name: &str) -> PathBuf {
    repo.home.join("orca-term/worktrees/repo").join(name)
}

/// The directory holding the primary checkout, canonical.
fn outside(repo: &Repo) -> PathBuf {
    repo.root.parent().unwrap().to_owned()
}

fn git_in(repo: &Repo, dir: &Path, args: &[&str]) -> String {
    let mut full = vec!["-C", dir.to_str().unwrap()];
    full.extend_from_slice(args);
    repo.git(&full).trim().to_owned()
}

fn worktree_count(repo: &Repo) -> usize {
    repo.git(&["worktree", "list", "--porcelain"])
        .lines()
        .filter(|l| l.starts_with("worktree "))
        .count()
}

fn global_config(repo: &Repo) -> PathBuf {
    repo.xdg.join("orca-term/config.yaml")
}

fn write_global(repo: &Repo, contents: &str) {
    let path = global_config(repo);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, contents).unwrap();
}

#[test]
fn creates_the_worktree_on_a_new_branch_from_the_default_branch_whatever_is_checked_out() {
    let repo = Repo::new();
    repo.write("README.md", "hello\n");
    repo.commit_all("readme");
    let main = repo.git(&["rev-parse", "main"]).trim().to_owned();
    repo.git(&["switch", "--quiet", "-c", "feature"]);
    repo.write("feature.txt", "wip\n");
    repo.commit_all("feature work");

    let stdout = worktree_new(&repo, "fix-login");

    let worktree = default_worktree_path(&repo, "fix-login");
    assert!(worktree.is_dir(), "{stdout}");
    assert_eq!(git_in(&repo, &worktree, &["rev-parse", "HEAD"]), main);
    assert_eq!(
        git_in(&repo, &worktree, &["branch", "--show-current"]),
        "fix-login"
    );
    assert!(!worktree.join("feature.txt").exists());
    assert_eq!(repo.git(&["branch", "--show-current"]).trim(), "feature");
    assert!(stdout.contains("created worktree fix-login"), "{stdout}");
    assert!(
        stdout.contains(&format!("path:   {}", worktree.display())),
        "{stdout}"
    );
    assert!(stdout.contains("branch: fix-login (from main)"), "{stdout}");
}

#[test]
fn branches_from_origin_head_when_there_is_no_local_default_branch() {
    let repo = Repo::new();
    repo.git(&["switch", "--quiet", "-c", "scratch"]);
    repo.write("trunk.txt", "trunk\n");
    repo.commit_all("trunk work");
    let trunk = repo.git(&["rev-parse", "HEAD"]).trim().to_owned();
    repo.git(&["update-ref", "refs/remotes/origin/trunk", &trunk]);
    repo.git(&[
        "symbolic-ref",
        "refs/remotes/origin/HEAD",
        "refs/remotes/origin/trunk",
    ]);
    repo.git(&["switch", "--quiet", "main"]);

    let stdout = worktree_new(&repo, "docs");

    let worktree = default_worktree_path(&repo, "docs");
    assert_eq!(git_in(&repo, &worktree, &["rev-parse", "HEAD"]), trunk);
    assert!(
        stdout.contains("branch: docs (from origin/trunk)"),
        "{stdout}"
    );
}

#[test]
fn from_inside_a_worktree_the_new_worktree_belongs_to_the_primary_checkout() {
    let repo = Repo::new();
    let hand_made = repo.add_worktree(&repo.tmp.path().join("elsewhere/other"), "other");
    repo.orca_term(&hand_made)
        .args(["worktree", "new", "sibling"])
        .assert()
        .success();
    assert!(default_worktree_path(&repo, "sibling").is_dir());
    assert!(!repo.home.join("orca-term/worktrees/other").exists());
}

#[test]
fn base_defaults_under_home() {
    let repo = Repo::new();
    let stdout = worktree_new(&repo, "a");
    assert!(default_worktree_path(&repo, "a").is_dir());
    let base = repo.home.join("orca-term/worktrees");
    assert!(
        stdout.contains(&format!("base:   {} (built-in default)", base.display())),
        "{stdout}"
    );
}

#[test]
fn the_global_config_overrides_the_built_in_default() {
    let repo = Repo::new();
    write_global(&repo, "base: ~/global-worktrees\n");
    let stdout = worktree_new(&repo, "a");
    let base = repo.home.join("global-worktrees");
    assert!(base.join("repo/a").is_dir());
    assert!(
        stdout.contains(&format!(
            "base:   {} (from {})",
            base.display(),
            global_config(&repo).display()
        )),
        "{stdout}"
    );
}

#[test]
fn the_committed_config_overrides_the_global_one() {
    let repo = Repo::new();
    write_global(&repo, "base: ~/global-worktrees\n");
    repo.write(
        "orca-term.yaml",
        "# worktrees beside the repo\nbase: ../committed-worktrees\n",
    );
    repo.commit_all("config");
    let stdout = worktree_new(&repo, "a");
    let base = outside(&repo).join("committed-worktrees");
    assert!(base.join("repo/a").is_dir());
    assert!(
        stdout.contains(&format!(
            "base:   {} (from orca-term.yaml, overriding {})",
            base.display(),
            global_config(&repo).display()
        )),
        "{stdout}"
    );
}

#[test]
fn the_local_override_beats_the_committed_config() {
    let repo = Repo::new();
    write_global(&repo, "base: ~/global-worktrees\n");
    repo.write("orca-term.yaml", "base: ../committed-worktrees\n");
    repo.commit_all("config");
    let base = outside(&repo).join("local-worktrees");
    repo.write(
        ".git/orca-term.yaml",
        &format!("base: {}\n", base.display()),
    );
    let stdout = worktree_new(&repo, "a");
    assert!(base.join("repo/a").is_dir());
    assert!(
        stdout.contains(&format!(
            "base:   {} (from .git/orca-term.yaml, overriding orca-term.yaml, {})",
            base.display(),
            global_config(&repo).display()
        )),
        "{stdout}"
    );
}

#[test]
fn an_unknown_key_is_refused_naming_the_file_and_the_key() {
    let repo = Repo::new();
    repo.write(
        "orca-term.yaml",
        "base: ../worktrees\nsharedDirectories: [node_modules]\n",
    );
    repo.orca_term(&repo.root)
        .args(["worktree", "new", "a"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains(repo.root.join("orca-term.yaml").to_str().unwrap())
                .and(predicate::str::contains("unknown key `sharedDirectories`")),
        );
    assert_eq!(worktree_count(&repo), 1);

    let repo = Repo::new();
    write_global(&repo, "worktrees: ~/x\n");
    repo.orca_term(&repo.root)
        .args(["worktree", "new", "a"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains(global_config(&repo).to_str().unwrap())
                .and(predicate::str::contains("unknown key `worktrees`")),
        );
}

#[test]
fn malformed_yaml_is_refused_naming_the_file() {
    let repo = Repo::new();
    repo.write(".git/orca-term.yaml", "base: [\n");
    repo.orca_term(&repo.root)
        .args(["worktree", "new", "a"])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains(repo.root.join(".git/orca-term.yaml").to_str().unwrap())
                .and(predicate::str::contains("not valid YAML")),
        );
    assert_eq!(worktree_count(&repo), 1);
}

#[test]
fn a_base_inside_the_primary_checkout_is_refused() {
    let repo = Repo::new();
    repo.write("orca-term.yaml", "base: worktrees\n");
    repo.orca_term(&repo.root)
        .args(["worktree", "new", "a"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("inside the primary checkout"));
    assert_eq!(worktree_count(&repo), 1);
    assert!(!repo.root.join("worktrees").exists());
}

#[test]
fn invalid_names_are_refused() {
    let repo = Repo::new();
    let taken = default_worktree_path(&repo, "taken");
    std::fs::create_dir_all(&taken).unwrap();
    for (args, reason) in [
        (vec!["a/b"], "cannot contain `/`"),
        (vec![""], "cannot be empty"),
        (vec!["-x"], "cannot start with `-`"),
        (vec!["--", "-x"], "cannot start with `-`"),
        (vec!["bad..name"], "not a valid branch name"),
        (vec!["has space"], "not a valid branch name"),
        (vec!["taken"], "already exists"),
    ] {
        let mut cmd = repo.orca_term(&repo.root);
        cmd.args(["worktree", "new"]).args(&args);
        cmd.assert()
            .failure()
            .stderr(predicate::str::contains(reason));
    }
    assert_eq!(worktree_count(&repo), 1);
    assert_eq!(repo.git(&["branch", "--list"]).trim(), "* main");
}

/// The worktree name's branch rules mirror `git check-ref-format --branch` without running git:
/// every name here is one git rejects, and the core refuses it while parsing its arguments.
#[test]
fn names_git_rejects_as_branches_are_refused_while_parsing() {
    let repo = Repo::new();
    for name in [
        "bad..name",
        "has space",
        "tab\there",
        "del\u{7f}",
        "a~1",
        "a^1",
        "a:b",
        "a?b",
        "a*b",
        "a[b",
        "a\\b",
        ".hidden",
        "trailing.",
        "x.lock",
        "a@{1}",
        "HEAD",
    ] {
        let git = Command::new("git")
            .args(["check-ref-format", "--branch", name])
            .output()
            .unwrap();
        assert!(!git.status.success(), "git accepts {name:?}");
        repo.orca_term(&repo.root)
            .args(["worktree", "new", name])
            .assert()
            .failure()
            .stderr(predicate::str::contains("invalid value"))
            .stderr(predicate::str::contains("not a valid branch name"));
    }
    assert_eq!(worktree_count(&repo), 1);
    assert_eq!(repo.git(&["branch", "--list"]).trim(), "* main");
}

/// The guard for ADR 0002: the copy list copies ignored files byte for byte, and nothing in a worktree
/// shares an inode or a symlink with the primary checkout.
#[test]
fn the_copy_list_copies_ignored_files_and_never_shares_them() {
    let repo = Repo::new();
    repo.write(
        ".gitignore",
        ".env\n.env.link\nsecret.txt\nconfig/local.yaml\n",
    );
    repo.write(
        ".worktreeinclude",
        ".env\n.env.link\ntracked.txt\nnotes.txt\nconfig/local.yaml\n",
    );
    repo.write("tracked.txt", "committed\n");
    repo.commit_all("ignore rules and copy list");
    let env = repo.write(".env", "TOKEN=abc\n");
    std::fs::set_permissions(&env, std::fs::Permissions::from_mode(0o600)).unwrap();
    repo.write("config/local.yaml", "port: 1\n");
    repo.write("secret.txt", "not listed\n");
    repo.write("notes.txt", "listed but not ignored\n");
    repo.write("tracked.txt", "dirty in the primary checkout\n");
    std::os::unix::fs::symlink(".env", repo.root.join(".env.link")).unwrap();

    let stdout = worktree_new(&repo, "copy");
    let worktree = default_worktree_path(&repo, "copy");

    assert!(stdout.contains("copied: 2 files"), "{stdout}");
    for rel in [".env", "config/local.yaml"] {
        let src = std::fs::symlink_metadata(repo.root.join(rel)).unwrap();
        let dst = std::fs::symlink_metadata(worktree.join(rel)).unwrap();
        assert!(dst.file_type().is_file(), "{rel} is a regular file");
        assert_ne!(dst.ino(), src.ino(), "{rel} shares no inode");
        assert_eq!(dst.nlink(), 1, "{rel} is not hardlinked");
        assert_eq!(
            std::fs::read(worktree.join(rel)).unwrap(),
            std::fs::read(repo.root.join(rel)).unwrap()
        );
    }
    let mode = std::fs::metadata(worktree.join(".env")).unwrap().mode() & 0o777;
    assert_eq!(mode, 0o600, "the mode is preserved");
    assert!(
        std::fs::symlink_metadata(worktree.join(".env.link")).is_err(),
        "symlinks are skipped"
    );
    assert!(!worktree.join("secret.txt").exists(), "unlisted files stay");
    assert!(!worktree.join("notes.txt").exists(), "unignored files stay");
    assert_eq!(
        std::fs::read_to_string(worktree.join("tracked.txt")).unwrap(),
        "committed\n",
        "tracked files come from the checkout, not from the copy list"
    );

    std::fs::write(worktree.join(".env"), "TOKEN=changed-in-worktree\n").unwrap();
    assert_eq!(std::fs::read_to_string(&env).unwrap(), "TOKEN=abc\n");
}

#[test]
fn a_failure_after_the_worktree_exists_says_so_and_leaves_it() {
    let repo = Repo::new();
    repo.write(".gitignore", ".env\n");
    repo.write(".worktreeinclude", ".env\n");
    repo.commit_all("copy list");
    let env = repo.write(".env", "TOKEN=abc\n");
    std::fs::set_permissions(&env, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&env).is_ok() {
        // Running as root: permissions cannot make the copy fail.
        return;
    }
    repo.orca_term(&repo.root)
        .args(["worktree", "new", "half"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("left in place"));
    assert!(default_worktree_path(&repo, "half").is_dir());
    assert_eq!(worktree_count(&repo), 2);
    std::fs::set_permissions(&env, std::fs::Permissions::from_mode(0o600)).unwrap();
}

#[test]
fn the_worktree_state_is_written_to_the_admin_dir_and_lists_as_no_agent() {
    let repo = Repo::new();
    worktree_new(&repo, "fresh");
    let worktree = default_worktree_path(&repo, "fresh");

    let admin = git_in(
        &repo,
        &worktree,
        &[
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "orca-term",
        ],
    );
    let state: Value =
        serde_json::from_slice(&std::fs::read(Path::new(&admin).join("worktree.json")).unwrap())
            .unwrap();
    assert_eq!(state, serde_json::json!({"schema": 1, "agent": null}));
    assert!(!worktree.join(".orca-term").exists());
    assert_eq!(git_in(&repo, &worktree, &["status", "--porcelain"]), "");

    let out = repo
        .orca_term(&repo.root)
        .args(["worktree", "ls", "--json"])
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["worktrees"][0]["name"], "fresh");
    assert_eq!(doc["worktrees"][0]["state"], "no_agent");
    repo.orca_term(&repo.root)
        .args(["worktree", "ls"])
        .assert()
        .success()
        .stdout(predicate::str::contains("STATE").and(predicate::str::contains("no agent")));
}
