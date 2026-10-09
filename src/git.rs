//! Running git. Every git invocation of the core goes through here.

use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result, bail};

/// Runs `git -C <dir> <args>` and returns its stdout as raw bytes.
pub fn output(dir: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("could not run git; is it installed and on PATH?")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stderr = stderr.trim();
        if stderr.contains("not a git repository") {
            bail!("not inside a git repository: {}", dir.display());
        }
        bail!("git {} failed: {stderr}", args.join(" "));
    }
    Ok(out.stdout)
}
