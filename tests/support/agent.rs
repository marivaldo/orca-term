//! A fake `claude` and fake sandbox tools on an isolated PATH, for black-box tests of the agent
//! commands (seam 1), plus a driver for `agent attach` over pipes and a guard that stops every
//! holder a test leaves behind.
//!
//! The binary runs with `PATH` set to one directory holding: the fake `claude` (a bash script using
//! only builtins and absolute paths), symlinks named `git` and `bash` to the real ones, and, unless
//! a test removes them, `sandbox-exec`, `bwrap` and `socat` that do nothing. So the tests need no
//! claude, no sandbox and no network, on macOS and Linux alike.

use std::fs::{self, File};
use std::io::Write as _;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal};
use serde_json::Value;

use crate::support::Repo;

/// How long a test waits for something to happen before failing.
const WAIT: Duration = Duration::from_secs(15);

/// The fake sandbox tools, one set for each OS; all are present so the suite runs the same way
/// on macOS and Linux.
const SANDBOX_TOOLS: &[&str] = &["sandbox-exec", "bwrap", "socat"];

/// The fake `claude`: answers `--version`, records its arguments, directory and environment
/// variable names (never values), runs a test's prelude, echoes every line typed, prints 4 MiB on
/// `flood`, exits with the code typed after `exit`, and ends on `SIGHUP` as claude does (unless
/// the prelude says otherwise). The idle timeout ends a leaked one on its own.
const FAKE_CLAUDE: &str = r#"#!/bin/bash
if [ "$1" = "--version" ]; then echo "VERSION"; exit 0; fi
log="$ORCA_TERM_FAKE_CLAUDE_LOG"
printf '%s\0' "$@" > "$log.argv"
pwd > "$log.cwd"
compgen -e > "$log.env"
echo start >> "$log.starts"
trap 'echo "size=$(/bin/stty size)"' WINCH
trap 'echo "got HUP"; exit 129' HUP
PRELUDE
echo "fake claude ready"
while IFS= read -r -t 120 line; do
  case "$line" in
    exit*)  echo "bye"; exit "${line#exit }" ;;
    flood*) x=x; for i in 1 2 3 4 5 6 7 8 9 10 11 12; do x="$x$x"; done
            for i in $(eval echo {1..1024}); do echo "$x"; done; echo "flooded" ;;
    *)      echo "you said: $line" ;;
  esac
done
exit 0
"#;

/// A repository, the isolated PATH and the fake claude's log files.
#[derive(Debug)]
pub(crate) struct AgentRepo {
    pub(crate) repo: Repo,
    /// The one directory on the binary's PATH.
    pub(crate) bin: PathBuf,
    /// The prefix of the fake claude's log files (`<log>.argv`, `<log>.cwd`, ...).
    pub(crate) log: PathBuf,
}

impl AgentRepo {
    pub(crate) fn new() -> Self {
        let repo = Repo::new();
        let bin = repo.tmp.path().join("bin");
        fs::create_dir_all(&bin).unwrap();
        for tool in ["git", "bash"] {
            let real = which(tool);
            std::os::unix::fs::symlink(real, bin.join(tool)).unwrap();
        }
        let fixture = Self {
            log: repo.tmp.path().join("fake-claude"),
            repo,
            bin,
        };
        for tool in SANDBOX_TOOLS {
            fixture.write_tool(tool, "#!/bin/sh\nexit 0\n");
        }
        fixture.write_claude("2.1.296 (Claude Code)", "");
        fixture
    }

    /// Writes the fake `claude`, answering `version` to `--version` and running `prelude` (bash)
    /// before it says it is ready.
    pub(crate) fn write_claude(&self, version: &str, prelude: &str) {
        let script = FAKE_CLAUDE
            .replace("VERSION", version)
            .replace("PRELUDE", prelude);
        self.write_tool("claude", &script);
    }

