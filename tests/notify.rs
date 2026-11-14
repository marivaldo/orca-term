//! The registry of running Neovims and what the core sends them: a Neovim registers its server
//! address with `nvim register` (the application registers each one it starts), and the core
//! sends it one msgpack-RPC request at a time, waiting for the answer before it hangs up (Neovim
//! drops a call still waiting to run when its connection closes). Only two things are sent: the
//! `:checktime` at the end of an agent's turn (#177), and a changed file's diff (#194). Writing a
//! worktree's state sends nothing: the worktree-changed notice went with the Neovim client.
//!
//! A test Neovim is a `UnixListener` the test binds at a short path under `/tmp` (a unix socket
//! address holds about 100 bytes), registered like a real one. A live one answers each request as
//! Neovim does, on a thread of its own, and checks the core was still there to read the answer; a
//! deaf one never accepts, and its messages are read from its backlog once the core hung up. Each
//! message is decoded from msgpack and compared whole. The agent fixture (fake claude on an isolated
//! PATH, holders stopped at the end) is the one `tests/agent.rs` uses. A real Neovim opening a
//! diff is `tests/nvim_diff.rs`.

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

/// One request a live test Neovim answered.
struct Answered {
    /// The request's bytes, as read.
    bytes: Vec<u8>,
    /// Whether the answer could be written: the core had not hung up yet.
    answer_written: bool,
}

/// A test Neovim: a unix socket listening at a short path of its own, removed when dropped.
struct Neovim {
    dir: PathBuf,
    listener: UnixListener,
    /// The requests a live one answered and `messages()` has not taken yet; `None` when deaf.
    answered: Option<Arc<Mutex<Vec<Answered>>>>,
}

impl Neovim {
    /// A live Neovim listening at `/tmp/ot-<pid>-<n>/nvim.sock`, answering every request at once.
    fn new() -> Self {
        Self::answering_after(Duration::ZERO)
    }

