//! Adapter: the holder, the detached process that keeps one worktree's agent running and holds its
//! screen (ADR 0009). This file is what the commands use: starting a holder and waiting for its
//! first word, connecting to one through its socket, and asking it to stop. The holder's own
//! process is `holder::process`.
//!
//! The socket is bound and connected by its bare name after a change of directory into the admin
//! directory: a unix socket address holds about 100 bytes, and admin directories are longer. Every
//! path handed to the holder is absolute, so the change of directory is harmless.
#![expect(
    clippy::disallowed_types,
    reason = "this adapter is the one place that starts the holder process"
)]

pub(crate) mod process;

use std::io::{BufRead as _, BufReader, Write as _};
use std::net::Shutdown;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::domain::agent::Size;
use crate::domain::protocol::Frame;
use crate::domain::state::{AGENT_SOCKET_FILE, AdminDir};

/// How long `agent start` waits for the holder's first word.
const READY_TIMEOUT: Duration = Duration::from_secs(10);

/// What a holder is told when started: the worktree's admin directory and path, both absolute,
/// the agent's first terminal size and the command that is the agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HolderArgs {
    pub(crate) admin: AdminDir,
    pub(crate) worktree: PathBuf,
    pub(crate) size: Size,
    /// The agent's command line, program first.
    pub(crate) argv: Vec<String>,
}

/// Starts a holder for `args` as a detached child of this process, with the variables in
/// `strip_vars` removed from its environment, and waits for its first word: the holder's pid once
/// the agent runs, or why it could not. The holder gets no handle on this process's output, so
/// `agent start` can return while the holder lives on; it is reparented when `agent start` exits.
pub(crate) fn spawn(args: &HolderArgs, strip_vars: &[&str]) -> Result<u32> {
    let exe = std::env::current_exe().context("could not find the orca-term binary")?;
    let mut command = Command::new(exe);
    command
        .arg("agent")
        .arg("holder")
        .arg("--admin")
        .arg(args.admin.path())
        .arg("--worktree")
        .arg(&args.worktree)
        .arg("--rows")
        .arg(args.size.rows.to_string())
        .arg("--cols")
        .arg(args.size.cols.to_string())
        .arg("--")
        .args(&args.argv)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for var in strip_vars {
        command.env_remove(var);
    }
    let mut child = command.spawn().context("could not start the holder")?;
    let Some(stdout) = child.stdout.take() else {
        bail!("the holder has no pipe to report on");
    };
    // The holder is never waited for: dropping `child` neither kills nor reaps it.
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        let read = BufReader::new(stdout).read_line(&mut line).map(|_| line);
        let _ = sender.send(read);
    });
    let line = receiver
        .recv_timeout(READY_TIMEOUT)
        .map_err(|_| anyhow::anyhow!("the holder did not report within {READY_TIMEOUT:?}"))?
        .context("could not read the holder's report")?;
    parse_report(&line)
}

/// The holder's first word on its stdout: `ok <pid>` once the agent runs, `error <why>` when it
/// could not start.
pub(crate) fn ready_line(pid: u32) -> String {
    format!("ok {pid}\n")
}

pub(crate) fn error_line(message: &str) -> String {
    format!("error {}\n", message.replace('\n', " "))
}

fn parse_report(line: &str) -> Result<u32> {
    let line = line.trim_end();
    if line.is_empty() {
        bail!("the holder exited before reporting");
    }
    match line.split_once(' ') {
        Some(("ok", pid)) => pid
            .parse()
            .with_context(|| format!("the holder reported a pid that is not one: `{pid}`")),
        Some(("error", message)) => bail!("{message}"),
        _ => bail!("the holder said something unexpected: `{line}`"),
    }
}

/// Connects to the holder of the worktree whose admin directory is `admin`. Changes this process's
/// directory to `admin` (see the module doc), so callers resolve their own paths first and call
/// this before starting any thread.
pub(crate) fn connect(admin: &AdminDir) -> Result<UnixStream> {
    std::env::set_current_dir(admin.path())
        .with_context(|| format!("could not enter {}", admin.path().display()))?;
    UnixStream::connect(AGENT_SOCKET_FILE)
        .with_context(|| format!("could not connect to {}", admin.agent_socket().display()))
}

/// How long a write to a holder may block before the delivery gives up.
const DELIVERY_TIMEOUT: Duration = Duration::from_secs(5);

/// Types into the agent's terminal through the holder of `admin`, as an attached terminal would:
/// `first`, then after `pause` `second`, each as one input frame. It connects as one more client
/// and sends no size, so the terminals attached keep theirs (only a client's own first size
/// resizes the agent), never reads what the holder sends back, and closes without a stop, leaving
/// the agent running. Returns once both frames were written to the holder. Changes this process's
/// directory, as [`connect`] does.
pub(crate) fn type_input(
    admin: &AdminDir,
    first: &[u8],
    pause: Duration,
    second: &[u8],
) -> Result<()> {
    let mut stream = connect(admin)?;
    stream
        .set_write_timeout(Some(DELIVERY_TIMEOUT))
        .context("could not set up the connection to the holder")?;
    stream
        .write_all(&Frame::Input(first.to_vec()).encode())
        .context("could not send the input to the holder")?;
    thread::sleep(pause);
    stream
        .write_all(&Frame::Input(second.to_vec()).encode())
        .context("could not send the input to the holder")?;
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

/// Asks the holder of `admin` to end its agent, and returns at once. The holder hangs the agent up,
/// then escalates; `agent stop` watches `agent.lock` to know when it is gone.
pub(crate) fn request_stop(admin: &AdminDir) -> Result<()> {
    let mut stream = connect(admin)?;
    stream
        .write_all(&Frame::Stop.encode())
        .context("could not send the stop to the holder")?;
    let _ = stream.shutdown(Shutdown::Both);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_holder_reports_its_pid_or_its_failure() {
        assert_eq!(parse_report(&ready_line(4242)).unwrap(), 4242);
        let err =
            parse_report(&error_line("claude exited at once (exit 1): bad settings")).unwrap_err();
        assert_eq!(
            err.to_string(),
            "claude exited at once (exit 1): bad settings"
        );
        assert_eq!(error_line("two\nlines"), "error two lines\n");
        assert!(
            parse_report("")
                .unwrap_err()
                .to_string()
                .contains("exited before reporting")
        );
        assert!(parse_report("ok x\n").is_err());
        assert!(parse_report("hello\n").is_err());
    }
}
