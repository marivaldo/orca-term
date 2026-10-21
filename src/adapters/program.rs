//! Adapter: asking a program, found on PATH by the fs adapter, what version it is, and asking
//! opencode which sessions it keeps (#41). Both only read: neither spends anything.
#![expect(
    clippy::disallowed_types,
    reason = "this adapter is the one place that runs a program for what it reports"
)]

use std::path::Path;
use std::process::{Command, Stdio};

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
