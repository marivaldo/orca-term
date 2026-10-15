//! Live sidebar updates (seam 1): a running Neovim registers its server address with `nvim
//! register`, and after every write to a worktree's state the core sends it one msgpack-RPC
//! notification firing `User OrcaTermWorktreeChanged` with the worktree's path.
//!
//! A test Neovim is a `UnixListener` the test binds at a short path under `/tmp` (a unix socket
//! address holds about 100 bytes), registered like a real one. Each message is read until the core
//! hangs up, decoded from msgpack and compared whole. The agent fixture (fake claude on an isolated
//! PATH, holders stopped at the end) is the one `tests/agent.rs` uses.

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
    reason = "the agent fixture is shared between suites, and this one leaves part of it unused"
)]
mod fixture;
#[expect(
    dead_code,
    reason = "the fixture is shared between suites, and this one leaves part of it unused"
)]
mod support;

use std::fs;
use std::io::{ErrorKind, Read as _, Write as _};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use fixture::AgentRepo;
use serde_json::{Value, json};

/// How long a test waits for a message that comes from another process (the holder).
const WAIT: Duration = Duration::from_secs(15);

/// A test Neovim: a unix socket listening at a short path of its own, removed when dropped.
struct Neovim {
    dir: PathBuf,
    listener: UnixListener,
}

impl Neovim {
    /// A Neovim listening at `/tmp/ot-<pid>-<n>/nvim.sock`.
    fn new() -> Self {
        let dir = short_dir();
        let listener = UnixListener::bind(dir.join("nvim.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        Self { dir, listener }
    }

    /// Its server address, as Neovim's `v:servername` would be.
    fn address(&self) -> String {
        self.dir.join("nvim.sock").display().to_string()
    }

    /// Every message sent so far: each connection waiting to be accepted, read until the core
    /// hung up, decoded.
    fn messages(&self) -> Vec<Value> {
        let mut messages = Vec::new();
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => messages.push(decode_all(&read_to_end(stream))),
                Err(err) if err.kind() == ErrorKind::WouldBlock => return messages,
                Err(err) => panic!("accept: {err}"),
            }
        }
    }

    /// The worktree paths of every message sent so far, each message checked whole.
    fn paths(&self) -> Vec<String> {
        self.messages().iter().map(path_of).collect()
    }

    /// Waits until at least `count` messages came, then returns all their paths.
    fn wait_for_paths(&self, count: usize) -> Vec<String> {
        let start = Instant::now();
        let mut paths = Vec::new();
        while paths.len() < count {
            assert!(
                start.elapsed() < WAIT,
                "only {} of {count} messages came: {paths:?}",
                paths.len()
            );
            paths.extend(self.paths());
            thread::sleep(Duration::from_millis(20));
        }
        paths
    }
}