    /// A live Neovim that answers every request `delay` after reading it, as a busy one does.
    fn answering_after(delay: Duration) -> Self {
        let mut neovim = Self::deaf();
        let answered = Arc::new(Mutex::new(Vec::new()));
        let keep = Arc::clone(&answered);
        let listener = neovim.listener.try_clone().unwrap();
        listener.set_nonblocking(false).unwrap();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                answer(stream, delay, &keep);
            }
        });
        neovim.answered = Some(answered);
        neovim
    }

    /// A Neovim that listens but never accepts, as a suspended one does: what the core sends
    /// waits in its backlog, unanswered.
    fn deaf() -> Self {
        let dir = short_dir();
        let listener = UnixListener::bind(dir.join("nvim.sock")).unwrap();
        listener.set_nonblocking(true).unwrap();
        Self {
            dir,
            listener,
            answered: None,
        }
    }

    /// Its server address, as Neovim's `v:servername` would be.
    fn address(&self) -> String {
        self.dir.join("nvim.sock").display().to_string()
    }

    /// Every message sent so far, decoded: for a live Neovim, each request it answered, checking
    /// the core read the answer; for a deaf one, each connection waiting to be accepted, read until
    /// the core hung up.
    fn messages(&self) -> Vec<Value> {
        if let Some(answered) = &self.answered {
            let taken: Vec<Answered> = answered.lock().unwrap().drain(..).collect();
            return taken
                .iter()
                .map(|request| {
                    assert!(
                        request.answer_written,
                        "the core hung up before Neovim answered"
                    );
                    decode_all(&request.bytes)
                })
                .collect();
        }
        let mut messages = Vec::new();
        loop {
            match self.listener.accept() {
                Ok((stream, _)) => messages.push(decode_all(&read_to_end(stream))),
                Err(err) if err.kind() == ErrorKind::WouldBlock => return messages,
                Err(err) => panic!("accept: {err}"),
            }
        }
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

/// Reads one whole request from `stream`, waits `delay`, then answers it as Neovim does,
/// `[1, id, nil, nil]`, and records it in `answered`. The lock is held while the answer is written,
/// so the request is recorded whole by the time the core, having read the answer, returns. Nothing
/// is recorded when the core hung up before the request was whole.
fn answer(mut stream: UnixStream, delay: Duration, answered: &Mutex<Vec<Answered>>) {
    stream.set_nonblocking(false).unwrap();
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let mut bytes = Vec::new();
    let request = loop {
        let mut chunk = [0_u8; 4096];
        let Some(read) = stream.read(&mut chunk).ok().filter(|&read| read > 0) else {
            return;
        };
        bytes.extend_from_slice(&chunk[..read]);
        if let Some(request) = decode(&bytes, &mut 0) {
            break request;
        }
    };
    thread::sleep(delay);
    let id = u8::try_from(request[1].as_u64().unwrap_or_default()).unwrap_or_default();
    let mut answered = answered.lock().unwrap();
    // A peer that already hung up makes the write fail (EPIPE).
    let answer_written = stream.write_all(&[0x94, 0x01, id, 0xc0, 0xc0]).is_ok();
    answered.push(Answered {
        bytes,
        answer_written,
    });
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

/// The one msgpack value `bytes` hold, failing on a truncated value or trailing bytes.
fn decode_all(bytes: &[u8]) -> Value {
    let mut at = 0;
    let value = decode(bytes, &mut at).unwrap_or_else(|| panic!("truncated message: {bytes:?}"));
    assert_eq!(at, bytes.len(), "trailing bytes after {value}: {bytes:?}");
    value
}

/// The msgpack value at `bytes[*at..]`, as JSON, or `None` when `bytes` end before it does: only
/// the types the core sends (positive fixint, fixmap, fixarray and array 16, fixstr, str 8, str 16
/// and str 32).
fn decode(bytes: &[u8], at: &mut usize) -> Option<Value> {
    let tag = take(bytes, at, 1)?[0];
    let value = match tag {
        0x00..=0x7f => json!(tag),
        0x80..=0x8f => {
            let mut map = serde_json::Map::new();
            for _ in 0..(tag & 0x0f) {
                let Value::String(key) = decode(bytes, at)? else {
                    panic!("a map key that is not a string in {bytes:?}");
                };
                map.insert(key, decode(bytes, at)?);
            }
            Value::Object(map)
        }
        0x90..=0x9f => decode_array(bytes, at, usize::from(tag & 0x0f))?,
        0xdc => {
            let len = u16::from_be_bytes(take(bytes, at, 2)?.try_into().unwrap());
            decode_array(bytes, at, usize::from(len))?
        }
        0xa0..=0xbf => decode_str(bytes, at, usize::from(tag & 0x1f))?,
        0xd9 => {
            let len = usize::from(take(bytes, at, 1)?[0]);
            decode_str(bytes, at, len)?
        }
        0xda => {
            let len = u16::from_be_bytes(take(bytes, at, 2)?.try_into().unwrap());
            decode_str(bytes, at, usize::from(len))?
        }
        0xdb => {
            let len = u32::from_be_bytes(take(bytes, at, 4)?.try_into().unwrap());
            decode_str(bytes, at, usize::try_from(len).unwrap())?
        }
        other => panic!("unexpected msgpack type byte {other:#x} in {bytes:?}"),
    };
    Some(value)
}

fn decode_array(bytes: &[u8], at: &mut usize, len: usize) -> Option<Value> {
    let items: Option<Vec<Value>> = (0..len).map(|_| decode(bytes, at)).collect();
    Some(Value::Array(items?))
}

fn decode_str(bytes: &[u8], at: &mut usize, len: usize) -> Option<Value> {
    Some(json!(
        String::from_utf8(take(bytes, at, len)?.to_vec()).unwrap()
    ))
}

/// The next `len` bytes, moving `at` past them, or `None` when fewer are left.
fn take<'b>(bytes: &'b [u8], at: &mut usize, len: usize) -> Option<&'b [u8]> {
    let taken = bytes.get(*at..*at + len)?;
    *at += len;
    Some(taken)
}

/// The request that asks a Neovim to `:checktime`.
fn checktime() -> Value {
    json!([0, 1, "nvim_command", ["checktime"]])
}

