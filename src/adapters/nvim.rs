//! Adapter: the server sockets of the running Neovims. The core only ever sends one request to
//! each and hangs up: it connects, writes the request, waits for Neovim's answer, and closes.
//! Telling a Neovim a worktree changed needs only the answer's first byte; the doctor's question
//! (#179) reads the few bytes of its answer, and the domain decides what they mean.
//!
//! The wait matters: Neovim drops a call still waiting to run when its connection closes, so a
//! message written and closed at once may never run (Neovim 0.11 on Linux drops it every time).
//! Neovim answers a request only after the call ran, so the first byte of the answer is the proof.
//!
//! Sending is bounded in time and never fails: the caller only learns, for each Neovim, whether
//! the call ran, whether the Neovim is gone (nothing at the path, or nobody listening there), or
//! whether it was skipped for any other reason (a path too long for a socket address, no answer in
//! time, a permission).

use std::io::{self, ErrorKind, Read as _, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// How long sending may take, to every Neovim at once: connecting, writing and the answer.
const SEND_TIMEOUT: Duration = Duration::from_millis(200);

/// What became of one message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sent {
    /// It was written to the socket and Neovim answered: the call ran.
    Delivered,
    /// The Neovim is gone: there is no socket at the path, or nobody listens on it.
    Gone,
    /// It could not be sent for another reason, or not answered in time; the Neovim may still be
    /// there.
    Skipped,
}

/// Sends `message` to every unix socket in `sockets` at once, all within [`SEND_TIMEOUT`], and
/// returns what became of each, in the same order.
pub(crate) fn send_all(sockets: &[PathBuf], message: &[u8]) -> Vec<Sent> {
    exchange_all(sockets, message, 1)
        .into_iter()
        .map(|outcome| match outcome {
            Outcome::Answered(_) => Sent::Delivered,
            Outcome::Gone => Sent::Gone,
            Outcome::Skipped => Sent::Skipped,
        })
        .collect()
}

/// Asks every unix socket in `sockets` the request `message`, at once and within
/// [`SEND_TIMEOUT`], and returns the first `answer_len` bytes of each answer, in the same order:
/// `None` for a Neovim that is gone, did not answer in time, or answered fewer bytes.
pub(crate) fn ask_all(
    sockets: &[PathBuf],
    message: &[u8],
    answer_len: usize,
) -> Vec<Option<Vec<u8>>> {
    exchange_all(sockets, message, answer_len)
        .into_iter()
        .map(|outcome| match outcome {
            Outcome::Answered(answer) => Some(answer),
            Outcome::Gone | Outcome::Skipped => None,
        })
        .collect()
}

/// What became of one request, with the answer's first bytes when it was answered.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Outcome {
    Answered(Vec<u8>),
    Gone,
    Skipped,
}

/// Sends `message` to every socket at once and reads the first `answer_len` bytes of each
/// answer. Each exchange runs on a thread of its own, so a Neovim that never accepts, never reads
/// or never answers cannot hold the caller up: past the timeout its thread is left behind, and
/// ends with the process.
fn exchange_all(sockets: &[PathBuf], message: &[u8], answer_len: usize) -> Vec<Outcome> {
    let (sender, receiver) = mpsc::channel();
    for (index, socket) in sockets.iter().enumerate() {
        let sender = sender.clone();
        let socket = socket.clone();
        let message = message.to_vec();
        thread::spawn(move || {
            let _ = sender.send((index, request(&socket, &message, answer_len)));
        });
    }
    drop(sender);

    // Anything not heard from by the deadline was skipped.
    let mut outcomes = vec![Outcome::Skipped; sockets.len()];
    let deadline = Instant::now() + SEND_TIMEOUT;
    for _ in sockets {
        let left = deadline.saturating_duration_since(Instant::now());
        match receiver.recv_timeout(left) {
            Ok((index, outcome)) => outcomes[index] = outcome,
            Err(_) => break,
        }
    }
    outcomes
}

/// Connects to `socket`, writes `message`, reads the first `answer_len` bytes of Neovim's
/// answer, and closes.
fn request(socket: &Path, message: &[u8], answer_len: usize) -> Outcome {
    let mut stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        // Nothing at the path, or a socket file nobody listens on: the Neovim is gone.
        Err(err) if is_gone(&err) => return Outcome::Gone,
        // Too long for a socket address, no permission, or anything else: leave it be.
        Err(_) => return Outcome::Skipped,
    };
    let timeouts = stream
        .set_write_timeout(Some(SEND_TIMEOUT))
        .and_then(|()| stream.set_read_timeout(Some(SEND_TIMEOUT)));
    if timeouts.is_err() || stream.write_all(message).is_err() {
        return Outcome::Skipped;
    }
    // The answer's first bytes mean the call ran; the rest of it is not needed.
    let mut answer = vec![0_u8; answer_len];
    match stream.read_exact(&mut answer) {
        Ok(()) => Outcome::Answered(answer),
        // Closed without a whole answer, or no answer in time.
        Err(_) => Outcome::Skipped,
    }
}

/// Whether a failed connect says the Neovim is gone.
fn is_gone(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        ErrorKind::NotFound | ErrorKind::ConnectionRefused
    )
}
