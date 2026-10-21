//! A worktree's setup, run by `orca-term worktree new` and again by `orca-term worktree setup`,
//! driven as a black box.

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

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use serde_json::Value;
use support::Repo;

/// Runs the binary from the primary checkout with `args` and returns what it did.
fn orca_term(repo: &Repo, args: &[&str]) -> Output {
    repo.orca_term(&repo.root).args(args).output().unwrap()
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

/// Where a worktree lands under the built-in default base.
fn worktree_path(repo: &Repo, name: &str) -> PathBuf {
    repo.home.join("orca-term/worktrees/repo").join(name)
}

/// The worktree's state directory in git's admin dir, as git reports it.
fn admin_dir(repo: &Repo, worktree: &Path) -> PathBuf {
    let dir = repo.git(&[
        "-C",
        worktree.to_str().unwrap(),
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "orca-term",
    ]);
    PathBuf::from(dir.trim())
}

/// The setup script as YAML for `orca-term.yaml`.
fn setup_yaml(script: &str) -> String {
    let indented: Vec<String> = script.lines().map(|line| format!("    {line}")).collect();
    format!("scripts:\n  setup: |\n{}\n", indented.join("\n"))
}

/// Commits `orca-term.yaml` with `script` as the setup script.
fn commit_setup(repo: &Repo, script: &str) {
    repo.write("orca-term.yaml", &setup_yaml(script));
    repo.commit_all("setup script");
}

/// Writes the uncommitted override `.git/orca-term.yaml` with `script` as the setup script.
fn override_setup(repo: &Repo, script: &str) {
    repo.write(".git/orca-term.yaml", &setup_yaml(script));
}

/// The one worktree of `worktree ls --json`.
fn listed(repo: &Repo) -> Value {
    let out = orca_term(repo, &["worktree", "ls", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    doc["worktrees"][0].clone()
}

fn recorded_setup(repo: &Repo, worktree: &Path) -> Value {
    let file = admin_dir(repo, worktree).join("worktree.json");
    let state: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    state["setup"].clone()
}

/// Waits, for at most 30 seconds, until `path` exists.
fn wait_for(path: &Path) {
    let start = Instant::now();
    while !path.exists() {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "{} never appeared",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A setup script that writes its pid to `<dir>/setup.pid`, says it started by creating
/// `<dir>/started`, then waits for `<dir>/release` for at most 30 seconds, failing if it never
/// comes, so a failing test cannot hang.
fn blocking_script(dir: &Path) -> String {
    let dir = dir.display();
    format!(
        "echo $$ > {dir}/setup.pid\n\
         touch {dir}/started\n\
         i=0\n\
         while [ ! -f {dir}/release ]; do\n\
         \x20 sleep 0.1\n\
         \x20 i=$((i + 1))\n\
         \x20 if [ $i -ge 300 ]; then exit 9; fi\n\
         done\n\
         echo released"
    )
}

#[test]
fn setup_runs_in_the_new_worktree_with_its_variables_and_is_logged_in_the_admin_dir() {
    let repo = Repo::new();
    commit_setup(
        &repo,
        "echo to-stdout\n\
         echo to-stderr >&2\n\
         {\n\
         \x20 echo \"cwd=$(pwd -P)\"\n\
         \x20 echo \"root=$ORCA_TERM_ROOT_PATH\"\n\
         \x20 echo \"worktree=$ORCA_TERM_WORKTREE_PATH\"\n\
         \x20 echo \"name=$ORCA_TERM_WORKTREE_NAME\"\n\
         \x20 echo \"orca-root=$ORCA_ROOT_PATH\"\n\
         \x20 echo \"orca-worktree=$ORCA_WORKTREE_PATH\"\n\
         } > ../setup-env.txt",
    );

    let out = orca_term(&repo, &["worktree", "new", "fix", "--approve-setup"]);

    assert!(out.status.success(), "{}", stderr(&out));
    let worktree = worktree_path(&repo, "fix");
    let env = std::fs::read_to_string(worktree.parent().unwrap().join("setup-env.txt")).unwrap();
    let root = repo.root.display();
    let path = worktree.display();
    assert_eq!(
        env,
        format!(
            "cwd={path}\nroot={root}\nworktree={path}\nname=fix\norca-root={root}\n\
             orca-worktree={path}\n"
        )
    );
    let log = admin_dir(&repo, &worktree).join("setup.log");
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "to-stdout\nto-stderr\n"
    );
    assert_eq!(
        stderr(&out),
        "creating worktree fix...\nto-stdout\nto-stderr\n",
        "on stderr, orca-term says what it starts before the setup's live output"
    );
    let report = stdout(&out);
    assert!(
        report.contains("setup:  succeeded (script from orca-term.yaml)"),
        "{report}"
    );
    assert!(
        report.contains(&format!("log:    {}", log.display())),
        "{report}"
    );
    assert_eq!(
        recorded_setup(&repo, &worktree),
        serde_json::json!({"status": "succeeded"})
    );
    assert_eq!(listed(&repo)["state"], "no_agent");
}

#[test]
fn a_failing_setup_leaves_the_worktree_not_ready_with_its_log_and_nothing_undone() {
    let repo = Repo::new();
    commit_setup(
        &repo,
        "echo installing\necho 'error: no lockfile' >&2\nexit 3\necho never",
    );

    let out = orca_term(
        &repo,
        &["worktree", "new", "broken-deps", "--approve-setup"],
    );

    assert_eq!(out.status.code(), Some(1), "{}", stderr(&out));
    let worktree = worktree_path(&repo, "broken-deps");
    assert!(worktree.is_dir(), "the worktree stays");
    assert!(
        repo.git(&["branch", "--list", "broken-deps"])
            .contains("broken-deps"),
        "the branch stays"
    );
    let log = admin_dir(&repo, &worktree).join("setup.log");
    assert_eq!(
        std::fs::read_to_string(&log).unwrap(),
        "installing\nerror: no lockfile\n"
    );
    let report = stdout(&out);
    for expected in [
        "setup:  failed (script from orca-term.yaml)".to_owned(),
        format!("log:    {}", log.display()),
        "not ready: setup failed (exit 3): error: no lockfile".to_owned(),
        "run `orca-term worktree setup broken-deps`".to_owned(),
    ] {
        assert!(report.contains(&expected), "{expected}\n{report}");
    }
    assert_eq!(
        recorded_setup(&repo, &worktree),
        serde_json::json!({"status": "failed", "exit_code": 3})
    );

    let entry = listed(&repo);
    assert_eq!(entry["state"], "not_ready");
    assert_eq!(entry["detail"], "setup failed (exit 3): error: no lockfile");
    let table = stdout(&orca_term(&repo, &["worktree", "ls"]));
    assert!(table.contains("not ready"), "{table}");
    assert!(
        table.contains("setup failed (exit 3): error: no lockfile"),
        "{table}"
    );
}

#[test]
fn with_json_a_failed_setup_prints_the_normal_document_and_echoes_nothing() {
    let repo = Repo::new();
    commit_setup(&repo, "echo noisy\nfalse");

    let out = orca_term(
        &repo,
        &["worktree", "new", "quiet", "--json", "--approve-setup"],
    );

    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), "", "no echo with --json");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["contract"], 1);
    assert_eq!(doc["worktree"]["name"], "quiet");
    assert_eq!(doc["worktree"]["state"], "not_ready");
    assert_eq!(doc["worktree"]["detail"], "setup failed (exit 1): noisy");
    assert_eq!(doc["setup"]["status"], "failed");
    assert!(
        doc["setup"]["log"]
            .as_str()
            .unwrap()
            .ends_with("/orca-term/setup.log")
    );
}

#[test]
fn worktree_setup_runs_the_script_again_and_a_success_makes_the_worktree_ready() {
    let repo = Repo::new();
    commit_setup(
        &repo,
        "if [ ! -f ready.txt ]; then echo 'missing ready.txt'; exit 1; fi\necho all good",
    );
    assert_eq!(
        orca_term(&repo, &["worktree", "new", "fix", "--approve-setup"])
            .status
            .code(),
        Some(1)
    );
    let worktree = worktree_path(&repo, "fix");
    assert_eq!(listed(&repo)["state"], "not_ready");

    std::fs::write(worktree.join("ready.txt"), "").unwrap();
    let out = orca_term(&repo, &["worktree", "setup", "fix"]);

    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(stderr(&out), "running the setup of fix...\nall good\n");
    assert!(stdout(&out).starts_with("worktree fix is ready\n"));
    assert_eq!(listed(&repo)["state"], "no_agent");
    let log = admin_dir(&repo, &worktree).join("setup.log");
    assert_eq!(
        std::fs::read_to_string(log).unwrap(),
        "all good\n",
        "overwritten on every run"
    );

    let out = orca_term(
        &repo,
        &["worktree", "setup", worktree.to_str().unwrap(), "--json"],
    );
    assert!(out.status.success(), "a path names the worktree too");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["worktree"]["state"], "no_agent");
    assert_eq!(doc["setup"]["status"], "succeeded");
}

