//! Adapter: asking a program, found on PATH by the fs adapter, what version it is, asking
//! opencode which sessions it keeps (#41), and running a program with a time limit, which
//! `pr refresh` runs gh with (#87). All only read: none spends anything.
#![expect(
    clippy::disallowed_types,
    reason = "this adapter is the one place that runs a program for what it reports"
)]

use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

/// What `<program> --version` prints on stdout.
pub(crate) fn version(program: &Path) -> Result<String> {
    let out = Command::new(program)
        .arg("--version")
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("could not run {} --version", program.display()))?;
    if !out.status.success() {
        bail!(
            "{} --version failed: {}",
            program.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// What `program` (opencode, by its absolute path) prints on stdout for `args`
/// (`domain/launch/opencode.rs`: its list of sessions), run in `dir`, the worktree, whose project
/// the list is of. Its stdin is empty. Fails when it cannot run or exits with an error.
pub(crate) fn listed_by(program: &Path, args: &[&str], dir: &Path) -> Result<Vec<u8>> {
    let out = Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("could not run {} {}", program.display(), args.join(" ")))?;
    if !out.status.success() {
        bail!(
            "{} {} failed: {}",
            program.display(),
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(out.stdout)
}

/// What a program run with a time limit did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Finished {
    /// It exited: whether it succeeded, and what it printed on stdout and stderr.
    Exited {
        success: bool,
        stdout: Vec<u8>,
        stderr: Vec<u8>,
    },
    /// It ran past the time limit and was killed.
    TimedOut,
}

/// Runs `program` (by its absolute path) with `args` in `dir`, its stdin empty, and waits for it
/// at most `limit`: past it the program is killed. Fails only when it cannot be started.
pub(crate) fn run_within(
    program: &Path,
    args: &[String],
    dir: &Path,
    limit: Duration,
) -> std::io::Result<Finished> {
    let mut child = Command::new(program)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    // Both pipes are read while it runs, so a program that prints a lot never blocks on a full
    // pipe.
    let stdout = read_all_in_background(child.stdout.take());
    let stderr = read_all_in_background(child.stderr.take());
    let deadline = Instant::now() + limit;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Finished::Exited {
                success: status.success(),
                stdout: stdout.join().unwrap_or_default(),
                stderr: stderr.join().unwrap_or_default(),
            });
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            // The readers are left to end on their own: a process the program started may still
            // hold its pipes.
            return Ok(Finished::TimedOut);
        }
        thread::sleep(Duration::from_millis(10));
    }
}

/// Reads all of `pipe` on a thread of its own, handing back what it read.
fn read_all_in_background<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        bytes
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str, limit: Duration) -> Finished {
        let args = vec!["-c".to_owned(), script.to_owned()];
        run_within(Path::new("/bin/sh"), &args, Path::new("/"), limit).unwrap()
    }

    #[test]
    fn a_program_that_exits_in_time_gives_its_status_and_both_outputs() {
        assert_eq!(
            sh("echo out; echo err >&2; exit 3", Duration::from_secs(10)),
            Finished::Exited {
                success: false,
                stdout: b"out\n".to_vec(),
                stderr: b"err\n".to_vec(),
            }
        );
    }

    #[test]
    fn a_program_past_the_limit_is_killed() {
        let started = Instant::now();
        assert_eq!(
            sh("exec sleep 30", Duration::from_millis(200)),
            Finished::TimedOut
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_program_that_cannot_start_is_an_error() {
        let missing = Path::new("/nonexistent/orca-term-test-program");
        assert!(run_within(missing, &[], Path::new("/"), Duration::from_secs(1)).is_err());
    }
}
