//! `orca-term app`, the terminal application (#174), run for real in a terminal of its own (a
//! PTY), with the fake `claude` of the agent suites as the agent: what the person sees is read
//! back through a terminal emulator, as their terminal would show it.

#![expect(
    clippy::unwrap_used,
    reason = "clippy.toml relaxes unwrap only inside #[test] fns; fixture helpers panic on failure too"
)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "a black-box test builds real repositories and files around the binary it drives"
)]

#[path = "support/agent.rs"]
#[expect(
    dead_code,
    reason = "the fixture is shared between suites, and this one leaves part of it unused"
)]
mod fixture;
#[expect(
    dead_code,
    reason = "the fixture is shared between suites, and this one leaves part of it unused"
)]
mod support;

use std::fs;
use std::io::{Read as _, Write as _};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use fixture::{AgentRepo, holder_alive, wait_until};
use rustix::process::{Pid, Signal};

const ROWS: u16 = 40;
const COLS: u16 = 150;

/// `orca-term app` running in a PTY: everything it wrote, raw and as a screen.
struct Running {
    pty: Arc<pty_process::blocking::Pty>,
    child: std::process::Child,
    raw: Arc<Mutex<Vec<u8>>>,
    screen: Arc<Mutex<vt100::Parser>>,
}

impl Running {
    /// Starts `orca-term app` as `fx` runs the binary, in a terminal `ROWS` by `COLS`.
    fn start(fx: &AgentRepo) -> Self {
        Self::start_as(&fx.command(&["app"]))
    }

    /// Starts `orca-term app` as `template` says, in a terminal `ROWS` by `COLS`.
    fn start_as(template: &std::process::Command) -> Self {
        let (pty, pts) = pty_process::blocking::open().unwrap();
        pty.resize(pty_process::Size::new(ROWS, COLS)).unwrap();
        let mut command = pty_process::blocking::Command::new(template.get_program())
            .args(template.get_args())
            .current_dir(template.get_current_dir().unwrap());
        for (key, value) in template.get_envs() {
            command = match value {
                Some(value) => command.env(key, value),
                None => command.env_remove(key),
            };
        }
        let child = command.spawn(pts).unwrap();
        let raw = Arc::new(Mutex::new(Vec::new()));
        let screen = Arc::new(Mutex::new(vt100::Parser::new(
            ROWS.try_into().unwrap(),
            COLS.try_into().unwrap(),
            0,
        )));
        let pty = Arc::new(pty);
        let reader = Arc::clone(&pty);
        let (sink, shown) = (Arc::clone(&raw), Arc::clone(&screen));
        thread::spawn(move || {
            let mut chunk = [0u8; 16 * 1024];
            // The PTY answers 0 (macOS) or an error (Linux) once the application is gone.
            while let Ok(n @ 1..) = (&*reader).read(&mut chunk) {
                sink.lock().unwrap().extend_from_slice(&chunk[..n]);
                shown.lock().unwrap().process(&chunk[..n]);
            }
        });
        Self {
            pty,
            child,
            raw,
            screen,
        }
    }

    /// What the terminal shows now.
    fn shown(&self) -> String {
        self.screen.lock().unwrap().screen().contents()
    }

    /// Waits until the terminal shows `text`.
    fn wait_for(&self, text: &str) {
        let mut last = String::new();
        let found = (0..750).any(|_| {
            last = self.shown();
            let found = last.contains(text);
            if !found {
                thread::sleep(Duration::from_millis(20));
            }
            found
        });
        assert!(found, "`{text}` never showed; the screen:\n{last}");
    }

    /// Types `bytes` at the terminal.
    fn type_in(&mut self, bytes: &[u8]) {
        (&*self.pty).write_all(bytes).unwrap();
        (&*self.pty).flush().unwrap();
    }

    /// Waits for the application to end.
    fn wait_ended(&mut self) -> ExitStatus {
        let mut status = None;
        wait_until("the application ending", || {
            status = self.child.try_wait().unwrap();
            status.is_some()
        });
        status.unwrap()
    }

