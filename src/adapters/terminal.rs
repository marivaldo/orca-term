//! Adapter: the terminal `agent attach` runs in, and the bytes between it and the holder. It puts
//! the terminal in raw mode, sends its size (and every change of it) to the holder, forwards what
//! is typed as input, writes what the holder sends to the terminal, and restores the terminal on
//! the way out. Without a terminal (a pipe, as the tests use) it only moves the bytes, and the
//! end of its input detaches.
//!
//! This is the one adapter that writes to stdout: not a report, the agent's own screen.

use std::io::{self, Read as _, Write as _};
use std::net::Shutdown;
use std::os::fd::AsFd as _;
use std::os::unix::net::UnixStream;
use std::thread;

use anyhow::{Context, Result};
use rustix::termios::{self, OptionalActions, Termios};

use crate::domain::agent::{AttachEnd, Size};
use crate::domain::protocol::{Decoder, Frame};

/// Ctrl-\, the key that detaches when the terminal is a tty.
const DETACH_KEY: u8 = 0x1c;

/// Written to the terminal on the way out: the agent's screen modes left behind are undone (the
/// alternate screen, a hidden cursor, bracketed paste, mouse and focus reports, the kitty
/// keyboard flags), then the cursor moves to a fresh line.
const MODE_RESET: &[u8] =
    b"\x1b[?1049l\x1b[?25h\x1b[?2004l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[?1004l\x1b[<u\r\n";

/// How the person may detach from the agent's terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DetachKey {
    /// Ctrl-\ detaches.
    CtrlBackslash,
    /// No key detaches: closing the terminal does. The Neovim client asks for this, since Ctrl-\
    /// is Neovim's own prefix inside a terminal.
    None,
}

/// The size of the terminal on stdin, when stdin is one.
pub(crate) fn stdin_size() -> Option<Size> {
    let stdin = io::stdin();
    if !termios::isatty(stdin.as_fd()) {
        return None;
    }
    let winsize = termios::tcgetwinsize(stdin.as_fd()).ok()?;
    (winsize.ws_row > 0 && winsize.ws_col > 0).then_some(Size {
        rows: winsize.ws_row,
        cols: winsize.ws_col,
    })
}

/// Joins this process's terminal to the holder on `stream` until the person detaches, the holder
/// closes the connection or the agent ends.
pub(crate) fn attach(stream: UnixStream, detach_key: DetachKey) -> Result<AttachEnd> {
    let stdin = io::stdin();
    let is_tty = termios::isatty(stdin.as_fd());
    let _restore = if is_tty {
        Some(RawMode::enter()?)
    } else {
        None
    };
    if is_tty {
        send_size(&mut stream.try_clone()?)?;
        watch_size(stream.try_clone()?)?;
    }
    let detach_key = if is_tty { detach_key } else { DetachKey::None };
    forward_stdin(stream.try_clone()?, detach_key);
    let end = copy_output(stream)?;
    if is_tty {
        let mut stdout = io::stdout().lock();
        let _ = stdout.write_all(MODE_RESET);
        let _ = stdout.flush();
    }
    Ok(end)
}

/// The terminal's settings before raw mode, put back when dropped.
struct RawMode(Termios);

impl RawMode {
    fn enter() -> Result<Self> {
        let stdin = io::stdin();
        let saved = termios::tcgetattr(stdin.as_fd()).context("could not read the terminal")?;
        let mut raw = saved.clone();
        raw.make_raw();
        termios::tcsetattr(stdin.as_fd(), OptionalActions::Now, &raw)
            .context("could not put the terminal in raw mode")?;
        Ok(Self(saved))
    }
}

impl Drop for RawMode {
    fn drop(&mut self) {
        let _ = termios::tcsetattr(io::stdin().as_fd(), OptionalActions::Now, &self.0);
    }
}

/// Sends the terminal's size to the holder.
fn send_size(stream: &mut UnixStream) -> Result<()> {
    if let Some(size) = stdin_size() {
        stream
            .write_all(&Frame::Resize(size).encode())
            .context("could not send the terminal size to the holder")?;
    }
    Ok(())
}

/// Sends the size again whenever the terminal changes it (SIGWINCH).
fn watch_size(mut stream: UnixStream) -> Result<()> {
    let mut signals = signal_hook::iterator::Signals::new([signal_hook::consts::SIGWINCH])
        .context("could not watch the terminal size")?;
    thread::spawn(move || {
        for _ in signals.forever() {
            if send_size(&mut stream).is_err() {
                return;
            }
        }
    });
    Ok(())
}

/// Forwards what is typed to the holder as input. The detach key, when there is one, sends what
/// came before it and closes the connection. The end of a piped stdin closes only the sending
/// half, so the output keeps coming until the holder closes.
fn forward_stdin(mut stream: UnixStream, detach_key: DetachKey) {
    thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut buf = [0u8; 4096];
        loop {
            let n = match stdin.read(&mut buf) {
                Ok(0) | Err(_) => {
                    let _ = stream.shutdown(Shutdown::Write);
                    return;
                }
                Ok(n) => n,
            };
            let chunk = &buf[..n];
            let detach_at = match detach_key {
                DetachKey::CtrlBackslash => chunk.iter().position(|&b| b == DETACH_KEY),
                DetachKey::None => None,
            };
            if let Some(at) = detach_at {
                let _ = stream.write_all(&Frame::Input(chunk[..at].to_vec()).encode());
                let _ = stream.shutdown(Shutdown::Both);
                return;
            }
            if stream
                .write_all(&Frame::Input(chunk.to_vec()).encode())
                .is_err()
            {
                return;
            }
        }
    });
}

/// Writes the holder's output to the terminal until the holder closes, remembering the agent's
/// end if the holder reported it.
fn copy_output(mut stream: UnixStream) -> Result<AttachEnd> {
    let mut stdout = io::stdout().lock();
    let mut decoder = Decoder::default();
    let mut buf = vec![0u8; 16 * 1024];
    let mut end = AttachEnd::Detached;
    loop {
        let n = match stream.read(&mut buf) {
            Ok(0) | Err(_) => return Ok(end),
            Ok(n) => n,
        };
        decoder.push(&buf[..n]);
        while let Some(frame) = decoder
            .next_frame()
            .context("the holder sent bytes that are not the protocol")?
        {
            match frame {
                Frame::Output(bytes) => {
                    stdout
                        .write_all(&bytes)
                        .context("could not write to the terminal")?;
                    stdout.flush().context("could not write to the terminal")?;
                }
                Frame::Exit(text) => end = AttachEnd::AgentEnded(text),
                Frame::Input(_) | Frame::Resize(_) | Frame::Stop => {}
            }
        }
    }
}
