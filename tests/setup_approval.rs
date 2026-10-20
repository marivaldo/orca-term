//! The approval of a committed setup script (#58): `worktree new` and `worktree setup` run a
//! script from a committed `orca-term.yaml` only once the person approved its exact text in this
//! repository, driven as a black box. Without a terminal they fail as `untrusted`; at a terminal
//! (a real pseudo-terminal here) they ask.

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

use std::fmt::Write as _;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::Output;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest as _, Sha256};
use support::Repo;

/// The detail of a worktree whose committed script was not approved.
fn not_approved(name: &str) -> String {
    format!("setup not approved; run orca-term worktree setup {name}")
}

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

/// A setup script that appends `<label>` to `../ran.txt`, beside the worktrees, so a test can
/// tell whether, and which, script ran.
fn marking_script(label: &str) -> String {
    format!("echo {label} >> ../ran.txt\n")
}

/// The config file holding `script` as the setup script.
fn setup_yaml(script: &str) -> String {
    let indented: Vec<String> = script.lines().map(|line| format!("    {line}")).collect();
    format!("scripts:\n  setup: |\n{}\n", indented.join("\n"))
}

/// Commits `orca-term.yaml` with `script` as the setup script.
fn commit_setup(repo: &Repo, script: &str) {
    repo.write("orca-term.yaml", &setup_yaml(script));
    repo.commit_all("setup script");
}

/// What the scripts that ran wrote, one label a line; empty when none ran.
fn ran(repo: &Repo) -> String {
    std::fs::read_to_string(worktree_path(repo, "ran.txt")).unwrap_or_default()
}

/// The approvals' file, in the git common dir.
fn trust_file(repo: &Repo) -> PathBuf {
    repo.root.join(".git/orca-term-trust.json")
}

/// The sha256 of `text`, in lowercase hex.
fn sha256(text: &str) -> String {
    let mut hex = String::new();
    for byte in Sha256::digest(text.as_bytes()) {
        write!(hex, "{byte:02x}").unwrap();
    }
    hex
}