    fn raw(&self) -> Vec<u8> {
        self.raw.lock().unwrap().clone()
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// The last place `needle` occurs in `haystack`.
fn last_at(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .rposition(|window| window == needle)
}

#[test]
fn the_application_shows_the_projects_and_types_into_the_agent_then_quits_leaving_it_running() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    fx.new_worktree("docs");
    let admin = fx.admin_dir(&worktree);
    fx.ok(&["project", "add"]);
    fx.ok(&["agent", "start", "fix"]);
    fx.wait_for_output("fix", "fake claude ready");

    let mut app = Running::start(&fx);
    app.wait_for("Projects");
    app.wait_for("docs");
    app.type_in(b"\x01k");
    app.wait_for("fake claude ready");
    app.wait_for("repo/fix");
    app.type_in(b"hello\r");
    app.wait_for("you said: hello");

    app.type_in(b"\x01q");
    assert!(app.wait_ended().success());
    let raw = app.raw();
    let entered = last_at(&raw, b"\x1b[?1049h").unwrap();
    let left = last_at(&raw, b"\x1b[?1049l");
    assert!(left > Some(entered), "the alternate screen was left");
    assert!(holder_alive(&admin), "quitting stops no agent");
}

#[test]
fn a_signal_ends_the_application_and_restores_the_terminal() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");
    fx.ok(&["project", "add"]);

    for signal in [Signal::TERM, Signal::HUP] {
        let mut app = Running::start(&fx);
        app.wait_for("no agent running");
        let pid = Pid::from_raw(i32::try_from(app.child.id()).unwrap()).unwrap();
        rustix::process::kill_process(pid, signal).unwrap();
        assert!(app.wait_ended().success(), "{signal:?}");
        let raw = app.raw();
        let entered = last_at(&raw, b"\x1b[?1049h").unwrap();
        assert!(last_at(&raw, b"\x1b[?1049l") > Some(entered), "{signal:?}");
        assert!(
            last_at(&raw, b"\x1b[?25h") > Some(entered),
            "the cursor shows again after {signal:?}"
        );
    }
}

#[test]
fn an_agent_started_elsewhere_shows_up_on_its_own() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");
    fx.ok(&["project", "add"]);

    let mut app = Running::start(&fx);
    // Started in the primary checkout, which it opens (#175): `fix` comes after it.
    app.wait_for("repo/repo");
    app.type_in(b"\x01j");
    app.wait_for("repo/fix");
    app.wait_for("no agent running");
    fx.ok(&["agent", "start", "fix"]);
    app.wait_for("fake claude ready");
    app.type_in(b"\x01q");
    assert!(app.wait_ended().success());
}

/// The stand-in Neovim: records its arguments and its pid beside the fake claude's log, says
/// where it runs, and echoes every line typed. It never starts a real Neovim.
const FAKE_NVIM: &str = r#"#!/bin/bash
printf '%s\n' "$@" > "$ORCA_TERM_FAKE_CLAUDE_LOG.nvim-args"
echo $$ > "$ORCA_TERM_FAKE_CLAUDE_LOG.nvim-pid"
pwd -P > "$ORCA_TERM_FAKE_CLAUDE_LOG.nvim-cwd"
echo "fake nvim ready"
while IFS= read -r line; do echo "nvim got: $line"; done
"#;

/// The stand-in shell, run as `$SHELL`.
const FAKE_SHELL: &str = r#"#!/bin/bash
echo $$ > "$ORCA_TERM_FAKE_CLAUDE_LOG.shell-pid"
echo "fake shell ready"
while IFS= read -r line; do echo "shell got: $line"; done
"#;

/// Whether the process `pid`, read from the file at `path`, still runs.
fn alive(path: &Path) -> bool {
    let pid: i32 = fs::read_to_string(path).unwrap().trim().parse().unwrap();
    rustix::process::test_kill_process(Pid::from_raw(pid).unwrap()).is_ok()
}

/// A short directory under `/tmp` for the application's sockets, as `XDG_RUNTIME_DIR`, removed
/// when dropped. Each is its own, so tests running at once never remove another's.
struct RuntimeDir(PathBuf);

/// How many runtime directories this test process made, to name the next one.
static RUNTIME_DIRS: AtomicUsize = AtomicUsize::new(0);