impl Drop for Neovim {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// A fresh, short directory under `/tmp`, so a socket in it fits a unix socket address.
fn short_dir() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    let dir = PathBuf::from(format!("/tmp/ot-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Everything `stream` yields until the peer hangs up. The timeout is a safety net only: macOS
/// refuses to set it (EINVAL) once the peer has already hung up, and then the read ends at once.
fn read_to_end(mut stream: UnixStream) -> Vec<u8> {
    stream.set_nonblocking(false).unwrap();
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut bytes = Vec::new();
    stream.read_to_end(&mut bytes).unwrap();
    bytes
}

/// The one msgpack value `bytes` hold, failing on trailing bytes.
fn decode_all(bytes: &[u8]) -> Value {
    let mut at = 0;
    let value = decode(bytes, &mut at);
    assert_eq!(at, bytes.len(), "trailing bytes after {value}: {bytes:?}");
    value
}

/// The msgpack value at `bytes[*at..]`, as JSON: only the types the core sends (positive fixint,
/// fixmap, fixarray and array 16, fixstr, str 8, str 16 and str 32).
fn decode(bytes: &[u8], at: &mut usize) -> Value {
    let tag = take(bytes, at, 1)[0];
    match tag {
        0x00..=0x7f => json!(tag),
        0x80..=0x8f => {
            let mut map = serde_json::Map::new();
            for _ in 0..(tag & 0x0f) {
                let Value::String(key) = decode(bytes, at) else {
                    panic!("a map key that is not a string in {bytes:?}");
                };
                map.insert(key, decode(bytes, at));
            }
            Value::Object(map)
        }
        0x90..=0x9f => decode_array(bytes, at, usize::from(tag & 0x0f)),
        0xdc => {
            let len = u16::from_be_bytes(take(bytes, at, 2).try_into().unwrap());
            decode_array(bytes, at, usize::from(len))
        }
        0xa0..=0xbf => decode_str(bytes, at, usize::from(tag & 0x1f)),
        0xd9 => {
            let len = usize::from(take(bytes, at, 1)[0]);
            decode_str(bytes, at, len)
        }
        0xda => {
            let len = u16::from_be_bytes(take(bytes, at, 2).try_into().unwrap());
            decode_str(bytes, at, usize::from(len))
        }
        0xdb => {
            let len = u32::from_be_bytes(take(bytes, at, 4).try_into().unwrap());
            decode_str(bytes, at, usize::try_from(len).unwrap())
        }
        other => panic!("unexpected msgpack type byte {other:#x} in {bytes:?}"),
    }
}

fn decode_array(bytes: &[u8], at: &mut usize, len: usize) -> Value {
    Value::Array((0..len).map(|_| decode(bytes, at)).collect())
}

fn decode_str(bytes: &[u8], at: &mut usize, len: usize) -> Value {
    json!(String::from_utf8(take(bytes, at, len).to_vec()).unwrap())
}

/// The next `len` bytes, moving `at` past them.
fn take<'b>(bytes: &'b [u8], at: &mut usize, len: usize) -> &'b [u8] {
    assert!(*at + len <= bytes.len(), "truncated message: {bytes:?}");
    let taken = &bytes[*at..*at + len];
    *at += len;
    taken
}

/// The worktree path a notification carries, after checking the whole message is the call that
/// fires `User OrcaTermWorktreeChanged`.
fn path_of(message: &Value) -> String {
    let path = message[2][1]["data"]["path"].as_str().unwrap_or_default();
    assert_eq!(
        message,
        &json!([
            2,
            "nvim_exec_autocmds",
            ["User", {"pattern": "OrcaTermWorktreeChanged", "data": {"path": path}}]
        ])
    );
    path.to_owned()
}

/// `count` times the path `path`: the paths `count` notifications about one worktree carry.
fn times(path: &str, count: usize) -> Vec<String> {
    vec![path.to_owned(); count]
}

/// The repository's registry of Neovims, in its git common dir.
fn registry(fx: &AgentRepo) -> PathBuf {
    fx.repo.root.join(".git/orca-term/nvim")
}

/// The addresses in the registry, one per file, sorted.
fn registered(fx: &AgentRepo) -> Vec<String> {
    let Ok(entries) = fs::read_dir(registry(fx)) else {
        return Vec::new();
    };
    let mut addresses: Vec<String> = entries
        .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect();
    addresses.sort();
    addresses
}

/// `worktree ls --json`'s path of the worktree `name`: what a notification must carry.
fn listed_path(fx: &AgentRepo, name: &str) -> String {
    fx.listed(name)["path"].as_str().unwrap().to_owned()
}

/// Commits a setup script for every new worktree.
fn commit_setup(fx: &AgentRepo, script: &str) {
    fx.repo
        .write("orca-term.yaml", &format!("scripts:\n  setup: {script}\n"));
    fx.repo.commit_all("setup");
}

/// Runs the handler the agent's settings register for `SessionStart`, as claude would: the
/// command with its arguments, a payload on stdin, in the worktree.
fn fire_session_start(worktree: &Path, admin: &Path) {
    let settings: Value =
        serde_json::from_slice(&fs::read(admin.join("claude-settings.json")).unwrap()).unwrap();
    let handler = &settings["hooks"]["SessionStart"][0]["hooks"][0];
    let args: Vec<&str> = handler["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect();
    let payload = json!({
        "session_id": "40b1fb34-8b28-4e24-9bcf-31292c24b65f",
        "cwd": worktree,
        "hook_event_name": "SessionStart",
        "source": "startup",
    });
    let mut child = Command::new(handler["command"].as_str().unwrap())
        .args(&args)
        .current_dir(worktree)
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.to_string().as_bytes());
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success() && out.stdout.is_empty() && out.stderr.is_empty());
}