    /// Writes an executable `name` on the isolated PATH.
    pub(crate) fn write_tool(&self, name: &str, script: &str) {
        let path = self.bin.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Removes `name` from the isolated PATH.
    pub(crate) fn remove_tool(&self, name: &str) {
        fs::remove_file(self.bin.join(name)).unwrap();
    }

    /// The binary as a `Command`, run from the primary checkout with the isolated PATH.
    pub(crate) fn command(&self, args: &[&str]) -> Command {
        let mut cmd = self.repo.command(&self.repo.root);
        cmd.args(args)
            .env("PATH", &self.bin)
            .env("ORCA_TERM_FAKE_CLAUDE_LOG", &self.log)
            .stdin(Stdio::null());
        cmd
    }

    /// Runs the binary with `args` and returns what it did.
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

    /// Creates the worktree `name` and returns its path.
    pub(crate) fn new_worktree(&self, name: &str) -> PathBuf {
        self.ok(&["worktree", "new", name]);
        self.repo
            .home
            .join("orca-term/worktrees/repo")
            .join(name)
            .canonicalize()
            .unwrap()
    }

    /// The worktree's state directory in git's admin dir, as git reports it.
    pub(crate) fn admin_dir(&self, worktree: &Path) -> PathBuf {
        let dir = self.repo.git(&[
            "-C",
            worktree.to_str().unwrap(),
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "orca-term",
        ]);
        PathBuf::from(dir.trim())
    }

    /// The arguments the fake claude was started with, after its own name.
    pub(crate) fn argv(&self) -> Vec<String> {
        let raw = fs::read(self.log.with_extension("argv")).unwrap();
        raw.split(|&b| b == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| String::from_utf8(arg.to_vec()).unwrap())
            .collect()
    }

    /// How many times the fake claude was started.
    pub(crate) fn starts(&self) -> usize {
        fs::read_to_string(self.log.with_extension("starts")).map_or(0, |text| text.lines().count())
    }

    /// The settings file the fake claude was given, parsed.
    pub(crate) fn settings(&self) -> Value {
        let argv = self.argv();
        let at = argv.iter().position(|arg| arg == "--settings").unwrap();
        serde_json::from_slice(&fs::read(&argv[at + 1]).unwrap()).unwrap()
    }

    /// The one entry of `worktree ls --json` named `name`.
    pub(crate) fn listed(&self, name: &str) -> Value {
        let doc: Value = serde_json::from_str(&self.ok(&["worktree", "ls", "--json"])).unwrap();
        doc["worktrees"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == name)
            .cloned()
            .unwrap_or_else(|| panic!("no worktree {name} in {doc}"))
    }

    /// Starts `agent attach` on `name` over pipes.
    pub(crate) fn attach(&self, name: &str) -> Attach {
        Attach::new(self.spawn_attach(name))
    }

    /// Starts `agent attach` on `name` over pipes that nobody reads: once its stdout pipe is
    /// full it stops reading from the holder, like an attach suspended in its shell. Killed when
    /// dropped.
    pub(crate) fn stuck_attach(&self, name: &str) -> StuckAttach {
        StuckAttach(self.spawn_attach(name))
    }

    /// Starts `agent attach` on `name` over pipes, run through `wrapper` (a command line that runs
    /// the rest of its arguments, such as `unshare --net`), with the same environment and
    /// directory as [`Self::command`].
    #[cfg(target_os = "linux")]
    pub(crate) fn attach_inside(&self, wrapper: &[String], name: &str) -> Attach {
        let inner = self.command(&["agent", "attach", name]);
        let mut cmd = Command::new(&wrapper[0]);
        cmd.args(&wrapper[1..])
            .arg(inner.get_program())
            .args(inner.get_args());
        for (key, value) in inner.get_envs() {
            match value {
                Some(value) => cmd.env(key, value),
                None => cmd.env_remove(key),
            };
        }
        if let Some(dir) = inner.get_current_dir() {
            cmd.current_dir(dir);
        }
        let child = cmd
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        Attach::new(child)
    }

    fn spawn_attach(&self, name: &str) -> Child {
        self.command(&["agent", "attach", name])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    /// Attaches until the fake claude has printed `marker`, then detaches: how a test waits for the
    /// agent to be ready, or for output it caused, before acting on it. Returns everything the
    /// attach saw.
    pub(crate) fn wait_for_output(&self, name: &str, marker: &str) -> String {
        let attach = self.attach(name);
        attach.wait_for(marker);
        attach.detach().stdout
    }
}

/// Every holder is stopped when the fixture goes, so a failing test leaves none behind; a holder
/// that will not stop is killed.
impl Drop for AgentRepo {
    fn drop(&mut self) {
        let Ok(out) = self.command(&["worktree", "ls", "--json"]).output() else {
            return;
        };
        let Ok(doc) = serde_json::from_slice::<Value>(&out.stdout) else {
            return;
        };
        for entry in doc["worktrees"].as_array().into_iter().flatten() {
            if entry["holder"] != true {
                continue;
            }
            let path = entry["path"].as_str().unwrap_or_default();
            let _ = self.command(&["agent", "stop", path]).output();
            let admin = self.admin_dir(Path::new(path));
            if holder_alive(&admin) {
                let pid = holder_pid(&admin);
                let _ = Pid::from_raw(i32::try_from(pid).unwrap_or(0))
                    .map(|pid| rustix::process::kill_process(pid, Signal::KILL));
            }
        }
    }
}

/// An `agent attach` whose output nobody reads.
#[derive(Debug)]
pub(crate) struct StuckAttach(Child);

impl Drop for StuckAttach {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// What a finished `agent attach` printed and how it ended.
#[derive(Debug)]
pub(crate) struct AttachEnded {
    pub(crate) status: ExitStatus,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
}

/// A running `agent attach` over pipes: its stdout and stderr are collected as they come.
#[derive(Debug)]
pub(crate) struct Attach {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Arc<Mutex<Vec<u8>>>,
    stderr: Arc<Mutex<Vec<u8>>>,
}

impl Attach {
    fn new(mut child: Child) -> Self {
        let stdin = child.stdin.take();
        let stdout = collect(child.stdout.take().unwrap());
        let stderr = collect(child.stderr.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            stderr,
        }
    }

    pub(crate) fn pid(&self) -> u32 {
        self.child.id()
    }

    /// What the attach has printed so far.
    pub(crate) fn output(&self) -> String {
        String::from_utf8_lossy(&self.stdout.lock().unwrap()).into_owned()
    }

    /// Waits until `marker` has been printed, failing the test with what was seen otherwise.
    pub(crate) fn wait_for(&self, marker: &str) {
        let start = Instant::now();
        while !self.output().contains(marker) {
            assert!(
                start.elapsed() < WAIT,
                "`{marker}` never appeared; the attach printed:\n{}",
                self.output()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Types `text` into the agent.
    pub(crate) fn send(&mut self, text: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(text.as_bytes()).unwrap();
        stdin.flush().unwrap();
    }

    /// Types `text` into the attach if it still reads its stdin: for an attach the holder may
    /// already have closed.
    #[cfg(target_os = "linux")]
    pub(crate) fn offer(&mut self, text: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        let _ = stdin.write_all(text.as_bytes());
        let _ = stdin.flush();
    }

    /// Closes the attach's stdin, which detaches, and waits for it to end.
    pub(crate) fn detach(mut self) -> AttachEnded {
        drop(self.stdin.take());
        self.wait_ended()
    }

    /// Waits for the attach to end on its own, as it does when the agent ends.
    pub(crate) fn wait_ended(mut self) -> AttachEnded {
        let start = Instant::now();
        let status = loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                break status;
            }
            if start.elapsed() > WAIT {
                let _ = self.child.kill();
                panic!("the attach did not end; it printed:\n{}", self.output());
            }
            thread::sleep(Duration::from_millis(20));
        };
        // Let the collectors see the pipes close.
        thread::sleep(Duration::from_millis(50));
        AttachEnded {
            status,
            stdout: self.output(),
            stderr: String::from_utf8_lossy(&self.stderr.lock().unwrap()).into_owned(),
        }
    }

    /// Sends `signal` to the attach process, as a terminal closing (SIGHUP) or a kill would.
    pub(crate) fn kill_with(&self, signal: Signal) {
        let pid = Pid::from_raw(i32::try_from(self.pid()).unwrap()).unwrap();
        rustix::process::kill_process(pid, signal).unwrap();
    }
}

impl Drop for Attach {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Collects everything `reader` yields, in a thread, until it closes.
fn collect(mut reader: impl std::io::Read + Send + 'static) -> Arc<Mutex<Vec<u8>>> {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&buffer);
    thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(n) = reader.read(&mut chunk) {
            if n == 0 {
                break;
            }
            sink.lock().unwrap().extend_from_slice(&chunk[..n]);
        }
    });
    buffer
}

/// Whether a holder is alive for the worktree whose admin directory is `admin`: the read-only
/// probe of `agent.lock`, as the core does it.
pub(crate) fn holder_alive(admin: &Path) -> bool {
    lock_held(&admin.join("agent.lock"))
}

/// The holder's pid from `agent.json`.
pub(crate) fn holder_pid(admin: &Path) -> u32 {
    let record: Value =
        serde_json::from_slice(&fs::read(admin.join("agent.json")).unwrap()).unwrap();
    u32::try_from(record["pid"].as_u64().unwrap()).unwrap()
}

/// `worktree.json` of the worktree whose admin directory is `admin`.
pub(crate) fn state_file(admin: &Path) -> Value {
    serde_json::from_slice(&fs::read(admin.join("worktree.json")).unwrap()).unwrap()
}

/// Whether the file at `path` is locked right now (`flock`), probing and releasing at once.
pub(crate) fn lock_held(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    match file.try_lock() {
        Ok(()) => false,
        Err(std::fs::TryLockError::WouldBlock) => true,
        Err(std::fs::TryLockError::Error(err)) => panic!("probing {}: {err}", path.display()),
    }
}

/// Waits until `condition` holds, failing the test with `what` otherwise.
pub(crate) fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(start.elapsed() < WAIT, "{what} never happened");
        thread::sleep(Duration::from_millis(20));
    }
}

/// The real `tool`, as the test's own PATH finds it.
fn which(tool: &str) -> PathBuf {
    let out = Command::new("sh")
        .args(["-c", &format!("command -v {tool}")])
        .output()
        .unwrap();
    let path = String::from_utf8(out.stdout).unwrap();
    assert!(!path.trim().is_empty(), "{tool} is not on the test's PATH");
    PathBuf::from(path.trim())
}
