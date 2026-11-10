//! Adapter: a client of a holder whose ends are not this process's terminal (#174), as the
//! application's agent panel is. It sends the panel's keys and size as frames, and hands what the
//! holder sends back to the caller, from a thread of its own, until the connection closes.
//!
//! Unlike [`super::connect`], it leaves this process's directory where it was whenever the socket's
//! path is short enough to connect by: the application has threads that must not see it move.

use std::io::{ErrorKind, Read as _, Write as _};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};

use crate::adapters::env;
use crate::domain::agent::Size;
use crate::domain::protocol::{Decoder, Frame};
use crate::domain::state::{AGENT_SOCKET_FILE, AdminDir};

/// How long a frame to the holder may take to write before the client gives up on it. The holder
/// reads its clients all the time, so only a holder that hangs makes one wait.
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);

/// What a holder said to its client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Heard {
    /// Bytes the agent's terminal printed; the first is the whole replay.
    Output(Vec<u8>),
    /// The agent ended, as the text says (`exit 0`); the holder closes next.
    Ended(String),
    /// The connection closed: the holder is gone, or this client hung up.
    Closed,
}

/// A connection to one worktree's holder. Dropping it detaches, leaving the agent running.
#[derive(Debug)]
pub(crate) struct Client {
    stream: UnixStream,
}

impl Client {
    /// Connects to the holder of the worktree whose admin directory is `admin`, and hands every
    /// frame it sends to `on_heard`, on a thread of its own, ending with [`Heard::Closed`].
    pub(crate) fn connect(
        admin: &AdminDir,
        on_heard: impl FnMut(Heard) + Send + 'static,
    ) -> Result<Self> {
        let stream = connect_staying(admin)?;
        stream
            .set_write_timeout(Some(WRITE_TIMEOUT))
            .context("could not set up the connection to the holder")?;
        let reader = stream
            .try_clone()
            .context("could not set up the connection to the holder")?;
        thread::spawn(move || listen(reader, on_heard));
        Ok(Self { stream })
    }

    /// Types `bytes` into the agent's terminal.
    pub(crate) fn send_input(&self, bytes: &[u8]) -> Result<()> {
        self.send(&Frame::Input(bytes.to_vec()))
    }

    /// Tells the holder this client's terminal size.
    pub(crate) fn send_size(&self, size: Size) -> Result<()> {
        self.send(&Frame::Resize(size))
    }

    fn send(&self, frame: &Frame) -> Result<()> {
        (&self.stream)
            .write_all(&frame.encode())
            .context("could not write to the agent's holder")
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

/// Connects to the socket in `admin` by its absolute path when it fits in a socket address, which
/// leaves the current directory alone. A longer path is connected by its bare name from inside
/// `admin`, and the directory is changed back at once.
fn connect_staying(admin: &AdminDir) -> Result<UnixStream> {
    let socket = admin.agent_socket();
    match UnixStream::connect(&socket) {
        Err(err) if err.kind() == ErrorKind::InvalidInput => {}
        connected => {
            return connected.with_context(|| format!("could not connect to {}", socket.display()));
        }
    }
    let back = env::current_dir()?;
    env::enter_dir(admin.path())?;
    let connected = UnixStream::connect(AGENT_SOCKET_FILE);
    env::enter_dir(&back)?;
    connected.with_context(|| format!("could not connect to {}", socket.display()))
}

/// Reads the holder's frames until the connection closes, handing each to `on_heard`.
fn listen(mut stream: UnixStream, mut on_heard: impl FnMut(Heard)) {
    let mut decoder = Decoder::default();
    let mut buf = vec![0u8; 16 * 1024];
    loop {
        let n = match stream.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        decoder.push(&buf[..n]);
        while let Ok(Some(frame)) = decoder.next_frame() {
            match frame {
                Frame::Output(bytes) => on_heard(Heard::Output(bytes)),
                Frame::Exit(text) => on_heard(Heard::Ended(text)),
                Frame::Input(_) | Frame::Resize(_) | Frame::Stop => {}
            }
        }
    }
    on_heard(Heard::Closed);
}