/// Whether `message` is a diff request (`nvim_exec_lua` of the core's own code) for `file`.
fn is_diff_of(message: &Value, file: &str) -> bool {
    message[2] == "nvim_exec_lua" && message[3][1][1] == file
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

/// `worktree ls --json`'s path of the worktree `name`: what a request must carry.
fn listed_path(fx: &AgentRepo, name: &str) -> String {
    fx.listed(name)["path"].as_str().unwrap().to_owned()
}

/// Commits a setup script for every new worktree.
fn commit_setup(fx: &AgentRepo, script: &str) {
    fx.repo
        .write("orca-term.yaml", &format!("scripts:\n  setup: {script}\n"));
    fx.repo.commit_all("setup");
}

/// Runs the handler the agent's settings register for `event`, as claude would, with a payload
/// naming the session, the directory and the event, plus `fields`.
fn fire(worktree: &Path, admin: &Path, event: &str, fields: &Value) {
    let settings: Value =
        serde_json::from_slice(&fs::read(admin.join("claude-settings.json")).unwrap()).unwrap();
    let handler = &settings["hooks"][event][0]["hooks"][0];
    let args: Vec<&str> = handler["args"]
        .as_array()
        .unwrap()
        .iter()
        .map(|arg| arg.as_str().unwrap())
        .collect();
    let mut payload = json!({
        "session_id": "40b1fb34-8b28-4e24-9bcf-31292c24b65f",
        "cwd": worktree,
        "hook_event_name": event,
    });
    for (key, value) in fields.as_object().unwrap() {
        payload[key] = value.clone();
    }
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

/// A turn of the agent of the worktree at `worktree`, from its prompt to its end, as its hooks
/// report it: what the Neovims are asked after the prompt, then after the end.
fn a_turn(worktree: &Path, admin: &Path, neovim: &Neovim) -> (Vec<Value>, Vec<Value>) {
    fire(
        worktree,
        admin,
        "UserPromptSubmit",
        &json!({"prompt": "write it"}),
    );
    let begun = neovim.messages();
    fire(worktree, admin, "Stop", &json!({}));
    (begun, neovim.messages())
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
fn only_the_end_of_a_turn_asks_every_registered_neovim_to_checktime() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let admin = fx.admin_dir(&worktree);
    fx.ok(&["agent", "start", "fix"]);
    let (first, second) = (Neovim::new(), Neovim::new());
    fx.ok(&["nvim", "register", &first.address()]);
    fx.ok(&["nvim", "register", &second.address()]);

    let (begun, ended) = a_turn(&worktree, &admin, &first);
    assert!(begun.is_empty(), "a turn begins: nothing is sent");
    assert_eq!(ended, [checktime()], "a turn ends");
    assert_eq!(second.messages(), [checktime()]);
}

#[test]
fn the_end_of_a_turn_in_the_primary_checkout_asks_to_checktime() {
    let fx = AgentRepo::new();
    let root = fx.repo.root.clone();
    let admin = fx.admin_dir(&root);
    let neovim = Neovim::new();
    fx.ok(&["nvim", "register", &neovim.address()]);

    fx.ok(&["agent", "start", "repo"]);
    let (begun, ended) = a_turn(&root, &admin, &neovim);
    assert!(begun.is_empty());
    assert_eq!(ended, [checktime()]);
}

#[test]
fn nvim_diff_asks_every_registered_neovim_to_open_the_files_diff() {
    let fx = AgentRepo::new();
    fx.repo.write("src/a.rs", "fn a() {}\n");
    fx.repo.commit_all("a");
    let worktree = fx.new_worktree("fix");
    fs::write(worktree.join("src/a.rs"), "fn a() { 1 }\n").unwrap();
    let neovim = Neovim::new();
    fx.ok(&["nvim", "register", &neovim.address()]);
    let path = listed_path(&fx, "fix");
    let base = fx.repo.git(&["rev-parse", "--short=7", "main"]);

    let text = fx.ok(&["nvim", "diff", "fix", "src/a.rs"]);
    assert_eq!(
        text,
        "asked the Neovims of fix to open the diff of src/a.rs\n"
    );
    // The code the core sends, then the worktree, the file, the merge base and the file there.
    assert_eq!(
        neovim.messages(),
        [json!([
            0,
            1,
            "nvim_exec_lua",
            [
                include_str!("../src/domain/nvim/open_diff.lua"),
                [path, "src/a.rs", base.trim(), "fn a() {}\n"]
            ]
        ])]
    );
    let out = fx.run(&["nvim", "diff", "nope", "a.rs", "--json"]);
    assert!(!out.status.success(), "an unknown worktree is refused");
}

#[test]
fn writing_a_worktrees_state_sends_nothing_to_the_neovims() {
    // Only the Neovim client listened to the worktree-changed notice, and it is gone (#194).
    let fx = AgentRepo::new();
    commit_setup(&fx, "echo set up");
    let neovim = Neovim::new();
    fx.ok(&["nvim", "register", &neovim.address()]);

    fx.ok(&["worktree", "new", "fix", "--approve-setup"]);
    fx.ok(&["worktree", "setup", "fix"]);
    fx.ok(&["worktree", "unseen", "fix"]);
    fx.ok(&["worktree", "seen", "fix"]);
    fx.ok(&["worktree", "review", "fix"]);
    fx.ok(&["note", "add", "fix", "a.txt", "--body", "one"]);
    fx.ok(&["note", "edit", "fix", "1", "--body", "uno"]);
    fx.ok(&["note", "rm", "fix", "1"]);
    fx.ok(&["agent", "start", "fix"]);
    fx.ok(&["agent", "stop", "fix"]);
    fx.ok(&["worktree", "rm", "fix"]);
    assert!(neovim.messages().is_empty());
}

#[test]
fn a_stale_address_is_pruned_and_the_command_still_succeeds() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");
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

    let out = fx.run(&["nvim", "diff", "fix", "a.txt", "--json"]);
    assert_eq!(out.status.code(), Some(0));
    assert!(out.stderr.is_empty());

    assert_eq!(registered(&fx), [live.address()]);
    let messages = live.messages();
    assert_eq!(messages.len(), 1);
    assert!(is_diff_of(&messages[0], "a.txt"));
    let _ = fs::remove_dir_all(crashed_dir);
}

#[test]
fn an_address_too_long_for_a_socket_is_skipped_and_kept() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");
    let long = format!("/tmp/{}/nvim.sock", "x".repeat(150));
    fx.ok(&["nvim", "register", &long]);
    fx.ok(&["nvim", "diff", "fix", "a.txt"]);
    assert_eq!(registered(&fx), [long]);
}

