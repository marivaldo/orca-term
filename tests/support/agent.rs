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

/// Between two reads of a slow attach's output. The fake claude's flood (4 MiB) then takes over a
/// second to read, far longer than the agent takes to print it.
const SLOW_READ_PAUSE: Duration = Duration::from_millis(1);

/// The fake sandbox tools, one set for each OS; all are present so the suite runs the same way
/// on macOS and Linux.
const SANDBOX_TOOLS: &[&str] = &["sandbox-exec", "bwrap", "socat"];

/// The fake `claude`: answers `--version`, records its arguments, directory and environment
/// variable names (never values), runs a test's prelude, echoes every line typed, prints 4 MiB on
/// `flood`, exits with the code typed after `exit`, and ends on `SIGHUP` as claude does (unless
/// the prelude says otherwise). The idle timeout ends a leaked one on its own.
///
/// The lines are read by a subshell that traps no signal, while the script itself only waits for
/// it, reports each size and ends with its exit code: macOS's `/bin/bash` (3.2) can drop the last
/// character of the line `read` is reading when a trapped signal arrives, and the holder resizes
/// the terminal (a `WINCH`) right before it types a client's input.
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
(
  trap - WINCH
  while IFS= read -r -t 120 line; do
    case "$line" in
      exit*)  echo "bye"; exit "${line#exit }" ;;
      flood*) x=x; for i in 1 2 3 4 5 6 7 8 9 10 11 12; do x="$x$x"; done
              for i in $(eval echo {1..1024}); do echo "$x"; done; echo "flooded" ;;
      *)      echo "you said: $line" ;;
    esac
  done
  exit 0
) <&0 &
reader=$!
while kill -0 "$reader" 2>/dev/null; do wait "$reader"; done
wait "$reader"
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
        Self::with_repo(Repo::new())
    }

    /// The fixture around `repo`: the isolated PATH and the fake claude's log files beside it.
    pub(crate) fn with_repo(repo: Repo) -> Self {
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

    /// Writes an executable `name` on the isolated PATH. Whatever was there goes first, so that
    /// replacing the link to the real `git` never writes through it.
    pub(crate) fn write_tool(&self, name: &str, script: &str) {
        let path = self.bin.join(name);
        if path.symlink_metadata().is_ok() {
            fs::remove_file(&path).unwrap();
        }
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// Removes `name` from the isolated PATH.
    pub(crate) fn remove_tool(&self, name: &str) {
        fs::remove_file(self.bin.join(name)).unwrap();
    }

    /// The binary as a `Command`, run from the primary checkout with the isolated PATH. The XDG
    /// base directories opencode's are found under fall back to the fixture's `HOME`, so starting
    /// opencode makes its directories there and never in the developer's own (#35).
    pub(crate) fn command(&self, args: &[&str]) -> Command {
        let mut cmd = self.repo.command(&self.repo.root);
        cmd.args(args)
            .env("PATH", &self.bin)
            .env("ORCA_TERM_FAKE_CLAUDE_LOG", &self.log)
            .env_remove("XDG_DATA_HOME")
            .env_remove("XDG_CACHE_HOME")
            .env_remove("XDG_STATE_HOME")
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

    /// Starts `agent attach` on `name` over pipes read with a pause between reads: a client that
    /// keeps reading but falls far behind an agent that prints a lot at once.
    pub(crate) fn slow_attach(&self, name: &str) -> Attach {
        Attach::reading_every(SLOW_READ_PAUSE, self.spawn_attach(name))
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
    fn new(child: Child) -> Self {
        Self::reading_every(Duration::ZERO, child)
    }

    /// An attach whose stdout is read with `pause` between reads.
    fn reading_every(pause: Duration, mut child: Child) -> Self {
        let stdin = child.stdin.take();
        let stdout = collect(child.stdout.take().unwrap(), pause);
        let stderr = collect(child.stderr.take().unwrap(), Duration::ZERO);
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

    /// Waits until `marker` has been printed, failing the test with the end of what was seen
    /// otherwise. Each look searches only the bytes not searched yet, in place, so a flood of
    /// output (megabytes) costs no more than reading it once (#144).
    pub(crate) fn wait_for(&self, marker: &str) {
        let start = Instant::now();
        let mut searched = 0;
        while !self.printed_since(marker.as_bytes(), &mut searched) {
            assert!(
                start.elapsed() < WAIT,
                "`{marker}` never appeared; the attach printed, ending with:\n{}",
                self.output_tail()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Whether `marker` was printed from byte `*from` on. Moves `*from` to where the next look
    /// starts: the end, less the marker's length but one, so a marker split between two reads is
    /// still found.
    fn printed_since(&self, marker: &[u8], from: &mut usize) -> bool {
        let out = self.stdout.lock().unwrap();
        let found = out[*from..]
            .windows(marker.len())
            .any(|window| window == marker);
        *from = (*from).max(out.len().saturating_sub(marker.len().saturating_sub(1)));
        found
    }

    /// The last few thousand characters the attach printed, for a failure message.
    fn output_tail(&self) -> String {
        const TAIL: usize = 4000;
        let out = self.stdout.lock().unwrap();
        let start = out.len().saturating_sub(TAIL);
        String::from_utf8_lossy(&out[start..]).into_owned()
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

/// Collects everything `reader` yields, in a thread, until it closes, waiting `pause` between
/// reads.
fn collect(
    mut reader: impl std::io::Read + Send + 'static,
    pause: Duration,
) -> Arc<Mutex<Vec<u8>>> {
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&buffer);
    thread::spawn(move || {
        let mut chunk = [0u8; 4096];
        while let Ok(n) = reader.read(&mut chunk) {
            if n == 0 {
                break;
            }
            sink.lock().unwrap().extend_from_slice(&chunk[..n]);
            thread::sleep(pause);
        }
    });
    buffer
}

/// A client of a holder that speaks the protocol itself, as the application does (#174), rather
/// than through `agent attach`: it sends its own sizes. What the holder sends is collected as it
/// comes: the output frames' bytes, one after the other.
#[derive(Debug)]
pub(crate) struct RawClient {
    stream: std::os::unix::net::UnixStream,
    output: Arc<Mutex<Vec<u8>>>,
    /// The first output frame, the replay, once it came.
    replay: Arc<Mutex<Option<Vec<u8>>>>,
}

impl RawClient {
    /// Connects to the holder whose admin directory is `admin`, through a link with a short path
    /// at `link` (a unix socket's path holds about 100 bytes, and admin directories are longer).
    pub(crate) fn connect(admin: &Path, link: &Path) -> Self {
        if link.symlink_metadata().is_err() {
            std::os::unix::fs::symlink(admin.join("agent.sock"), link).unwrap();
        }
        let stream = std::os::unix::net::UnixStream::connect(link).unwrap();
        let output = Arc::new(Mutex::new(Vec::new()));
        let replay = Arc::new(Mutex::new(None));
        let reader = stream.try_clone().unwrap();
        let (sink, first) = (Arc::clone(&output), Arc::clone(&replay));
        thread::spawn(move || read_frames(reader, &sink, &first));
        Self {
            stream,
            output,
            replay,
        }
    }

    /// Sends this client's terminal size.
    pub(crate) fn resize(&mut self, rows: u16, cols: u16) {
        let mut payload = rows.to_be_bytes().to_vec();
        payload.extend_from_slice(&cols.to_be_bytes());
        self.send_frame(b'r', &payload);
    }

    /// Types `text` into the agent.
    pub(crate) fn send(&mut self, text: &str) {
        self.send_frame(b'i', text.as_bytes());
    }

    fn send_frame(&mut self, tag: u8, payload: &[u8]) {
        let mut frame = vec![tag];
        frame.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_be_bytes());
        frame.extend_from_slice(payload);
        self.stream.write_all(&frame).unwrap();
    }

    /// How many bytes of output came so far: where [`Self::wait_for_since`] starts looking.
    pub(crate) fn mark(&self) -> usize {
        self.output.lock().unwrap().len()
    }

    /// Waits until `marker` was printed after the first `from` bytes of output, failing the test
    /// with everything printed after them otherwise.
    pub(crate) fn wait_for_since(&self, from: usize, marker: &str) {
        let start = Instant::now();
        while !self.printed_since(from, marker) {
            assert!(
                start.elapsed() < WAIT,
                "`{marker}` after byte {from} never happened; the agent printed after it:\n{}",
                self.output_since(from)
            );
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn printed_since(&self, from: usize, marker: &str) -> bool {
        let out = self.output.lock().unwrap();
        out[from.min(out.len())..]
            .windows(marker.len())
            .any(|window| window == marker.as_bytes())
    }

    fn output_since(&self, from: usize) -> String {
        let out = self.output.lock().unwrap();
        String::from_utf8_lossy(&out[from.min(out.len())..]).into_owned()
    }

    /// The replay, the holder's first output frame, waiting for it.
    pub(crate) fn replay(&self) -> Vec<u8> {
        wait_until("the replay", || self.replay.lock().unwrap().is_some());
        self.replay.lock().unwrap().clone().unwrap()
    }
}

/// Dropping the client closes its connection, which detaches it: the reader's copy of the socket
/// would keep it open otherwise.
impl Drop for RawClient {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

/// Reads the holder's frames from `reader` until it closes: every output frame's bytes go to
/// `sink`, and the first one is kept in `first` too.
fn read_frames(
    mut reader: impl std::io::Read,
    sink: &Mutex<Vec<u8>>,
    first: &Mutex<Option<Vec<u8>>>,
) {
    let mut header = [0u8; 5];
    while reader.read_exact(&mut header).is_ok() {
        let len = u32::from_be_bytes([header[1], header[2], header[3], header[4]]);
        let mut payload = vec![0u8; usize::try_from(len).unwrap()];
        if reader.read_exact(&mut payload).is_err() {
            return;
        }
        if header[0] == b'o' {
            first.lock().unwrap().get_or_insert_with(|| payload.clone());
            sink.lock().unwrap().extend_from_slice(&payload);
        }
    }
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
