//! Adapter: a program the application runs on a terminal of its own, a PTY (#176): a worktree's
//! Neovim or shell. Unlike the agent, no holder keeps it: it lives as long as the application, and
//! ending the application ends it.
//!
//! Three threads per program, all blocking: the reader hands what the program prints to the
//! caller and, once the PTY closes, waits for the program and reports how it ended; the writer
//! types what the caller sends, so a program that stops reading never holds up the caller; the
//! caller's own thread resizes and ends it. Signals reach the program only under the mutex that
//! reaps it, and only while it runs, so a pid reused by another process is never hit.

use std::io::{Read as _, Write as _};
use std::os::unix::process::ExitStatusExt as _;
use std::path::Path;
use std::process::Child;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use pty_process::blocking::Pty;
use rustix::process::{Pid, Signal};

use crate::domain::agent::Size;
use crate::domain::protocol;

/// The terminal the programs are told they run on: the one the panel's emulator speaks.
const TERM: &str = "xterm-256color";

/// Between two looks at whether a program ended.
const REAP_EVERY: Duration = Duration::from_millis(20);

/// What a program printed, or how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Printed {
    /// Bytes its terminal printed.
    Output(Vec<u8>),
    /// It ended, as the text says (`exit 0`, `signal 1`); nothing comes after.
    Ended(String),
}

/// A program running on a PTY of its own.
pub(crate) struct Process {
    pty: Arc<Pty>,
    child: Arc<Mutex<Child>>,
    typed: Sender<Vec<u8>>,
}

impl std::fmt::Debug for Process {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Process")
            .field("child", &self.child)
            .finish_non_exhaustive()
    }
}

impl Process {
    /// Starts `argv` in `dir` on a PTY of `size`, and hands what it prints to `on_printed`, on a
    /// thread of its own, ending with [`Printed::Ended`].
    pub(crate) fn start(
        argv: &[String],
        dir: &Path,
        size: Size,
        on_printed: impl FnMut(Printed) + Send + 'static,
    ) -> Result<Self> {
        let (program, rest) = argv.split_first().context("no program to run")?;
        let (pty, pts) = pty_process::blocking::open().context("could not open a pty")?;
        pty.resize(pty_size(size))
            .context("could not size the pty")?;
        let child = pty_process::blocking::Command::new(program)
            .args(rest)
            .current_dir(dir)
            .env("TERM", TERM)
            .spawn(pts)
            .with_context(|| format!("could not start {program}"))?;
        let pty = Arc::new(pty);
        let child = Arc::new(Mutex::new(child));
        let (typed, to_type) = mpsc::channel::<Vec<u8>>();
        let writer = Arc::clone(&pty);
        thread::spawn(move || write(&writer, &to_type));
        let (reader, reaped) = (Arc::clone(&pty), Arc::clone(&child));
        thread::spawn(move || read(&reader, &reaped, on_printed));
        Ok(Self { pty, child, typed })
    }

    /// Types `bytes` into the program.
    pub(crate) fn type_bytes(&self, bytes: Vec<u8>) {
        let _ = self.typed.send(bytes);
    }

    /// Makes the program's terminal `size` large.
    pub(crate) fn resize(&self, size: Size) {
        let _ = self.pty.resize(pty_size(size));
    }

    /// Hangs the program up, as closing its terminal would.
    pub(crate) fn hang_up(&self) {
        signal(&self.child, Signal::HUP);
    }

    /// Waits until `deadline` for the program to end, then kills it if it has not.
    pub(crate) fn end_by(&self, deadline: Instant) {
        while Instant::now() < deadline {
            if !matches!(lock(&self.child).try_wait(), Ok(None)) {
                return;
            }
            thread::sleep(REAP_EVERY);
        }
        signal(&self.child, Signal::KILL);
        let _ = lock(&self.child).wait();
    }
}