#[test]
fn register_records_the_address_and_unregister_removes_it_both_idempotent() {
    let fx = AgentRepo::new();
    let nvim = Neovim::new();
    let address = nvim.address();

    let text = fx.ok(&["nvim", "register", &address]);
    assert_eq!(text, format!("registered Neovim {address}\n"));
    assert_eq!(registered(&fx), vec![address.clone()]);
    let doc: Value =
        serde_json::from_str(&fx.ok(&["nvim", "register", &address, "--json"])).unwrap();
    assert_eq!(doc.as_object().unwrap().len(), 2, "{doc}");
    assert_eq!(doc["contract"], 1);
    assert!(doc["version"].is_string());
    assert_eq!(
        registered(&fx),
        vec![address.clone()],
        "registering twice changes nothing"
    );

    // From a worktree, the same registry: the repository's, in its git common dir.
    let worktree = fx.new_worktree("fix");
    let other = Neovim::new();
    let out = fx
        .command(&["nvim", "register", &other.address()])
        .current_dir(&worktree)
        .output()
        .unwrap();
    assert!(out.status.success());
    let mut both = vec![address.clone(), other.address()];
    both.sort();
    assert_eq!(registered(&fx), both);

    let text = fx.ok(&["nvim", "unregister", &address]);
    assert_eq!(text, format!("unregistered Neovim {address}\n"));
    assert_eq!(registered(&fx), [other.address()]);
    let doc: Value =
        serde_json::from_str(&fx.ok(&["nvim", "unregister", &address, "--json"])).unwrap();
    assert_eq!(
        doc.as_object().unwrap().len(),
        2,
        "unregistering twice succeeds: {doc}"
    );
    assert_eq!(registered(&fx), [other.address()]);
}

#[test]
fn an_address_that_is_not_an_absolute_path_is_refused() {
    let fx = AgentRepo::new();
    for address in ["nvim.sock", "127.0.0.1:6666", "./nvim.sock"] {
        let out = fx.run(&["nvim", "register", address, "--json"]);
        assert_eq!(out.status.code(), Some(2), "{address}");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            stderr.contains("absolute path of a unix socket"),
            "{stderr}"
        );
    }
    assert!(registered(&fx).is_empty());
}

#[test]
fn a_hook_event_notifies_every_registered_neovim() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    fx.ok(&["agent", "start", "fix"]);
    let (first, second) = (Neovim::new(), Neovim::new());
    fx.ok(&["nvim", "register", &first.address()]);
    fx.ok(&["nvim", "register", &second.address()]);

    fire_session_start(&worktree, &admin);

    let path = listed_path(&fx, "fix");
    assert_eq!(first.paths(), times(&path, 1));
    assert_eq!(second.paths(), times(&path, 1));
}

#[test]
fn worktree_new_setup_and_rm_notify_with_the_worktrees_path() {
    let fx = AgentRepo::new();
    commit_setup(&fx, "echo set up");
    let (first, second) = (Neovim::new(), Neovim::new());
    fx.ok(&["nvim", "register", &first.address()]);
    fx.ok(&["nvim", "register", &second.address()]);

    fx.ok(&["worktree", "new", "fix"]);
    let path = listed_path(&fx, "fix");
    // Once the worktree exists, once its setup records itself running, once it ends.
    assert_eq!(first.paths(), times(&path, 3));
    assert_eq!(second.paths(), times(&path, 3));

    fx.ok(&["worktree", "setup", "fix"]);
    assert_eq!(first.paths(), times(&path, 2), "running, then ended");
    assert_eq!(second.paths(), times(&path, 2));

    fx.ok(&["worktree", "rm", "fix"]);
    assert_eq!(first.paths(), times(&path, 1));
    assert_eq!(second.paths(), times(&path, 1));
}

#[test]
fn a_setup_with_no_script_notifies_once() {
    let fx = AgentRepo::new();
    let nvim = Neovim::new();
    fx.ok(&["nvim", "register", &nvim.address()]);
    fx.ok(&["worktree", "new", "fix"]);
    let path = listed_path(&fx, "fix");
    assert_eq!(
        nvim.paths(),
        times(&path, 2),
        "created, then no setup recorded"
    );
    fx.ok(&["worktree", "setup", "fix"]);
    assert_eq!(nvim.paths(), times(&path, 1));
}