#[test]
fn worktree_setup_with_no_script_clears_a_failure() {
    let repo = Repo::new();
    commit_setup(&repo, "exit 1");
    orca_term(&repo, &["worktree", "new", "fix", "--approve-setup"]);
    let worktree = worktree_path(&repo, "fix");
    assert_eq!(listed(&repo)["state"], "not_ready");

    // An empty script in the local override turns the committed one off.
    repo.write(".git/orca-term.yaml", "scripts:\n  setup: ''\n");
    let out = orca_term(&repo, &["worktree", "setup", "fix"]);

    assert!(out.status.success(), "{}", stderr(&out));
    let report = stdout(&out);
    assert!(report.starts_with("worktree fix is ready\n"), "{report}");
    assert!(report.contains("nothing to run"), "{report}");
    assert_eq!(recorded_setup(&repo, &worktree), Value::Null);
    assert_eq!(listed(&repo)["state"], "no_agent");
}

#[test]
fn the_setup_script_comes_from_the_worktree_under_the_local_override() {
    let repo = Repo::new();
    commit_setup(&repo, "echo committed > ../which.txt");
    // An uncommitted change in the primary checkout is not what the worktree checks out.
    repo.write("orca-term.yaml", &setup_yaml("echo primary > ../which.txt"));
    let which = || std::fs::read_to_string(worktree_path(&repo, "which.txt")).unwrap();

    orca_term(&repo, &["worktree", "new", "fix", "--approve-setup"]);
    assert_eq!(which(), "committed\n");

    let worktree = worktree_path(&repo, "fix");
    std::fs::write(
        worktree.join("orca-term.yaml"),
        setup_yaml("echo worktree > ../which.txt"),
    )
    .unwrap();
    orca_term(&repo, &["worktree", "setup", "fix", "--approve-setup"]);
    assert_eq!(which(), "worktree\n");

    override_setup(&repo, "echo local > ../which.txt");
    let out = orca_term(&repo, &["worktree", "setup", "fix"]);
    assert_eq!(which(), "local\n");
    let report = stdout(&out);
    assert!(
        report.contains(
            "setup:  succeeded (script from .git/orca-term.yaml, overriding orca-term.yaml)"
        ),
        "{report}"
    );
}

