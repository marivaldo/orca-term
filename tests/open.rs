//! `orca-term` with no arguments and `orca-term open` (#148), driven as a black box (seam 1): a
//! fake `nvim`, a shell script on an isolated PATH, records the arguments it was opened with, so
//! no test ever runs a real Neovim. Every run gets a cleared environment holding only that PATH,
//! an `XDG_STATE_HOME` inside the sandbox and what the test sets, so the developer's own `NVIM`,
//! `ORCA_TERM_NVIM` or project list never leaks in. A pseudo-terminal stands in for a person at a
//! terminal.
//!
//! The launcher also adds the repository of the current directory to the project list (#152):
//! those tests put the real `git` on the isolated PATH and run the binary in a throwaway
//! repository.

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
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use assert_fs::TempDir;
use support::Repo;

/// What the binary prints first in its usage.
const USAGE: &str = "Usage: orca-term <COMMAND>";

/// What `orca-term open` says inside Neovim.
const ALREADY_INSIDE: &str = "orca-term: already inside Neovim; run :OrcaTerm sidebar\n";

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

    /// The project list's file.
    fn projects_file(&self) -> PathBuf {
        self.state().join("orca-term").join("projects.json")
    }

    /// What the project list's file holds, or `None` when nothing was ever added.
    fn projects(&self) -> Option<String> {
        std::fs::read_to_string(self.projects_file()).ok()
    }

    /// Writes `contents` as the project list's file.
    fn write_projects(&self, contents: &str) {
        std::fs::create_dir_all(self.projects_file().parent().unwrap()).unwrap();
        std::fs::write(self.projects_file(), contents).unwrap();
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
        opened_with_sidebar_in(&self.work())
    }

    /// The binary with `args` and `vars`, its stdin, stdout and stderr not terminals.
    fn run(&self, args: &[&str], vars: &[(&str, &str)]) -> Output {
        self.run_in(&self.work(), args, vars)
    }

    /// The same, run in `dir`.
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

    /// The binary with `args` and `vars` on a pseudo-terminal, as a person types it in a shell.
    fn run_at_terminal(&self, args: &[&str], vars: &[(&str, &str)]) -> TerminalRun {
        self.run_at_terminal_in(&self.work(), args, vars)
    }

    /// The same, run in `dir`.
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

/// What a fake `nvim` must record when it was opened with `-c 'OrcaTerm sidebar'` in `dir`.
fn opened_with_sidebar_in(dir: &Path) -> String {
    let dir = dir.canonicalize().unwrap();
    format!("-c\nOrcaTerm sidebar\ncwd={}\n", dir.display())
}

/// What the project list's file holds when the project at `path` is the only one.
fn only_project(path: &Path) -> String {
    format!(
        r#"{{"schema":1,"projects":[{{"path":"{}"}}]}}"#,
        path.display()
    )
}

/// A sandbox holding the fake `nvim` and the real `git`, and a repository to run the binary in.
fn sandbox_and_repository() -> (Sandbox, Repo) {
    let sandbox = Sandbox::new();
    sandbox.fake_nvim();
    sandbox.link_git();
    (sandbox, Repo::new())
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
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "",
        "nothing on stdout"
    );
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

#[test]
fn no_arguments_inside_a_repository_add_its_primary_checkout_and_open_nvim() {
    let (sandbox, repo) = sandbox_and_repository();
    let run = sandbox.run_at_terminal_in(&repo.root, &[], &[]);
    assert_eq!(run.code, Some(0), "{}", run.screen);
    assert_eq!(run.screen, "", "nothing is said");
    assert_eq!(
        sandbox.projects().as_deref(),
        Some(only_project(&repo.root).as_str())
    );
    assert_eq!(
        sandbox.recorded("nvim").as_deref(),
        Some(opened_with_sidebar_in(&repo.root).as_str())
    );
}

#[test]
fn no_arguments_from_a_linked_worktree_add_the_primary_checkout() {
    let (sandbox, repo) = sandbox_and_repository();
    let worktree = repo.add_worktree(&repo.tmp.path().join("fix"), "fix");
    let run = sandbox.run_at_terminal_in(&worktree, &[], &[]);
    assert_eq!(run.code, Some(0), "{}", run.screen);
    assert_eq!(
        sandbox.projects().as_deref(),
        Some(only_project(&repo.root).as_str())
    );
    assert_eq!(
        sandbox.recorded("nvim").as_deref(),
        Some(opened_with_sidebar_in(&worktree).as_str())
    );
}

#[test]
fn no_arguments_outside_a_repository_add_nothing_and_open_nvim() {
    let sandbox = Sandbox::new();
    sandbox.fake_nvim();
    sandbox.link_git();
    let run = sandbox.run_at_terminal(&[], &[]);
    assert_eq!(run.code, Some(0), "{}", run.screen);
    assert_eq!(run.screen, "", "nothing is said");
    assert_eq!(sandbox.projects(), None, "nothing was added");
    assert_eq!(
        sandbox.recorded("nvim").as_deref(),
        Some(sandbox.opened_with_sidebar().as_str())
    );
}

