//! A fake `opencode` on the isolated PATH of `support/agent.rs`, for black-box tests of opencode's
//! launch, events and swap (seam 1, #35), with fake sandbox tools that record their arguments and
//! run what they wrap, so the tests need no opencode, no sandbox and no network.
//!
//! The fake writes events to the pipe the holder hands it exactly as the plugin would: typed
//! `send <n> <line>` through an attached terminal, it writes `<line>` to the file descriptor
//! `ORCA_TERM_EVENTS_FD` names and says `sent #<n>.`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use serde_json::Value;

use crate::fixture::{AgentRepo, Attach};
use crate::support::Repo;

/// The fake `opencode`: answers `--version`, answers `session list` with `<log>.sessions` as the
/// test wrote it (nothing when it did not, which is no list) and its arguments in
/// `<log>.session-list`, records its arguments, directory, environment variable names (never
/// values) and the value of `OPENCODE_CONFIG_DIR` (ours), runs a test's prelude, says it is ready,
/// then writes the events it is told to send, echoes anything else, exits on `exit <code>`, and
/// ends on `SIGHUP`. Bash builtins and absolute paths only.
const FAKE_OPENCODE: &str = r#"#!/bin/bash
if [ "$1" = "--version" ]; then echo "VERSION"; exit 0; fi
log="$ORCA_TERM_FAKE_OPENCODE_LOG"
if [ "$1" = "session" ]; then
  printf '%s\0' "$@" > "$log.session-list"
  if [ -f "$log.sessions" ]; then printf '%s\n' "$(< "$log.sessions")"; fi
  exit 0
fi
printf '%s\0' "$@" > "$log.argv"
pwd > "$log.cwd"
compgen -e > "$log.env"
printf '%s' "$OPENCODE_CONFIG_DIR" > "$log.config"
echo start >> "$log.starts"
trap 'echo "got HUP"; exit 129' HUP
PRELUDE
echo "fake opencode ready"
while IFS= read -r -t 120 line; do
  case "$line" in
    exit*) echo "bye"; exit "${line#exit }" ;;
    send\ *) rest="${line#send }"
             printf '%s\n' "${rest#* }" >&"$ORCA_TERM_EVENTS_FD"
             echo "sent #${rest%% *}." ;;
    *) echo "you said: $line" ;;
  esac
done
exit 0
"#;

/// The fake `sandbox-exec`: records its arguments, then runs what follows `-f <profile>`.
const FAKE_SANDBOX_EXEC: &str = r#"#!/bin/bash
printf '%s\0' "$@" > "$ORCA_TERM_FAKE_OPENCODE_LOG.sandbox"
shift 2
exec "$@"
"#;

/// The fake `bwrap`: records its arguments, then runs what follows `--`.
const FAKE_BWRAP: &str = r#"#!/bin/bash
printf '%s\0' "$@" > "$ORCA_TERM_FAKE_OPENCODE_LOG.sandbox"
while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do shift; done
shift
exec "$@"
"#;

/// The agent fixture with the fake opencode and the fake sandbox tools that run it.
pub(crate) struct OpenCodeRepo {
    pub(crate) fx: AgentRepo,
    /// The prefix of the fake opencode's log files (`<log>.argv`, ...).
    pub(crate) log: PathBuf,
}

impl OpenCodeRepo {
    pub(crate) fn new() -> Self {
        Self::with(AgentRepo::new())
    }

    /// The same, its repository made inside `dir` (`Repo::new_in`).
    pub(crate) fn new_in(dir: &Path) -> Self {
        Self::with(AgentRepo::with_repo(Repo::new_in(dir)))
    }

    fn with(fx: AgentRepo) -> Self {
        let log = fx.repo.tmp.path().join("fake-opencode");
        let repo = Self { fx, log };
        repo.write_opencode("1.18.0", "");
        repo.fx.write_tool("sandbox-exec", FAKE_SANDBOX_EXEC);
        repo.fx.write_tool("bwrap", FAKE_BWRAP);
        repo
    }

