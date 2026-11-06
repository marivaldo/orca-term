//! One real terminal: a child process on a PTY, its output parsed into a vt100 screen.
//!
//! A reader thread feeds the parser and pings the main loop; the main loop writes keys and
//! resizes. The parser sits behind a mutex both sides lock for a moment.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use tui_term::vt100;

use crate::app::Msg;

/// How many lines a terminal keeps above its screen, for the mouse wheel.
const SCROLLBACK: usize = 2000;

/// Tells the main loop a terminal has something to draw. One ping at a time is enough, so a
/// terminal printing fast does not flood the channel.
#[derive(Clone)]
pub struct Wake {
    pub tx: Sender<Msg>,
    pub pending: Arc<AtomicBool>,
}

impl Wake {
    fn ping(&self) {
        if !self.pending.swap(true, Ordering::SeqCst) {
            let _ = self.tx.send(Msg::Wake);
        }
    }
}

/// What the vt100 parser leaves to us: the answers a program waits for when it asks the terminal
/// something, and the cursor's shape.
#[derive(Default)]
pub struct Extras {
    /// Bytes to write back to the child.
    replies: Vec<u8>,
    /// The last DECSCUSR value (`CSI Ps SP q`): 0 default, 1-2 block, 3-4 underline, 5-6 bar.
    pub cursor_shape: u16,
}

impl vt100::Callbacks for Extras {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        let first = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (i1, i2, c) {
            // Where is the cursor? (DSR 6)
            (None, None, 'n') if first == 6 => {
                let (row, col) = screen.cursor_position();
                let answer = format!("\x1b[{};{}R", row + 1, col + 1);
                self.replies.extend(answer.bytes());
            }
            // Are you there? (DSR 5)
            (None, None, 'n') if first == 5 => self.replies.extend(b"\x1b[0n"),
            // What are you? (DA1). Programs send this last, to know the other questions they
            // asked got no answer, so it must be answered or they wait for a timeout.
            (None, None, 'c') if first == 0 => self.replies.extend(b"\x1b[?62;22c"),
            (Some(b'>'), None, 'c') => self.replies.extend(b"\x1b[>0;0;0c"),
            // Is this mode supported? (DECRQM): no, whichever it is.
            (Some(b'?'), Some(b'$'), 'p') => {
                self.replies.extend(format!("\x1b[?{first};0$y").bytes());
            }
            (Some(b' '), None, 'q') => self.cursor_shape = first,
            _ => {}
        }
    }
}

pub type Parser = vt100::Parser<Extras>;

/// The running half of a terminal; missing when the command could not start.
struct Process {
    master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
}

pub struct Term {
    parser: Arc<Mutex<Parser>>,
    process: Option<Process>,
    exited: Arc<AtomicBool>,
    /// Rows and columns.
    size: (u16, u16),
    argv: Vec<String>,
    cwd: PathBuf,
    wake: Wake,
}

/// A poisoned lock still holds a usable screen; the prototype keeps going.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Term {
    /// Starts `argv` in `cwd` on a PTY of `rows` by `cols`. A command that cannot start gives a
    /// terminal that says why and counts as exited, so Enter tries again.
    pub fn spawn(argv: &[String], cwd: &Path, rows: u16, cols: u16, wake: &Wake) -> Term {
        let (rows, cols) = (rows.max(2), cols.max(2));
        let parser = Arc::new(Mutex::new(Parser::new_with_callbacks(
            rows,
            cols,
            SCROLLBACK,
            Extras::default(),
        )));
        let exited = Arc::new(AtomicBool::new(false));
        let mut term = Term {
            parser,
            process: None,
            exited,
            size: (rows, cols),
            argv: argv.to_vec(),
            cwd: cwd.to_path_buf(),
            wake: wake.clone(),
        };
        match term.start() {
            Ok(process) => term.process = Some(process),
            Err(error) => {
                let text = format!("could not start `{}`: {error}\r\n", argv.join(" "));
                lock(&term.parser).process(text.as_bytes());
                term.exited.store(true, Ordering::SeqCst);
            }
        }
        term
    }

    fn start(&self) -> Result<Process, Box<dyn std::error::Error>> {
        let (rows, cols) = self.size;
        let pair = native_pty_system().openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;
        let mut command = CommandBuilder::new(&self.argv[0]);
        command.args(&self.argv[1..]);
        command.cwd(&self.cwd);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        let child = pair.slave.spawn_command(command)?;
        // Only the child holds the PTY's other end now, so reading ends when the child does.
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader()?;
        let writer = Arc::new(Mutex::new(pair.master.take_writer()?));

        let parser = Arc::clone(&self.parser);
        let exited = Arc::clone(&self.exited);
        let replies_to = Arc::clone(&writer);
        let wake = self.wake.clone();
        std::thread::spawn(move || {
            let mut buffer = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(n) => {
                        let replies = {
                            let mut parser = lock(&parser);
                            parser.process(&buffer[..n]);
                            std::mem::take(&mut parser.callbacks_mut().replies)
                        };
                        if !replies.is_empty() {
                            let mut writer = lock(&replies_to);
                            let _ = writer.write_all(&replies);
                            let _ = writer.flush();
                        }
                        wake.ping();
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            exited.store(true, Ordering::SeqCst);
            wake.ping();
        });

        Ok(Process {
            master: pair.master,
            child,
            writer,
        })
    }

    /// The parser, locked: its screen is what the panel draws.
    pub fn parser(&self) -> MutexGuard<'_, Parser> {
        lock(&self.parser)
    }

    pub fn exited(&self) -> bool {
        self.exited.load(Ordering::SeqCst)
    }

    /// Sends bytes to the child, as if typed.
    pub fn write(&self, bytes: &[u8]) {
        if let Some(process) = &self.process {
            let mut writer = lock(&process.writer);
            let _ = writer.write_all(bytes);
            let _ = writer.flush();
        }
    }

    /// Gives the child's screen a size, when the panel that shows it has changed its own.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        if (rows, cols) == self.size || rows == 0 || cols == 0 {
            return;
        }
        self.size = (rows, cols);
        lock(&self.parser).screen_mut().set_size(rows, cols);
        if let Some(process) = &self.process {
            let _ = process.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            });
        }
    }

    /// Moves the view over the lines above the screen: positive is up. Back to 0 is the live
    /// screen.
    pub fn scroll(&self, lines: isize) {
        let mut parser = lock(&self.parser);
        let now = parser.screen().scrollback();
        parser
            .screen_mut()
            .set_scrollback(now.saturating_add_signed(lines));
    }

    pub fn scroll_to_bottom(&self) {
        let mut parser = lock(&self.parser);
        if parser.screen().scrollback() != 0 {
            parser.screen_mut().set_scrollback(0);
        }
    }

    /// Starts the same command again, on a fresh screen.
    pub fn respawn(&mut self) {
        self.kill();
        *self = Term::spawn(&self.argv, &self.cwd, self.size.0, self.size.1, &self.wake);
    }

    /// Hangs up on the child without waiting: the first half of `kill`, so that many terminals
    /// die at once when the prototype quits.
    pub fn hang_up(&mut self) {
        if let Some(process) = &mut self.process {
            let _ = process.child.clone_killer().kill();
        }
    }

    /// Ends the child (SIGHUP, then SIGKILL if it is still there) and closes its PTY.
    pub fn kill(&mut self) {
        if let Some(mut process) = self.process.take() {
            let gone = matches!(process.child.try_wait(), Ok(Some(_)));
            if !gone {
                let _ = process.child.kill();
                for _ in 0..20 {
                    if matches!(process.child.try_wait(), Ok(Some(_))) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }
}
