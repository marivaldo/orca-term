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

use std::io::{Read as _, Write as _};
use std::process::ExitStatus;
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
        let (pty, pts) = pty_process::blocking::open().unwrap();
        pty.resize(pty_process::Size::new(ROWS, COLS)).unwrap();
        let template = fx.command(&["app"]);
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
    app.wait_for("no agent running");
    fx.ok(&["agent", "start", "fix"]);
    app.wait_for("fake claude ready");
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
    app.wait_for("no agent running");
    app.type_in(b"\r");
    app.wait_for("fake claude ready");
    assert!(holder_alive(&admin));
    app.type_in(b"\x01q");
    assert!(app.wait_ended().success());
}