    /// Writes the fake `opencode`, answering `version` to `--version` and running `prelude`
    /// (bash) before it says it is ready.
    pub(crate) fn write_opencode(&self, version: &str, prelude: &str) {
        let script = FAKE_OPENCODE
            .replace("VERSION", version)
            .replace("PRELUDE", prelude);
        self.fx.write_tool("opencode", &script);
    }

    /// The binary as a `Command`, as the agent fixture runs it, telling the fake where to log.
    pub(crate) fn command(&self, args: &[&str]) -> Command {
        let mut cmd = self.fx.command(args);
        cmd.env("ORCA_TERM_FAKE_OPENCODE_LOG", &self.log);
        cmd
    }

    pub(crate) fn run(&self, args: &[&str]) -> Output {
        self.command(args).output().unwrap()
    }

    /// Runs the binary with `args`, failing the test unless it succeeded, and returns its stdout.
    pub(crate) fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert!(
            out.status.success(),
            "{args:?} failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// The arguments the fake opencode was last started with, after its own name.
    pub(crate) fn argv(&self) -> Vec<String> {
        split_nul(&fs::read(self.log.with_extension("argv")).unwrap())
    }

    /// Makes the fake opencode's `session list` print `listed`.
    pub(crate) fn list_sessions(&self, listed: &Value) {
        fs::write(self.log.with_extension("sessions"), listed.to_string()).unwrap();
    }

    /// The arguments the fake opencode's `session list` was last run with, `None` when it never
    /// was.
    pub(crate) fn session_list_argv(&self) -> Option<Vec<String>> {
        fs::read(self.log.with_extension("session-list"))
            .ok()
            .map(|raw| split_nul(&raw))
    }

    /// The arguments the fake sandbox tool was last given.
    pub(crate) fn sandbox_argv(&self) -> Vec<String> {
        split_nul(&fs::read(self.log.with_extension("sandbox")).unwrap())
    }

    /// The names of the variables the fake opencode was last started with.
    pub(crate) fn env_names(&self) -> Vec<String> {
        fs::read_to_string(self.log.with_extension("env"))
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The directory the fake opencode was last started in.
    pub(crate) fn cwd(&self) -> PathBuf {
        PathBuf::from(
            fs::read_to_string(self.log.with_extension("cwd"))
                .unwrap()
                .trim_end(),
        )
    }

    /// `OPENCODE_CONFIG_DIR` as the fake opencode last saw it.
    pub(crate) fn config_dir(&self) -> PathBuf {
        PathBuf::from(fs::read_to_string(self.log.with_extension("config")).unwrap())
    }

    /// How many times the fake opencode was started.
    pub(crate) fn starts(&self) -> usize {
        fs::read_to_string(self.log.with_extension("starts")).map_or(0, |text| text.lines().count())
    }

    /// Attaches to the worktree `name`'s opencode, once it is ready, to send it events.
    pub(crate) fn driver(&self, name: &str) -> Driver {
        let attach = self.fx.attach(name);
        attach.wait_for("fake opencode ready");
        Driver { attach, sent: 0 }
    }
}

/// An attached terminal that makes the fake opencode write events to its pipe.
pub(crate) struct Driver {
    attach: Attach,
    sent: u32,
}

impl Driver {
    /// Makes the fake opencode write `line` to its events pipe, as the plugin writes an event, and
    /// waits until it has.
    pub(crate) fn event(&mut self, line: &Value) {
        self.sent += 1;
        self.attach.send(&format!("send {} {line}\n", self.sent));
        self.attach.wait_for(&format!("sent #{}.", self.sent));
    }

    /// Types `text` into the fake opencode.
    pub(crate) fn send(&mut self, text: &str) {
        self.attach.send(text);
    }

    /// Detaches, leaving the fake opencode running.
    pub(crate) fn detach(self) {
        self.attach.detach();
    }

    /// Waits for the attach to end, as it does when the fake opencode exits.
    pub(crate) fn wait_ended(self) {
        self.attach.wait_ended();
    }
}

fn split_nul(raw: &[u8]) -> Vec<String> {
    raw.split(|&b| b == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| String::from_utf8(arg.to_vec()).unwrap())
        .collect()
}
