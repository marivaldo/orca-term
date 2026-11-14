//! `orca-term` with no arguments where it does not open the application (#148, #193), driven as a
//! black box (seam 1): inside Neovim and off a terminal it prints the usage, and without git it
//! refuses as `orca-term app` does. Every run gets a cleared environment holding only an isolated
//! PATH, an `XDG_STATE_HOME` inside the sandbox and what the test sets, so the developer's own
//! `NVIM` or project list never leaks in. A pseudo-terminal stands in for a person at a terminal.
//! Where it does open the application, `tests/app.rs` drives it.

#![expect(
    clippy::unwrap_used,
    reason = "clippy.toml relaxes unwrap only inside #[test] fns; fixture helpers panic on failure too"
)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "a black-box test builds real files around the binary it drives"
)]

#[expect(
    dead_code,
    reason = "the fixture is shared between suites, and this one leaves part of it unused"
)]
mod support;

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use assert_fs::TempDir;
use support::Repo;

/// What the binary prints first in its usage.
const USAGE: &str = "Usage: orca-term <COMMAND>";

/// What the application writes as it opens: the switch to the alternate screen.
const ALTERNATE_SCREEN: &str = "\x1b[?1049h";

/// A temporary directory holding `bin/`, the only directory on the binary's PATH, `work/`, the
/// directory it runs in unless a test names another, and `state/`, its `XDG_STATE_HOME`.
struct Sandbox {
    tmp: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        std::fs::create_dir(tmp.path().join("bin")).unwrap();
        std::fs::create_dir(tmp.path().join("work")).unwrap();
        Self { tmp }
    }

    fn bin(&self) -> PathBuf {
        self.tmp.path().join("bin")
    }

    fn work(&self) -> PathBuf {
        self.tmp.path().join("work")
    }

    /// The binary's `XDG_STATE_HOME`, which holds the project list.
    fn state(&self) -> PathBuf {
        self.tmp.path().join("state")
    }

    /// What the project list's file holds, or `None` when nothing was ever added.
    fn projects(&self) -> Option<String> {
        std::fs::read_to_string(self.state().join("orca-term").join("projects.json")).ok()
    }

    /// Puts the real `git`, as the test's own PATH finds it, on the binary's PATH.
    fn link_git(&self) {
        let out = Command::new("sh")
            .args(["-c", "command -v git"])
            .output()
            .unwrap();
        let git = String::from_utf8(out.stdout).unwrap();
        assert!(!git.trim().is_empty(), "git is not on the test's PATH");
        std::os::unix::fs::symlink(git.trim(), self.bin().join("git")).unwrap();
    }

    /// The binary with `args` and `vars` run in `dir`, its stdin, stdout and stderr not terminals.
    fn run_in(&self, dir: &Path, args: &[&str], vars: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_orca-term"))
            .args(args)
            .current_dir(dir)
            .env_clear()
            .env("PATH", self.bin())
            .env("XDG_STATE_HOME", self.state())
            .envs(vars.iter().copied())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    /// The binary with `args` and `vars` on a pseudo-terminal in `dir`, as a person types it in a
    /// shell.
    fn run_at_terminal_in(&self, dir: &Path, args: &[&str], vars: &[(&str, &str)]) -> TerminalRun {
        let command = pty_process::blocking::Command::new(env!("CARGO_BIN_EXE_orca-term"))
            .args(args)
            .current_dir(dir)
            .env_clear()
            .env("PATH", self.bin())
            .env("XDG_STATE_HOME", self.state())
            .envs(vars.iter().copied());
        let (pty, pts) = pty_process::blocking::open().unwrap();
        let mut child = command.spawn(pts).unwrap();
        let pty = Arc::new(pty);
        let screen = Arc::new(Mutex::new(Vec::new()));
        let reader = {
            let (pty, screen) = (Arc::clone(&pty), Arc::clone(&screen));
            thread::spawn(move || copy_screen(&pty, &screen))
        };
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
}

/// What a run at a terminal printed (stdout and stderr together) and its exit code.
struct TerminalRun {
    screen: String,
    code: Option<i32>,
}

/// Copies what the binary writes to its terminal into `screen`, until the read fails (EIO on
/// Linux) or ends.
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

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

/// A sandbox holding the real `git`, and a repository to run the binary in.
fn sandbox_and_repository() -> (Sandbox, Repo) {
    let sandbox = Sandbox::new();
    sandbox.link_git();
    (sandbox, Repo::new())
}

#[test]
fn no_arguments_inside_neovim_print_the_usage_even_at_a_terminal_and_add_nothing() {
    let (sandbox, repo) = sandbox_and_repository();
    let run = sandbox.run_at_terminal_in(&repo.root, &[], &[("NVIM", "/tmp/nvim.sock")]);
    assert_eq!(run.code, Some(2), "{}", run.screen);
    assert!(run.screen.contains(USAGE), "{}", run.screen);
    assert!(
        !run.screen.contains(ALTERNATE_SCREEN),
        "the application must not open"
    );
    assert_eq!(sandbox.projects(), None, "nothing was added");
}

#[test]
fn no_arguments_without_a_terminal_print_the_usage_and_add_nothing() {
    let (sandbox, repo) = sandbox_and_repository();
    let out = sandbox.run_in(&repo.root, &[], &[]);
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "",
        "nothing on stdout"
    );
    assert!(stderr(&out).contains(USAGE), "{}", stderr(&out));
    assert_eq!(sandbox.projects(), None, "nothing was added");
}

#[test]
fn no_arguments_without_git_refuse_as_the_application_does_and_add_nothing() {
    let sandbox = Sandbox::new();
    let repo = Repo::new();
    let run = sandbox.run_at_terminal_in(&repo.root, &[], &[]);
    assert_eq!(run.code, Some(9), "{}", run.screen);
    assert!(
        run.screen.starts_with("orca-term: git"),
        "the prerequisite names git: {}",
        run.screen
    );
    assert!(!run.screen.contains(ALTERNATE_SCREEN));
    assert_eq!(sandbox.projects(), None, "nothing was added");
}

#[test]
fn open_is_no_longer_a_command() {
    let sandbox = Sandbox::new();
    let out = sandbox.run_in(&sandbox.work(), &["open"], &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(
        stderr(&out).contains("unrecognized subcommand 'open'"),
        "{}",
        stderr(&out)
    );
}