impl RuntimeDir {
    fn new() -> Self {
        let n = RUNTIME_DIRS.fetch_add(1, Ordering::SeqCst);
        let dir = PathBuf::from(format!("/tmp/ot-app-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for RuntimeDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn the_nvim_tab_runs_neovim_in_the_worktree_registered_and_quitting_ends_it_and_the_shell() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    fx.ok(&["project", "add"]);
    fx.write_tool("nvim", FAKE_NVIM);
    fx.write_tool("fake-shell", FAKE_SHELL);
    let runtime = RuntimeDir::new();
    let mut command = fx.command(&["app"]);
    command
        .env("XDG_RUNTIME_DIR", &runtime.0)
        .env("SHELL", fx.bin.join("fake-shell"));

    let mut app = Running::start_as(&command);
    // Started in the primary checkout, which it opens (#175): `fix` comes after it.
    app.wait_for("repo/repo");
    app.type_in(b"\x01j");
    app.wait_for("repo/fix");
    app.type_in(b"\x012");
    app.wait_for("fake nvim ready");
    app.type_in(b"hello\r");
    app.wait_for("nvim got: hello");

    let log = fx.log.display().to_string();
    let cwd = fs::read_to_string(format!("{log}.nvim-cwd")).unwrap();
    assert_eq!(Path::new(cwd.trim()), worktree.canonicalize().unwrap());
    let args = fs::read_to_string(format!("{log}.nvim-args")).unwrap();
    let args: Vec<&str> = args.lines().collect();
    assert_eq!(args[0], "--listen", "{args:?}");
    let socket = Path::new(args[1]);
    assert!(
        socket.starts_with(runtime.0.join("orca-term")),
        "{socket:?}"
    );
    assert!(socket.as_os_str().len() <= 103, "fits a socket address");

    // The stand-in cannot listen: the test does it for it, as Neovim would.
    let listener = UnixListener::bind(socket).unwrap();
    let registry = fx.repo.root.join(".git/orca-term/nvim");
    let registered = || {
        fs::read_dir(&registry).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| fs::read(entry.path()).unwrap() == args[1].as_bytes())
        })
    };
    wait_until("the Neovim registering", registered);

    app.type_in(b"\x013");
    app.wait_for("fake shell ready");
    let pids = (
        PathBuf::from(format!("{log}.nvim-pid")),
        PathBuf::from(format!("{log}.shell-pid")),
    );
    assert!(alive(&pids.0) && alive(&pids.1));

    app.type_in(b"\x01q");
    assert!(app.wait_ended().success());
    assert!(!alive(&pids.0), "quitting ends Neovim");
    assert!(!alive(&pids.1), "and the shell");
    assert!(!registered(), "and takes Neovim off the registry");
    assert!(!socket.exists(), "and removes its socket");
    drop(listener);
}

/// Listens at `socket` as Neovim would, and returns the first request made there, read until it
/// holds `until`, after answering it as Neovim answers.
fn first_request(socket: &Path, until: &str) -> String {
    let listener = UnixListener::bind(socket).unwrap();
    listener.set_nonblocking(true).unwrap();
    let mut accepted = None;
    wait_until("a request to Neovim", || {
        accepted = listener.accept().ok();
        accepted.is_some()
    });
    let (mut stream, _) = accepted.unwrap();
    stream.set_nonblocking(false).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut request = Vec::new();
    let mut chunk = [0u8; 4096];
    while !String::from_utf8_lossy(&request).contains(until) {
        let n = stream.read(&mut chunk).unwrap();
        assert!(n > 0, "{:?}", String::from_utf8_lossy(&request));
        request.extend_from_slice(&chunk[..n]);
    }
    stream.write_all(&[0x94, 0x01, 0x01, 0xc0, 0xc0]).unwrap();
    String::from_utf8_lossy(&request).into_owned()
}

