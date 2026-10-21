//! Adapter: the events of an agent whose hooks cannot run orca-term themselves: opencode's plugin,
//! inside the sandbox that denies writes to the admin directory (#35). The holder makes a pipe and
//! hands its write end to the agent, its number in `ORCA_TERM_EVENTS_FD`; the agent writes one
//! JSON line per event, and the holder, outside the sandbox, runs
//! `orca-term hook <agent> --admin <dir>` for each line, one after the other, the line on its
//! stdin, as claude runs its hooks.
//!
//! The pipe's read end is close-on-exec, so neither the agent nor a hook holds it; the write end
//! is not, so the agent inherits it, and the holder closes its own copy at once. A line longer
//! than [`MAX_LINE`] is dropped whole: the hook reads nothing of it.
#![expect(
    clippy::disallowed_types,
    reason = "the holder runs orca-term's hook for each event the agent reports"
)]

use std::io::{BufRead as _, BufReader, PipeReader, PipeWriter, Read as _, Write as _};
use std::os::fd::AsRawFd as _;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// The longest line read as an event: far more than the plugin ever writes.
const MAX_LINE: u64 = 256 * 1024;

/// How long one hook may run before it is killed and the next line read: more than the hook's
/// own wait for its lock and the snapshot it may take.
const HOOK_TIMEOUT: Duration = Duration::from_secs(10);

/// The pipe the agent reports through.
#[derive(Debug)]
pub(crate) struct Pipe {
    /// The holder's end, close-on-exec.
    pub(crate) reader: PipeReader,
    /// The agent's end, inherited by the agent; the holder drops it once the agent runs.
    pub(crate) writer: PipeWriter,
}

impl Pipe {
    /// Makes the pipe, its write end inheritable.
    pub(crate) fn open() -> Result<Self> {
        let (reader, writer) = std::io::pipe().context("could not make the events pipe")?;
        rustix::io::fcntl_setfd(&writer, rustix::io::FdFlags::empty())
            .context("could not hand the events pipe to the agent")?;
        Ok(Self { reader, writer })
    }

    /// The number the agent finds its end under, for `ORCA_TERM_EVENTS_FD`.
    pub(crate) fn writer_fd(&self) -> String {
        self.writer.as_raw_fd().to_string()
    }
}

/// How to run the hook for one event: `<exe> hook <agent> --admin <admin>`, in the worktree.
#[derive(Debug, Clone)]
pub(crate) struct Hook {
    /// This orca-term, by its absolute path.
    pub(crate) exe: PathBuf,
    /// The agent as its hooks name it: `opencode`.
    pub(crate) agent: String,
    pub(crate) admin: PathBuf,
    pub(crate) worktree: PathBuf,
}

/// Starts the thread that reads the agent's events from `reader` and runs `hook` for each, in
/// order. It ends once every holder of the write end has closed it: the agent and whatever it
/// started that kept it.
pub(crate) fn start(reader: PipeReader, hook: Hook) -> JoinHandle<()> {
    thread::spawn(move || {
        let mut lines = BufReader::new(reader);
        loop {
            match next_line(&mut lines) {
                Line::Event(line) => run_hook(&hook, &line),
                Line::TooLong => {}
                Line::Closed => return,
            }
        }
    })
}

/// What reading the next line of the pipe gave.
#[derive(Debug, PartialEq, Eq)]
enum Line {
    /// A line, its newline included (the last one may lack it).
    Event(Vec<u8>),
    /// A line too long to read, skipped up to its end.
    TooLong,
    /// The pipe is closed, or cannot be read.
    Closed,
}

/// The next line of `lines`.
fn next_line(lines: &mut BufReader<PipeReader>) -> Line {
    let mut line = Vec::new();
    let Ok(read) = lines.by_ref().take(MAX_LINE).read_until(b'\n', &mut line) else {
        return Line::Closed;
    };
    if read == 0 {
        return Line::Closed;
    }
    if line.ends_with(b"\n") || read < usize::try_from(MAX_LINE).unwrap_or(usize::MAX) {
        return Line::Event(line);
    }
    // Too long: skip the rest of it.
    let mut rest = Vec::new();
    while !rest.ends_with(b"\n") {
        rest.clear();
        match lines.by_ref().take(MAX_LINE).read_until(b'\n', &mut rest) {
            Ok(0) | Err(_) => return Line::Closed,
            Ok(_) => {}
        }
    }
    Line::TooLong
}

/// Runs the hook with `line` on its stdin and waits for it, at most [`HOOK_TIMEOUT`]. A hook
/// that cannot start, or that hangs, is given up on: the event is lost, the next one is read.
fn run_hook(hook: &Hook, line: &[u8]) {
    let started = Command::new(&hook.exe)
        .arg("hook")
        .arg(&hook.agent)
        .arg("--admin")
        .arg(&hook.admin)
        .current_dir(&hook.worktree)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = started else {
        return;
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(line);
    }
    wait_within(&mut child, HOOK_TIMEOUT);
}

/// Waits for `child` to exit, killing it once `timeout` has passed.
fn wait_within(child: &mut Child, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(_)) | Err(_) => return,
            Ok(None) if Instant::now() > deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return;
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_are_read_whole_and_a_line_too_long_is_dropped() {
        let pipe = Pipe::open().unwrap();
        let mut writer = pipe.writer;
        let long = vec![b'x'; usize::try_from(MAX_LINE).unwrap() + 10];
        let feeder = thread::spawn(move || {
            writer.write_all(b"{\"type\":\"start\"}\n").unwrap();
            writer.write_all(&long).unwrap();
            writer
                .write_all(b"\n{\"type\":\"end\"}\nno newline")
                .unwrap();
        });
        let mut lines = BufReader::new(pipe.reader);
        assert_eq!(
            next_line(&mut lines),
            Line::Event(b"{\"type\":\"start\"}\n".to_vec())
        );
        assert_eq!(
            next_line(&mut lines),
            Line::TooLong,
            "the long line is dropped"
        );
        assert_eq!(
            next_line(&mut lines),
            Line::Event(b"{\"type\":\"end\"}\n".to_vec())
        );
        feeder.join().unwrap();
        assert_eq!(
            next_line(&mut lines),
            Line::Event(b"no newline".to_vec()),
            "the last line, cut by the end of the pipe, is still read"
        );
        assert_eq!(next_line(&mut lines), Line::Closed);
    }

    #[test]
    fn the_agents_end_is_inheritable_and_the_holders_is_not() {
        let pipe = Pipe::open().unwrap();
        let cloexec = rustix::io::FdFlags::CLOEXEC;
        let writer = rustix::io::fcntl_getfd(&pipe.writer).unwrap();
        let reader = rustix::io::fcntl_getfd(&pipe.reader).unwrap();
        assert!(!writer.contains(cloexec));
        assert!(reader.contains(cloexec));
        assert_eq!(pipe.writer_fd(), pipe.writer.as_raw_fd().to_string());
    }
}