#[test]
fn agent_start_stop_and_the_agents_own_exit_notify() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");
    let path = listed_path(&fx, "fix");
    let (first, second) = (Neovim::new(), Neovim::new());
    fx.ok(&["nvim", "register", &first.address()]);
    fx.ok(&["nvim", "register", &second.address()]);

    fx.ok(&["agent", "start", "fix"]);
    assert_eq!(first.paths(), times(&path, 1));
    assert_eq!(second.paths(), times(&path, 1));

    // `agent stop` once the holder is gone, and the holder itself once it removed its files.
    fx.ok(&["agent", "stop", "fix"]);
    assert_eq!(first.wait_for_paths(2), times(&path, 2));
    assert_eq!(second.wait_for_paths(2), times(&path, 2));

    fx.ok(&["agent", "start", "fix"]);
    assert_eq!(first.paths(), times(&path, 1));
    assert_eq!(second.paths(), times(&path, 1));
    let mut attach = fx.attach("fix");
    attach.wait_for("fake claude ready");
    attach.send("exit 0\n");
    attach.wait_ended();
    assert_eq!(
        first.wait_for_paths(1),
        times(&path, 1),
        "the holder, as the agent exits"
    );
    assert_eq!(second.wait_for_paths(1), times(&path, 1));
    assert_eq!(fx.listed("fix")["holder"], false);
}

#[test]
fn a_stale_address_is_pruned_and_the_command_still_succeeds() {
    let fx = AgentRepo::new();
    let live = Neovim::new();
    // A socket file nobody listens on any more, as a Neovim that crashed leaves it.
    let crashed_dir = short_dir();
    let crashed = crashed_dir.join("nvim.sock");
    drop(UnixListener::bind(&crashed).unwrap());
    assert!(
        crashed.exists(),
        "dropping a listener leaves its socket file"
    );
    let crashed_address = crashed.display().to_string();
    // Nothing at all at the path.
    let gone = format!("/tmp/ot-{}-gone/nvim.sock", std::process::id());
    for address in [&live.address(), &crashed_address, &gone] {
        fx.ok(&["nvim", "register", address]);
    }
    assert_eq!(registered(&fx).len(), 3);

    let out = fx.run(&["worktree", "new", "fix", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["worktree"]["name"], "fix");
    assert!(out.stderr.is_empty());

    assert_eq!(registered(&fx), [live.address()]);
    assert_eq!(live.paths().len(), 2);
    let _ = fs::remove_dir_all(crashed_dir);
}

#[test]
fn an_address_too_long_for_a_socket_is_skipped_and_kept() {
    let fx = AgentRepo::new();
    let long = format!("/tmp/{}/nvim.sock", "x".repeat(150));
    fx.ok(&["nvim", "register", &long]);
    fx.ok(&["worktree", "new", "fix"]);
    assert_eq!(registered(&fx), [long]);
}

#[test]
fn a_neovim_that_never_reads_or_never_accepts_does_not_hold_a_command_up() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");
    // One accepts every connection and never reads it; the other never accepts.
    let silent = Neovim::new();
    let held = Arc::new(Mutex::new(Vec::new()));
    let listener = silent.listener.try_clone().unwrap();
    let keep = Arc::clone(&held);
    listener.set_nonblocking(false).unwrap();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            keep.lock().unwrap().push(stream);
        }
    });
    let deaf = Neovim::new();
    fx.ok(&["nvim", "register", &silent.address()]);
    fx.ok(&["nvim", "register", &deaf.address()]);

    for _ in 0..3 {
        let start = Instant::now();
        fx.ok(&["worktree", "setup", "fix"]);
        let took = start.elapsed();
        assert!(took < Duration::from_secs(1), "took {took:?}");
    }
    let mut both = vec![silent.address(), deaf.address()];
    both.sort();
    assert_eq!(registered(&fx), both, "neither is pruned: both are alive");
    assert_eq!(deaf.paths().len(), 3);
}

#[test]
fn listing_sends_nothing() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");
    let nvim = Neovim::new();
    fx.ok(&["nvim", "register", &nvim.address()]);
    fx.ok(&["worktree", "ls"]);
    fx.ok(&["worktree", "ls", "--json"]);
    assert!(nvim.messages().is_empty());
}