/// The worktree `name` as `worktree ls --json` lists it.
fn listed(repo: &Repo, name: &str) -> Value {
    let out = orca_term(repo, &["worktree", "ls", "--json"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    doc["worktrees"]
        .as_array()
        .unwrap()
        .iter()
        .find(|worktree| worktree["name"] == name)
        .cloned()
        .unwrap_or_else(|| panic!("{name} not listed in {doc}"))
}

/// The worktree's `worktree.json`, as bytes.
fn state_file(repo: &Repo, worktree: &Path) -> Vec<u8> {
    let dir = repo.git(&[
        "-C",
        worktree.to_str().unwrap(),
        "rev-parse",
        "--path-format=absolute",
        "--git-path",
        "orca-term",
    ]);
    std::fs::read(PathBuf::from(dir.trim()).join("worktree.json")).unwrap()
}

/// Asserts that `out` failed as `untrusted` with `--json`, naming `script` from the committed file
/// of the worktree `name`.
fn assert_untrusted_json(repo: &Repo, out: &Output, name: &str, script: &str) {
    assert_eq!(out.status.code(), Some(8), "{}", stderr(out));
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["contract"], 1);
    assert_eq!(doc["error"]["kind"], "untrusted");
    assert_eq!(doc["error"]["script"], script);
    let source = worktree_path(repo, name).join("orca-term.yaml");
    assert_eq!(doc["error"]["source"], source.display().to_string());
    let message = doc["error"]["message"].as_str().unwrap();
    assert!(
        message.contains(&format!("the setup script of worktree {name}, from ")),
        "{message}"
    );
}

#[test]
fn without_a_terminal_a_committed_script_never_runs_and_fails_as_untrusted() {
    let repo = Repo::new();
    let script = marking_script("committed");
    commit_setup(&repo, &script);

    let out = orca_term(&repo, &["worktree", "new", "fix", "--json"]);

    assert_untrusted_json(&repo, &out, "fix", &script);
    assert_eq!(ran(&repo), "", "the script never ran");
    let worktree = worktree_path(&repo, "fix");
    assert!(worktree.is_dir(), "the worktree is created all the same");
    assert!(!trust_file(&repo).exists(), "nothing is approved");
    let entry = listed(&repo, "fix");
    assert_eq!(entry["state"], "not_ready");
    assert_eq!(entry["detail"], not_approved("fix"));

    // The same without --json: a message on stderr, the same exit code.
    let out = orca_term(&repo, &["worktree", "setup", "fix"]);
    assert_eq!(out.status.code(), Some(8));
    let message = stderr(&out);
    assert!(
        message.contains(&format!(
            "orca-term: the setup script of worktree fix, from {}, is not approved in this \
             repository, so it did not run and the worktree is not ready",
            worktree.join("orca-term.yaml").display()
        )),
        "{message}"
    );
    assert!(message.contains("--approve-setup"), "{message}");
    assert!(!message.contains("[y/N]"), "no prompt without a terminal");
    assert_eq!(ran(&repo), "");
}

#[test]
fn worktree_ls_shows_a_script_not_approved_without_writing_anything() {
    let repo = Repo::new();
    commit_setup(&repo, &marking_script("committed"));
    orca_term(&repo, &["worktree", "new", "fix"]);
    let worktree = worktree_path(&repo, "fix");
    let before = state_file(&repo, &worktree);
    assert_eq!(
        serde_json::from_slice::<Value>(&before).unwrap()["setup"],
        serde_json::json!({"status": "not_approved"})
    );

    let table = stdout(&orca_term(&repo, &["worktree", "ls"]));
    assert!(table.contains("not ready"), "{table}");
    assert!(table.contains(&not_approved("fix")), "{table}");
    let detail = orca_term(&repo, &["worktree", "detail", "fix", "--json"]);
    let detail: Value = serde_json::from_slice(&detail.stdout).unwrap();
    assert_eq!(detail["detail"], not_approved("fix"));
    assert_eq!(detail["setup"]["status"], "not_approved");

    assert_eq!(state_file(&repo, &worktree), before, "nothing rewritten");
    assert!(!trust_file(&repo).exists());
    assert_eq!(ran(&repo), "");
}

#[test]
fn approve_setup_records_the_approval_then_runs_and_it_covers_that_text() {
    let repo = Repo::new();
    let script = marking_script("committed");
    commit_setup(&repo, &script);
    orca_term(&repo, &["worktree", "new", "fix"]);

    let out = orca_term(&repo, &["worktree", "setup", "fix", "--approve-setup"]);

    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(ran(&repo), "committed\n");
    assert_eq!(listed(&repo, "fix")["state"], "no_agent");
    let trust: Value = serde_json::from_slice(&std::fs::read(trust_file(&repo)).unwrap()).unwrap();
    assert_eq!(trust["schema"], 1);
    let approved = trust["approved"].as_array().unwrap();
    assert_eq!(approved.len(), 1);
    assert_eq!(approved[0]["sha256"], sha256(&script));
    assert_eq!(approved[0]["source"], "orca-term.yaml");
    assert!(approved[0]["approved_at"].as_u64().unwrap() > 1_600_000_000_000);

    // The same text, in another worktree or again, runs without asking.
    let out = orca_term(&repo, &["worktree", "new", "docs", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    assert!(
        orca_term(&repo, &["worktree", "setup", "fix"])
            .status
            .success()
    );
    assert_eq!(ran(&repo), "committed\ncommitted\ncommitted\n");

    // Approving it again records nothing more.
    assert!(
        orca_term(&repo, &["worktree", "new", "more", "--approve-setup"])
            .status
            .success()
    );
    let trust: Value = serde_json::from_slice(&std::fs::read(trust_file(&repo)).unwrap()).unwrap();
    assert_eq!(trust["approved"].as_array().unwrap().len(), 1);
}

#[test]
fn a_changed_script_asks_again_on_a_branch_or_in_the_primary_checkout() {
    let repo = Repo::new();
    commit_setup(&repo, &marking_script("first"));
    assert!(
        orca_term(&repo, &["worktree", "new", "fix", "--approve-setup"])
            .status
            .success()
    );

    // A branch changes the script: setup reads the worktree's own file.
    let worktree = worktree_path(&repo, "fix");
    let branch_script = marking_script("branch");
    std::fs::write(worktree.join("orca-term.yaml"), setup_yaml(&branch_script)).unwrap();
    let out = orca_term(&repo, &["worktree", "setup", "fix", "--json"]);
    assert_untrusted_json(&repo, &out, "fix", &branch_script);
    assert_eq!(listed(&repo, "fix")["detail"], not_approved("fix"));

    // The primary checkout commits another script: a new worktree checks it out.
    let primary_script = marking_script("second");
    commit_setup(&repo, &primary_script);
    let out = orca_term(&repo, &["worktree", "new", "docs", "--json"]);
    assert_untrusted_json(&repo, &out, "docs", &primary_script);

    assert_eq!(ran(&repo), "first\n", "neither changed script ran");
}

#[test]
fn scripts_from_the_local_override_and_the_global_file_run_without_approval() {
    let repo = Repo::new();
    repo.write(
        ".git/orca-term.yaml",
        &setup_yaml(&marking_script("override")),
    );
    let out = orca_term(&repo, &["worktree", "new", "fix", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));

    std::fs::remove_file(repo.root.join(".git/orca-term.yaml")).unwrap();
    let global = repo.xdg.join("orca-term/config.yaml");
    std::fs::create_dir_all(global.parent().unwrap()).unwrap();
    std::fs::write(&global, setup_yaml(&marking_script("global"))).unwrap();
    let out = orca_term(&repo, &["worktree", "new", "docs", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));

    // The override wins over a committed script, and needs no approval either.
    commit_setup(&repo, &marking_script("committed"));
    repo.write(
        ".git/orca-term.yaml",
        &setup_yaml(&marking_script("override")),
    );
    let out = orca_term(&repo, &["worktree", "new", "more", "--json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));

    assert_eq!(ran(&repo), "override\nglobal\noverride\n");
    assert!(!trust_file(&repo).exists(), "nothing needed approving");
}

#[test]
fn approvals_live_in_the_git_common_dir_outside_every_worktree() {
    let repo = Repo::new();
    commit_setup(&repo, &marking_script("committed"));
    assert!(
        orca_term(&repo, &["worktree", "new", "fix", "--approve-setup"])
            .status
            .success()
    );
    let common = repo.git(&["rev-parse", "--path-format=absolute", "--git-common-dir"]);
    let common = PathBuf::from(common.trim()).canonicalize().unwrap();
    assert!(common.join("orca-term-trust.json").is_file());
    let worktree = worktree_path(&repo, "fix");
    assert!(!worktree.join("orca-term-trust.json").exists());
    assert!(!repo.root.join("orca-term-trust.json").exists());
    let status = repo.git(&["-C", worktree.to_str().unwrap(), "status", "--porcelain"]);
    assert_eq!(status, "", "the worktree has nothing new to commit");
}

/// What a run of the binary at a terminal printed (stdout and stderr together, as the terminal
/// shows them) and its exit code.
struct TerminalRun {
    screen: String,
    code: Option<i32>,
}

/// Runs the binary with `args` from the primary checkout on a pseudo-terminal, waits for `prompt`
/// on the screen (at most 30 seconds), types `answer` and a newline, and waits for it to exit.
fn run_at_terminal(repo: &Repo, args: &[&str], prompt: &str, answer: &str) -> TerminalRun {
    let (pty, pts) = pty_process::blocking::open().unwrap();
    let mut child = repo.pty_command(&repo.root, args).spawn(pts).unwrap();
    let pty = Arc::new(pty);
    let screen = Arc::new(Mutex::new(Vec::new()));
    let reader = {
        let (pty, screen) = (Arc::clone(&pty), Arc::clone(&screen));
        thread::spawn(move || copy_screen(&pty, &screen))
    };
    let start = Instant::now();
    while !String::from_utf8_lossy(&screen.lock().unwrap()).contains(prompt) {
        assert!(
            start.elapsed() < Duration::from_secs(30),
            "no prompt: {}",
            String::from_utf8_lossy(&screen.lock().unwrap())
        );
        thread::sleep(Duration::from_millis(20));
    }
    (&*pty).write_all(format!("{answer}\n").as_bytes()).unwrap();
    let status = child.wait().unwrap();
    // On macOS the read may not end while the master is open: give the reader a moment, then
    // take what it has.
    let start = Instant::now();
    while !reader.is_finished() && start.elapsed() < Duration::from_secs(2) {
        thread::sleep(Duration::from_millis(20));
    }
    let screen = String::from_utf8_lossy(&screen.lock().unwrap()).replace("\r\n", "\n");
    TerminalRun {
        screen,
        code: status.code(),
    }
}

/// Copies what the binary writes to its terminal into `screen`, until the read fails (EIO on
/// Linux) or ends once the binary has exited.
fn copy_screen(pty: &pty_process::blocking::Pty, screen: &Mutex<Vec<u8>>) {
    let mut buf = [0u8; 4096];
    let mut reader = pty;
    while let Ok(n) = reader.read(&mut buf) {
        if n == 0 {
            break;
        }
        screen.lock().unwrap().extend_from_slice(&buf[..n]);
    }
}

const PROMPT: &str = "run this setup script? [y/N] ";

#[test]
fn at_a_terminal_yes_records_the_approval_and_runs_the_script() {
    let repo = Repo::new();
    let script = marking_script("committed");
    commit_setup(&repo, &script);

    let run = run_at_terminal(&repo, &["worktree", "new", "fix"], PROMPT, "Yes");

    assert_eq!(run.code, Some(0), "{}", run.screen);
    let source = worktree_path(&repo, "fix").join("orca-term.yaml");
    let shown = format!(
        "the setup script of worktree fix comes from {}, committed to the repository, and is not \
         approved yet:\n\n    echo committed >> ../ran.txt\n\n{PROMPT}",
        source.display()
    );
    assert!(run.screen.contains(&shown), "{}", run.screen);
    assert!(run.screen.contains("setup:  succeeded"), "{}", run.screen);
    assert_eq!(ran(&repo), "committed\n");
    assert_eq!(listed(&repo, "fix")["state"], "no_agent");
    let trust = std::fs::read_to_string(trust_file(&repo)).unwrap();
    assert!(trust.contains(&sha256(&script)), "{trust}");

    // Approved, it runs at a terminal without asking.
    let out = orca_term(&repo, &["worktree", "setup", "fix"]);
    assert!(out.status.success(), "{}", stderr(&out));
}

#[test]
fn at_a_terminal_anything_but_yes_leaves_the_worktree_not_ready() {
    let repo = Repo::new();
    commit_setup(&repo, &marking_script("committed"));

    let run = run_at_terminal(&repo, &["worktree", "new", "fix"], PROMPT, "n");

    assert_eq!(run.code, Some(8), "{}", run.screen);
    assert!(
        run.screen.contains("is not approved in this repository"),
        "{}",
        run.screen
    );
    assert_eq!(ran(&repo), "");
    assert!(!trust_file(&repo).exists());
    let entry = listed(&repo, "fix");
    assert_eq!(entry["state"], "not_ready");
    assert_eq!(entry["detail"], not_approved("fix"));

    // `worktree setup` asks again, and an empty answer is a no.
    let run = run_at_terminal(&repo, &["worktree", "setup", "fix"], PROMPT, "");
    assert_eq!(run.code, Some(8), "{}", run.screen);
    assert_eq!(ran(&repo), "");

    // With --json nothing asks, even at a terminal.
    let (_pty, pts) = pty_process::blocking::open().unwrap();
    let mut child = repo
        .pty_command(&repo.root, &["worktree", "setup", "fix", "--json"])
        .stdout(std::process::Stdio::piped())
        .spawn(pts)
        .unwrap();
    let mut document = String::new();
    child
        .stdout
        .take()
        .unwrap()
        .read_to_string(&mut document)
        .unwrap();
    assert_eq!(child.wait().unwrap().code(), Some(8));
    let doc: Value = serde_json::from_str(&document).unwrap();
    assert_eq!(doc["error"]["kind"], "untrusted");
}
