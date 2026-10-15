//! Adapter: a running Neovim's server socket. The core only ever sends one message to it and
//! hangs up: it connects, writes, and closes, reading nothing back.
//!
//! Sending is bounded in time and never fails: the caller only learns whether the message went,
//! whether the Neovim is gone (nothing at the path, or nobody listening there), or whether it was
//! skipped for any other reason (a path too long for a socket address, a timeout, a permission).

use std::io::{self, ErrorKind, Write as _};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

/// How long one send may take, connecting and writing together.
const SEND_TIMEOUT: Duration = Duration::from_millis(200);

/// What became of one message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Sent {
    /// It was written to the socket.
    Delivered,
    /// The Neovim is gone: there is no socket at the path, or nobody listens on it.
    Gone,
    /// It could not be sent for another reason, or not in time; the Neovim may still be there.
    Skipped,
}

/// Sends `message` to the unix socket at `socket`, within [`SEND_TIMEOUT`]. The connect and the
/// write run on a thread of their own, so a Neovim that never accepts nor reads cannot hold the
/// caller up: past the timeout the thread is left behind, and ends with the process.
pub(crate) fn send(socket: &Path, message: &[u8]) -> Sent {
    let (sender, receiver) = mpsc::channel();
    let socket: PathBuf = socket.to_owned();
    let message = message.to_vec();
    thread::spawn(move || {
        let _ = sender.send(connect_and_write(&socket, &message));
    });
    receiver.recv_timeout(SEND_TIMEOUT).unwrap_or(Sent::Skipped)
}

/// Connects to `socket`, writes `message` and closes.
fn connect_and_write(socket: &Path, message: &[u8]) -> Sent {
    let mut stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        // Nothing at the path, or a socket file nobody listens on: the Neovim is gone.
        Err(err) if is_gone(&err) => return Sent::Gone,
        // Too long for a socket address, no permission, or anything else: leave it be.
        Err(_) => return Sent::Skipped,
    };
    if stream.set_write_timeout(Some(SEND_TIMEOUT)).is_err() {
        return Sent::Skipped;
    }
    match stream.write_all(message) {
        Ok(()) => Sent::Delivered,
        Err(_) => Sent::Skipped,
    }
}

/// Whether a failed connect says the Neovim is gone.
fn is_gone(err: &io::Error) -> bool {
    matches!(
        err.kind(),
        ErrorKind::NotFound | ErrorKind::ConnectionRefused
    )
}