#[test]
fn the_right_column_lists_the_changed_files_and_enter_opens_a_diff_in_the_worktrees_neovim() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    fs::write(worktree.join("login.rs"), "one\ntwo\nthree\n").unwrap();
    // Through the fixture's git config, which gives the commit an author on any machine.
    let at = worktree.to_str().unwrap();
    fx.repo.git(&["-C", at, "add", "login.rs"]);
    fx.repo.git(&["-C", at, "commit", "--quiet", "-m", "login"]);
    fs::write(worktree.join("notes.txt"), "a\nb\n").unwrap();
    fx.ok(&["project", "add"]);
    fx.write_tool("nvim", FAKE_NVIM);
    let runtime = RuntimeDir::new();
    let mut command = fx.command(&["app"]);
    command.env("XDG_RUNTIME_DIR", &runtime.0);

    let mut app = Running::start_as(&command);
    app.wait_for("repo/repo");
    app.type_in(b"\x01j");
    app.wait_for("changes  2");
    app.wait_for("login.rs");
    app.wait_for("notes.txt");
    app.wait_for("↑1 ahead");

    app.type_in(b"\x01l");
    app.wait_for("files");
    app.type_in(b"j\r");
    app.wait_for("fake nvim ready");

    // The stand-in cannot listen: the test does it for it, as Neovim would, and reads what the
    // application asks once the Neovim is registered.
    let log = fx.log.display().to_string();
    let args = fs::read_to_string(format!("{log}.nvim-args")).unwrap();
    let socket = PathBuf::from(args.lines().nth(1).unwrap());
    let text = first_request(&socket, "notes.txt");
    assert!(text.contains("diff_from_core"), "{text}");
    assert!(
        text.contains(worktree.canonicalize().unwrap().to_str().unwrap()),
        "{text}"
    );

    app.type_in(b"\x01q");
    assert!(app.wait_ended().success());
}

#[test]
fn enter_starts_the_agent_of_a_worktree_with_none() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    fx.ok(&["project", "add"]);

    let mut app = Running::start(&fx);
    app.wait_for("repo/repo");
    app.type_in(b"\x01j");
    app.wait_for("repo/fix");
    app.wait_for("no agent running");
    app.type_in(b"\r");
    app.wait_for("fake claude ready");
    assert!(holder_alive(&admin));
    app.type_in(b"\x01q");
    assert!(app.wait_ended().success());
}

#[test]
fn a_failed_agent_shows_its_card_its_log_and_r_resumes_it() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");
    fx.ok(&["project", "add"]);

    let mut app = Running::start(&fx);
    app.wait_for("repo/repo");
    app.type_in(b"\x01j");
    app.wait_for("no agent running");
    app.type_in(b"\r");
    app.wait_for("fake claude ready");
    app.type_in(b"exit 3\r");
    app.wait_for("agent stopped");
    app.wait_for("exit 3 after");
    app.wait_for("resume session");

    app.type_in(b"l");
    app.wait_for("agent log");
    app.wait_for("bye");
    app.type_in(b"\x1b");
    app.wait_for("resume session");

    app.type_in(b"r");
    wait_until("the card going once the agent runs again", || {
        !app.shown().contains("agent stopped")
    });
    app.wait_for("fake claude ready");
    app.type_in(b"again\r");
    app.wait_for("you said: again");
    app.type_in(b"\x01q");
    assert!(app.wait_ended().success());
}

#[test]
fn the_picker_opens_a_worktree_by_name_and_n_creates_one() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");
    fx.ok(&["project", "add"]);

    let mut app = Running::start(&fx);
    app.wait_for("repo/repo");
    app.type_in(b"\x01w");
    app.wait_for("switch worktree");
    app.type_in(b"fi\r");
    app.wait_for("repo/fix");

    app.type_in(b"\x01h");
    app.type_in(b"n");
    app.wait_for("new worktree in repo");
    app.type_in(b"made-here\r");
    app.wait_for("repo/made-here");
    assert!(
        fx.repo
            .home
            .join("orca-term/worktrees/repo/made-here")
            .is_dir()
    );
    app.type_in(b"\x01q");
    assert!(app.wait_ended().success());
}

#[test]
fn a_draft_pull_request_of_the_primary_checkout_on_the_default_branch_is_refused() {
    let fx = AgentRepo::new();
    fx.ok(&["project", "add"]);

    let mut app = Running::start(&fx);
    app.wait_for("repo/repo");
    app.type_in(b"\x01P");
    app.wait_for("the repository's default branch");
    app.type_in(b"\x01q");
    assert!(app.wait_ended().success());
}

#[test]
fn started_in_the_primary_checkout_it_opens_it_tagged_and_enter_starts_its_agent() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");
    let admin = fx.admin_dir(&fx.repo.root);
    fx.ok(&["project", "add"]);

    let mut app = Running::start(&fx);
    app.wait_for("repo  primary");
    app.wait_for("repo/repo");
    app.wait_for("no agent running");
    app.type_in(b"\r");
    app.wait_for("fake claude ready");
    assert!(
        holder_alive(&admin),
        "its agent runs in the primary checkout"
    );
    app.type_in(b"\x01q");
    assert!(app.wait_ended().success());
}
