//! Throwaway git repositories for black-box tests of the core.
//!
//! Every git process, ours and the binary's, runs against a gitconfig of the fixture's own, so
//! tests never read the developer's global config and behave the same on CI. The binary also gets
//! a `HOME` and an `XDG_CONFIG_HOME` inside the fixture, so it never reads the developer's global
//! orca-term config nor places worktrees under the developer's home.

use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use assert_cmd::cargo::cargo_bin_cmd;
use assert_fs::TempDir;
use assert_fs::prelude::*;

#[derive(Debug)]
pub(crate) struct Repo {
    pub(crate) tmp: TempDir,
    pub(crate) root: PathBuf,
    /// The binary's `HOME`.
    pub(crate) home: PathBuf,
    /// The binary's `XDG_CONFIG_HOME`.
    pub(crate) xdg: PathBuf,
    gitconfig: PathBuf,
}

impl Repo {
    /// A repository with one commit on `main`, at `<tmp>/repo`.
    pub(crate) fn new() -> Self {
        Self::in_dir(TempDir::new().unwrap())
    }

    /// The same, its temporary directory made inside `dir`: for a test whose fixture must not
    /// sit in the system's temporary directory, which opencode's sandbox lets it write (#35).
    pub(crate) fn new_in(dir: &Path) -> Self {
        Self::in_dir(TempDir::new_in(dir).unwrap())
    }

    fn in_dir(tmp: TempDir) -> Self {
        let gitconfig = tmp.child("gitconfig");
        gitconfig
            .write_str("[user]\n\tname = Fixture\n\temail = fixture@example.invalid\n[init]\n\tdefaultBranch = main\n")
            .unwrap();
        let root = tmp.child("repo");
        root.create_dir_all().unwrap();
        let home = tmp.child("home");
        home.create_dir_all().unwrap();
        let xdg = tmp.child("xdg");
        xdg.create_dir_all().unwrap();
        let repo = Self {
            root: root.path().canonicalize().unwrap(),
            home: home.path().canonicalize().unwrap(),
            xdg: xdg.path().canonicalize().unwrap(),
            gitconfig: gitconfig.path().to_owned(),
            tmp,
        };
        repo.git(&["init", "--quiet"]);
        repo.git(&["commit", "--quiet", "--allow-empty", "-m", "init"]);
        repo
    }

    /// Runs git in the primary checkout and panics on failure.
    pub(crate) fn git(&self, args: &[&str]) -> String {
        let out = self
            .env(Command::new("git").arg("-C").arg(&self.root).args(args))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// Writes `contents` to `rel` under the primary checkout, creating parent directories.
    pub(crate) fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// Stages everything and commits it.
    pub(crate) fn commit_all(&self, message: &str) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "-m", message]);
    }

    /// Adds a worktree by hand, as a person would with plain git, on a new branch.
    pub(crate) fn add_worktree(&self, path: &Path, branch: &str) -> PathBuf {
        self.git(&[
            "worktree",
            "add",
            "--quiet",
            "-b",
            branch,
            path.to_str().unwrap(),
        ]);
        path.canonicalize().unwrap()
    }

    /// The `orca-term` binary, run from `dir` with the fixture's git environment.
    pub(crate) fn orca_term(&self, dir: &Path) -> assert_cmd::Command {
        let mut cmd = cargo_bin_cmd!("orca-term");
        cmd.current_dir(dir);
        self.env_assert(&mut cmd);
        cmd
    }

    /// The `orca-term` binary started in the background from `dir` with `args` and the fixture's
    /// environment, for tests that act while it runs. Its output is discarded.
    pub(crate) fn spawn_orca_term(&self, dir: &Path, args: &[&str]) -> Child {
        self.command(dir)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap()
    }

    /// The `orca-term` binary as a plain `Command`, to run from `dir` with the fixture's
    /// environment, for tests that drive it themselves.
    pub(crate) fn command(&self, dir: &Path) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_orca-term"));
        cmd.current_dir(dir)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.xdg);
        self.env(&mut cmd);
        cmd
    }

    /// The `orca-term` binary with `args`, to run from `dir` with the fixture's environment on a
    /// pseudo-terminal, as a person at a terminal runs it: spawn it on the `Pts` of
    /// `pty_process::blocking::open()`.
    pub(crate) fn pty_command(&self, dir: &Path, args: &[&str]) -> pty_process::blocking::Command {
        pty_process::blocking::Command::new(env!("CARGO_BIN_EXE_orca-term"))
            .args(args)
            .current_dir(dir)
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.xdg)
            .env("GIT_CONFIG_GLOBAL", &self.gitconfig)
            .env("GIT_CONFIG_NOSYSTEM", "1")
    }

    fn env<'a>(&self, cmd: &'a mut Command) -> &'a mut Command {
        cmd.env("GIT_CONFIG_GLOBAL", &self.gitconfig)
            .env("GIT_CONFIG_NOSYSTEM", "1")
    }

    fn env_assert(&self, cmd: &mut assert_cmd::Command) {
        cmd.env("GIT_CONFIG_GLOBAL", &self.gitconfig)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.xdg);
    }
}

/// What a run of the binary at a terminal printed (stdout and stderr together, as the terminal
/// shows them) and its exit code.
pub(crate) struct TerminalRun {
    pub(crate) screen: String,
    pub(crate) code: Option<i32>,
}

/// Runs `command` (made by [`Repo::pty_command`]) on a pseudo-terminal, waits for `prompt` on the
/// screen (at most 30 seconds), types `answer` and a newline, and waits for it to exit.
pub(crate) fn run_at_terminal(
    command: pty_process::blocking::Command,
    prompt: &str,
    answer: &str,
) -> TerminalRun {
    let (pty, pts) = pty_process::blocking::open().unwrap();
    let mut child = command.spawn(pts).unwrap();
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