#[test]
fn while_setup_runs_the_worktree_is_setting_up_busy_and_cannot_be_removed() {
    let repo = Repo::new();
    assert!(orca_term(&repo, &["worktree", "new", "x"]).status.success());
    let signals = repo.tmp.path().canonicalize().unwrap();
    override_setup(&repo, &blocking_script(&signals));

    let mut running = repo.spawn_orca_term(&repo.root, &["worktree", "setup", "x"]);
    wait_for(&signals.join("started"));

    let entry = listed(&repo);
    assert_eq!(entry["state"], "setting_up");
    assert_eq!(entry["detail"], Value::Null);

    let second = orca_term(&repo, &["worktree", "setup", "x", "--json"]);
    assert_eq!(second.status.code(), Some(3));
    let doc: Value = serde_json::from_slice(&second.stdout).unwrap();
    assert_eq!(doc["contract"], 1);
    assert_eq!(doc["error"]["kind"], "busy");
    assert!(
        doc["error"]["message"].as_str().unwrap().contains("busy"),
        "{doc}"
    );
    let second = orca_term(&repo, &["worktree", "setup", "x"]);
    assert_eq!(second.status.code(), Some(3));
    assert!(stderr(&second).contains("is busy"), "{}", stderr(&second));

    for args in [
        vec!["worktree", "rm", "x"],
        vec!["worktree", "rm", "x", "--force"],
    ] {
        let removal = orca_term(&repo, &args);
        assert_eq!(removal.status.code(), Some(3), "{args:?}");
        assert!(stderr(&removal).contains("is busy"), "{}", stderr(&removal));
    }
    assert!(worktree_path(&repo, "x").is_dir(), "the worktree stays");

    std::fs::write(signals.join("release"), "").unwrap();
    assert!(running.wait().unwrap().success());
    assert_eq!(listed(&repo)["state"], "no_agent");
    let log = admin_dir(&repo, &worktree_path(&repo, "x")).join("setup.log");
    assert_eq!(std::fs::read_to_string(log).unwrap(), "released\n");
}