#[test]
fn a_neovim_that_never_reads_or_never_accepts_does_not_hold_a_command_up() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");
    // One accepts every connection and never reads it; the other never accepts. Neither answers.
    let silent = Neovim::deaf();
    let held = Arc::new(Mutex::new(Vec::new()));
    let listener = silent.listener.try_clone().unwrap();
    let keep = Arc::clone(&held);
    listener.set_nonblocking(false).unwrap();
    thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            keep.lock().unwrap().push(stream);
        }
    });
    let deaf = Neovim::deaf();
    fx.ok(&["nvim", "register", &silent.address()]);
    fx.ok(&["nvim", "register", &deaf.address()]);

    for _ in 0..3 {
        let start = Instant::now();
        fx.ok(&["nvim", "diff", "fix", "a.txt"]);
        let took = start.elapsed();
        assert!(took < Duration::from_secs(1), "took {took:?}");
    }
    let mut both = vec![silent.address(), deaf.address()];
    both.sort();
    assert_eq!(registered(&fx), both, "neither is pruned: both are alive");
    assert_eq!(deaf.messages().len(), 3);
}

#[test]
fn the_core_waits_for_the_answer_of_a_busy_neovim_before_hanging_up() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");
    let busy = Neovim::answering_after(Duration::from_millis(50));
    fx.ok(&["nvim", "register", &busy.address()]);

    fx.ok(&["nvim", "diff", "fix", "a.txt"]);

    // `messages` fails if the core hung up before an answer was written: Neovim would have dropped
    // the call.
    assert_eq!(busy.messages().len(), 1);
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