#[test]
fn the_usage_adds_nothing() {
    let (sandbox, repo) = sandbox_and_repository();
    let run = sandbox.run_at_terminal_in(&repo.root, &[], &[("NVIM", "/tmp/nvim.sock")]);
    assert_eq!(run.code, Some(2), "{}", run.screen);
    assert!(run.screen.contains(USAGE), "{}", run.screen);
    let out = sandbox.run_in(&repo.root, &[], &[]);
    assert_eq!(out.status.code(), Some(2));
    assert!(stderr(&out).contains(USAGE), "{}", stderr(&out));
    assert_eq!(sandbox.projects(), None, "nothing was added");
    assert_eq!(sandbox.recorded("nvim"), None, "nvim must not run");
}

#[test]
fn open_inside_a_repository_adds_it_too() {
    let (sandbox, repo) = sandbox_and_repository();
    let out = sandbox.run_in(&repo.root, &["open"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(String::from_utf8_lossy(&out.stdout), "", "nothing is said");
    assert_eq!(stderr(&out), "", "nothing is said");
    assert_eq!(
        sandbox.projects().as_deref(),
        Some(only_project(&repo.root).as_str())
    );
    assert_eq!(
        sandbox.recorded("nvim").as_deref(),
        Some(opened_with_sidebar_in(&repo.root).as_str())
    );
}

#[test]
fn an_open_that_is_refused_and_a_missing_nvim_add_nothing() {
    let (sandbox, repo) = sandbox_and_repository();
    let out = sandbox.run_in(&repo.root, &["open"], &[("NVIM", "/tmp/nvim.sock")]);
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(stderr(&out), ALREADY_INSIDE);
    assert_eq!(sandbox.recorded("nvim"), None, "nvim must not run");

    std::fs::remove_file(sandbox.bin().join("nvim")).unwrap();
    let run = sandbox.run_at_terminal_in(&repo.root, &[], &[]);
    assert_eq!(run.code, Some(9), "{}", run.screen);
    let out = sandbox.run_in(&repo.root, &["open"], &[]);
    assert_eq!(out.status.code(), Some(9));
    assert_eq!(sandbox.projects(), None, "nothing was added");
}

#[test]
fn a_repository_already_on_the_list_changes_nothing_and_says_nothing() {
    let (sandbox, repo) = sandbox_and_repository();
    let before = format!(
        r#"{{"schema":1,"projects":[{{"path":"{}"}},{{"path":"/somewhere/else"}}]}}"#,
        repo.root.display()
    );
    sandbox.write_projects(&before);
    let run = sandbox.run_at_terminal_in(&repo.root, &[], &[]);
    assert_eq!(run.code, Some(0), "{}", run.screen);
    assert_eq!(run.screen, "", "nothing is said");
    assert_eq!(sandbox.projects().as_deref(), Some(before.as_str()));
    assert_eq!(
        sandbox.recorded("nvim").as_deref(),
        Some(opened_with_sidebar_in(&repo.root).as_str())
    );
}

#[test]
fn a_broken_project_list_or_a_newer_schema_adds_nothing_and_still_opens_nvim() {
    for unreadable in ["not json", r#"{"schema":99,"projects":[]}"#] {
        let (sandbox, repo) = sandbox_and_repository();
        sandbox.write_projects(unreadable);
        let run = sandbox.run_at_terminal_in(&repo.root, &[], &[]);
        assert_eq!(run.code, Some(0), "{}", run.screen);
        assert_eq!(run.screen, "", "nothing is said");
        assert_eq!(
            sandbox.projects().as_deref(),
            Some(unreadable),
            "the file is not written over"
        );
        assert_eq!(
            sandbox.recorded("nvim").as_deref(),
            Some(opened_with_sidebar_in(&repo.root).as_str())
        );
    }
}

#[test]
fn a_held_lock_adds_nothing_and_still_opens_nvim() {
    let (sandbox, repo) = sandbox_and_repository();
    let lock = sandbox.state().join("orca-term").join("projects.lock");
    std::fs::create_dir_all(lock.parent().unwrap()).unwrap();
    let held = std::fs::File::create(&lock).unwrap();
    held.lock().unwrap();
    let out = sandbox.run_in(&repo.root, &["open"], &[]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stderr(&out), "", "nothing is said");
    assert_eq!(sandbox.projects(), None, "nothing was added");
    assert_eq!(
        sandbox.recorded("nvim").as_deref(),
        Some(opened_with_sidebar_in(&repo.root).as_str())
    );
}

#[test]
fn a_missing_git_adds_nothing_and_still_opens_nvim() {
    let sandbox = Sandbox::new();
    sandbox.fake_nvim();
    let repo = Repo::new();
    let run = sandbox.run_at_terminal_in(&repo.root, &[], &[]);
    assert_eq!(run.code, Some(0), "{}", run.screen);
    assert_eq!(run.screen, "", "nothing is said");
    assert_eq!(sandbox.projects(), None, "nothing was added");
    assert_eq!(
        sandbox.recorded("nvim").as_deref(),
        Some(opened_with_sidebar_in(&repo.root).as_str())
    );
}