#[test]
fn a_setup_whose_process_died_lists_as_interrupted_and_listing_writes_nothing() {
    let repo = Repo::new();
    assert!(orca_term(&repo, &["worktree", "new", "x"]).status.success());
    let signals = repo.tmp.path().canonicalize().unwrap();
    override_setup(&repo, &blocking_script(&signals));

    let mut running = repo.spawn_orca_term(&repo.root, &["worktree", "setup", "x"]);
    wait_for(&signals.join("started"));
    let script_pid = std::fs::read_to_string(signals.join("setup.pid")).unwrap();
    running.kill().unwrap();
    running.wait().unwrap();
    let killed = Command::new("kill")
        .args(["-KILL", script_pid.trim()])
        .status()
        .unwrap();
    assert!(killed.success());

    let worktree = worktree_path(&repo, "x");
    let admin = admin_dir(&repo, &worktree);
    let state_before = std::fs::read(admin.join("worktree.json")).unwrap();
    let modified_before = std::fs::metadata(admin.join("worktree.json"))
        .unwrap()
        .modified()
        .unwrap();

    let entry = listed(&repo);
    assert_eq!(entry["state"], "not_ready");
    assert_eq!(entry["detail"], "setup interrupted");
    let table = stdout(&orca_term(&repo, &["worktree", "ls"]));
    assert!(table.contains("not ready"), "{table}");
    assert!(table.contains("setup interrupted"), "{table}");

    assert_eq!(
        std::fs::read(admin.join("worktree.json")).unwrap(),
        state_before,
        "listing never rewrites the state"
    );
    assert_eq!(
        std::fs::metadata(admin.join("worktree.json"))
            .unwrap()
            .modified()
            .unwrap(),
        modified_before
    );
    assert_eq!(
        recorded_setup(&repo, &worktree),
        serde_json::json!({"status": "running"})
    );

    // The lock died with the process, so setup can run again.
    std::fs::write(signals.join("release"), "").unwrap();
    assert!(
        orca_term(&repo, &["worktree", "setup", "x"])
            .status
            .success()
    );
    assert_eq!(listed(&repo)["state"], "no_agent");
}