/// Types into the program whatever comes from `to_type`, until the caller is gone or the program
/// is: a program that is gone says so through its reader.
fn write(pty: &Pty, to_type: &Receiver<Vec<u8>>) {
    for bytes in to_type {
        if (&*pty).write_all(&bytes).is_err() {
            return;
        }
    }
}

/// Reads what the program prints until its PTY closes, then waits for it to end.
fn read(pty: &Pty, child: &Mutex<Child>, mut on_printed: impl FnMut(Printed)) {
    let mut buf = vec![0u8; 16 * 1024];
    // The PTY answers 0 (macOS) or an error (Linux) once the program is gone.
    while let Ok(n @ 1..) = (&*pty).read(&mut buf) {
        on_printed(Printed::Output(buf[..n].to_vec()));
    }
    let status = loop {
        match lock(child).try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        thread::sleep(REAP_EVERY);
    };
    let how = status.map_or_else(
        || "ended".to_owned(),
        |status| protocol::exit_text(status.code(), status.signal()),
    );
    on_printed(Printed::Ended(how));
}

/// Sends `signal` to the program's process group while it runs.
fn signal(child: &Mutex<Child>, signal: Signal) {
    let mut child = lock(child);
    if matches!(child.try_wait(), Ok(None)) {
        let _ = rustix::process::kill_process_group(Pid::from_child(&child), signal);
    }
}

/// The guard even when another thread panicked holding it: the child is still the child.
fn lock(child: &Mutex<Child>) -> MutexGuard<'_, Child> {
    child.lock().unwrap_or_else(PoisonError::into_inner)
}

fn pty_size(size: Size) -> pty_process::Size {
    pty_process::Size::new(size.rows, size.cols)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Starts `script` under `/bin/sh` and collects what it prints until it ends.
    fn start(script: &str) -> (Process, mpsc::Receiver<Printed>) {
        let (sender, printed) = mpsc::channel();
        let argv = ["/bin/sh".to_owned(), "-c".to_owned(), script.to_owned()];
        let size = Size { rows: 5, cols: 40 };
        let process = Process::start(&argv, Path::new("/"), size, move |out| {
            let _ = sender.send(out);
        })
        .unwrap();
        (process, printed)
    }

    /// Everything printed until the end, and how it ended.
    fn until_ended(printed: &mpsc::Receiver<Printed>) -> (String, String) {
        let mut text = Vec::new();
        loop {
            match printed.recv_timeout(Duration::from_secs(10)).unwrap() {
                Printed::Output(bytes) => text.extend(bytes),
                Printed::Ended(how) => return (String::from_utf8_lossy(&text).into_owned(), how),
            }
        }
    }

    #[test]
    fn a_program_runs_on_a_terminal_in_its_directory_and_reads_what_is_typed() {
        let (process, printed) =
            start("echo \"$TERM $(pwd) $(stty size)\"; read line; echo \"got $line\"; exit 3");
        process.type_bytes(b"hi\r".to_vec());
        let (text, how) = until_ended(&printed);
        assert!(text.contains("xterm-256color / 5 40"), "{text:?}");
        assert!(text.contains("got hi"), "{text:?}");
        assert_eq!(how, "exit 3");
    }

    #[test]
    fn hanging_up_ends_the_program_and_a_deaf_one_is_killed() {
        let (process, printed) = start("echo up; while :; do sleep 1; done");
        process.hang_up();
        process.end_by(Instant::now() + Duration::from_secs(5));
        assert_eq!(until_ended(&printed).1, "signal 1");

        let (process, printed) = start("trap '' HUP; echo up; while :; do sleep 1; done");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !matches!(
            printed.recv_timeout(Duration::from_secs(10)),
            Ok(Printed::Output(_))
        ) {
            assert!(Instant::now() < deadline, "never started");
        }
        process.hang_up();
        process.end_by(Instant::now() + Duration::from_millis(300));
        assert_eq!(until_ended(&printed).1, "signal 9");
    }
}
