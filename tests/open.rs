//! `orca-term` with no arguments and `orca-term open` (#148), driven as a black box (seam 1): a
//! fake `nvim`, a shell script on an isolated PATH, records the arguments it was opened with, so
//! no test ever runs a real Neovim. Every run gets a cleared environment holding only that PATH
//! and what the test sets, so the developer's own `NVIM` or `ORCA_TERM_NVIM` never leaks in. A
//! pseudo-terminal stands in for a person at a terminal.

#![expect(
    clippy::unwrap_used,
    reason = "clippy.toml relaxes unwrap only inside #[test] fns; fixture helpers panic on failure too"
)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "a black-box test builds real files around the binary it drives"
)]

use std::io::Read as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use assert_fs::TempDir;

/// What the binary prints first in its usage.
const USAGE: &str = "Usage: orca-term <COMMAND>";

/// What `orca-term open` says inside Neovim.
const ALREADY_INSIDE: &str = "orca-term: already inside Neovim; run :OrcaTerm sidebar\n";

/// A temporary directory holding `bin/`, the only directory on the binary's PATH, and `work/`,
/// the directory it runs in.
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

    /// Where a fake program named `name` records how it was run.
    fn record(&self, name: &str) -> PathBuf {
        self.tmp.path().join(format!("{name}.record"))
    }

    /// Writes a fake program named `name` into `dir`: it records its arguments, one per line, then
    /// `cwd=<its directory>`, in [`Sandbox::record`], and exits 0.
    fn fake_program(&self, dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let record = self.record(name);
        let script = format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{record}'\nprintf 'cwd=%s\\n' \"$PWD\" >> '{record}'\n",
            record = record.display()
        );
        std::fs::write(&path, script).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// The fake `nvim` on PATH.
    fn fake_nvim(&self) -> PathBuf {
        self.fake_program(&self.bin(), "nvim")
    }

    /// What the fake program `name` recorded, or `None` when it never ran.
    fn recorded(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(self.record(name)).ok()
    }

    /// What the fake `nvim` must record: opened with `-c 'OrcaTerm sidebar'` in `work/`.
    fn opened_with_sidebar(&self) -> String {
        let work = self.work().canonicalize().unwrap();
        format!("-c\nOrcaTerm sidebar\ncwd={}\n", work.display())
    }

    /// The binary with `args` and `vars`, its stdin, stdout and stderr not terminals.
    fn run(&self, args: &[&str], vars: &[(&str, &str)]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_orca-term"))
            .args(args)
            .current_dir(self.work())
            .env_clear()
            .env("PATH", self.bin())
            .envs(vars.iter().copied())
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    /// The binary with `args` and `vars` on a pseudo-terminal, as a person types it in a shell.
    fn run_at_terminal(&self, args: &[&str], vars: &[(&str, &str)]) -> TerminalRun {
        let command = pty_process::blocking::Command::new(env!("CARGO_BIN_EXE_orca-term"))
            .args(args)
            .current_dir(self.work())
            .env_clear()
            .env("PATH", self.bin())
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

#[test]
fn no_arguments_at_a_terminal_open_nvim_with_the_sidebar() {
    let sandbox = Sandbox::new();
    sandbox.fake_nvim();
    let run = sandbox.run_at_terminal(&[], &[]);
    assert_eq!(run.code, Some(0), "{}", run.screen);
    assert_eq!(
        sandbox.recorded("nvim").as_deref(),
        Some(sandbox.opened_with_sidebar().as_str())
    );
}

#[test]
fn no_arguments_inside_neovim_print_the_usage_even_at_a_terminal() {
    let sandbox = Sandbox::new();
    sandbox.fake_nvim();
    let run = sandbox.run_at_terminal(&[], &[("NVIM", "/tmp/nvim.sock")]);
    assert_eq!(run.code, Some(2), "{}", run.screen);
    assert!(run.screen.contains(USAGE), "{}", run.screen);
    assert_eq!(sandbox.recorded("nvim"), None, "nvim must not run");
}

#[test]
fn no_arguments_without_a_terminal_print_the_usage() {
    let sandbox = Sandbox::new();
    sandbox.fake_nvim();
    let out = sandbox.run(&[], &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert!(stderr(&out).contains(USAGE), "{}", stderr(&out));
    assert_eq!(sandbox.recorded("nvim"), None, "nvim must not run");
}

#[test]
fn open_opens_nvim_with_the_sidebar_without_a_terminal() {
    let sandbox = Sandbox::new();
    sandbox.fake_nvim();
    let out = sandbox.run(&["open"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        sandbox.recorded("nvim").as_deref(),
        Some(sandbox.opened_with_sidebar().as_str())
    );
}

#[test]
fn open_inside_neovim_is_refused() {
    let sandbox = Sandbox::new();
    sandbox.fake_nvim();
    let out = sandbox.run(&["open"], &[("NVIM", "/tmp/nvim.sock")]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), ALREADY_INSIDE);
    assert_eq!(sandbox.recorded("nvim"), None, "nvim must not run");
}

#[test]
fn a_missing_nvim_is_a_prerequisite() {
    let sandbox = Sandbox::new();
    let run = sandbox.run_at_terminal(&[], &[]);
    assert_eq!(run.code, Some(9), "{}", run.screen);
    assert!(
        run.screen
            .starts_with("orca-term: nvim: not found on PATH; install Neovim 0.11 or later"),
        "{}",
        run.screen
    );
    let out = sandbox.run(&["open"], &[]);
    assert_eq!(out.status.code(), Some(9));
    assert!(stderr(&out).contains("nvim: not found on PATH"));
}

#[test]
fn orca_term_nvim_names_the_program_to_open() {
    let sandbox = Sandbox::new();
    sandbox.fake_nvim();
    let elsewhere = sandbox.tmp.path().join("elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    let other = sandbox.fake_program(&elsewhere, "my-nvim");
    let run = sandbox.run_at_terminal(&[], &[("ORCA_TERM_NVIM", other.to_str().unwrap())]);
    assert_eq!(run.code, Some(0), "{}", run.screen);
    assert_eq!(
        sandbox.recorded("my-nvim").as_deref(),
        Some(sandbox.opened_with_sidebar().as_str())
    );
    assert_eq!(
        sandbox.recorded("nvim"),
        None,
        "the nvim on PATH is not used"
    );

    let missing = sandbox.tmp.path().join("no-such-nvim");
    let out = sandbox.run(&["open"], &[("ORCA_TERM_NVIM", missing.to_str().unwrap())]);
    assert_eq!(out.status.code(), Some(9));
    assert!(
        stderr(&out).contains("not an executable file (set by ORCA_TERM_NVIM)"),
        "{}",
        stderr(&out)
    );
}
