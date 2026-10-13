//! Adapter: the setup script's process. It starts the script with Bash and copies everything the
//! script prints, stdout and stderr merged in the order they were written, to the setup log and to
//! the live echo.
#![expect(
    clippy::disallowed_types,
    reason = "this adapter is the one place that starts the setup script"
)]

use std::ffi::OsString;
use std::io::{self, ErrorKind, PipeReader, Read as _, Write};
use std::os::unix::process::ExitStatusExt as _;
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};

use anyhow::{Context, Result, bail};

use crate::domain::setup::{Exit, SetupOutcome};

/// Runs `script` as `bash -e -c <script>` in `dir`, with `vars` added to the inherited environment
/// and stdin closed, and returns how it ended. Its output goes to `log` and to `echo` as it comes.
/// There is no timeout: this waits for as long as the script runs.
pub(crate) fn run_setup(
    script: &str,
    dir: &Path,
    vars: &[(&'static str, OsString)],
    log: &mut dyn Write,
    echo: &mut dyn Write,
) -> Result<SetupOutcome> {
    let (mut output, mut child) = spawn(script, dir, vars)?;
    copy_output(&mut output, log, echo)?;
    let status = child
        .wait()
        .context("could not wait for the setup script")?;
    outcome(status)
}

/// Starts the script with both its stdout and its stderr on the write end of one pipe, so its
/// output keeps its order, and returns the read end with the process.
fn spawn(
    script: &str,
    dir: &Path,
    vars: &[(&'static str, OsString)],
) -> Result<(PipeReader, Child)> {
    let (reader, writer) = io::pipe().context("could not create a pipe for the setup script")?;
    let stderr = writer
        .try_clone()
        .context("could not create a pipe for the setup script")?;
    // The `Command` holds the pipe's write ends until it is dropped, at the end of this
    // statement. After that only the script holds them, so reading ends when the script exits.
    let child = Command::new("bash")
        .arg("-e")
        .arg("-c")
        .arg(script)
        .current_dir(dir)
        .envs(vars.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(writer)
        .stderr(stderr)
        .spawn()
        .context("could not start bash for the setup script; is it installed and on PATH?")?;
    Ok((reader, child))
}

/// Copies `output` to `log` and `echo` until the script closes it. A failing echo is ignored, so a
/// terminal that went away never stops the setup; a failing log is an error.
fn copy_output(output: &mut PipeReader, log: &mut dyn Write, echo: &mut dyn Write) -> Result<()> {
    let mut buf = vec![0; 8 * 1024];
    loop {
        let n = match output.read(&mut buf) {
            Ok(0) => return Ok(()),
            Ok(n) => n,
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => return Err(err).context("could not read the setup script's output"),
        };
        log.write_all(&buf[..n])
            .context("could not write the setup log")?;
        let _ = echo.write_all(&buf[..n]);
        let _ = echo.flush();
    }
}

/// What the script's exit status means: success, a non-zero exit code or a signal.
fn outcome(status: ExitStatus) -> Result<SetupOutcome> {
    if status.success() {
        return Ok(SetupOutcome::Succeeded);
    }
    match (status.code(), status.signal()) {
        (Some(code), _) => Ok(SetupOutcome::Failed(Exit::Code(code))),
        (None, Some(signal)) => Ok(SetupOutcome::Failed(Exit::Signal(signal))),
        (None, None) => bail!("the setup script ended in a way that is not understood ({status})"),
    }
}

#[cfg(test)]
mod tests {
    use assert_fs::TempDir;

    use super::*;

    fn run(script: &str, dir: &Path) -> (SetupOutcome, String, String) {
        let vars = [("ORCA_TERM_TEST", OsString::from("value"))];
        let (mut log, mut echo) = (Vec::new(), Vec::new());
        let outcome = run_setup(script, dir, &vars, &mut log, &mut echo).unwrap();
        (
            outcome,
            String::from_utf8(log).unwrap(),
            String::from_utf8(echo).unwrap(),
        )
    }

    #[test]
    fn stdout_and_stderr_reach_the_log_and_the_echo_in_order() {
        let dir = TempDir::new().unwrap();
        let (outcome, log, echo) = run(
            "echo one; echo two >&2; echo \"$ORCA_TERM_TEST in $(basename \"$PWD\")\"",
            dir.path(),
        );
        assert_eq!(outcome, SetupOutcome::Succeeded);
        let name = dir.path().file_name().unwrap().to_str().unwrap();
        assert_eq!(log, format!("one\ntwo\nvalue in {name}\n"));
        assert_eq!(echo, log);
    }

    #[test]
    fn the_first_failing_command_stops_the_script() {
        let dir = TempDir::new().unwrap();
        let (outcome, log, _) = run("echo before; false; echo after", dir.path());
        assert_eq!(outcome, SetupOutcome::Failed(Exit::Code(1)));
        assert_eq!(log, "before\n");
    }

    #[test]
    fn a_script_killed_by_a_signal_says_which() {
        let dir = TempDir::new().unwrap();
        let (outcome, _, _) = run("kill -TERM $$", dir.path());
        assert_eq!(outcome, SetupOutcome::Failed(Exit::Signal(15)));
    }
}
