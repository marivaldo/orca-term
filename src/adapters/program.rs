//! Adapter: asking a program, found on PATH by the fs adapter, what version it is.
#![expect(
    clippy::disallowed_types,
    reason = "this adapter is the one place that runs a program for its version"
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
