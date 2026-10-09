//! Running git. Every git invocation of the core goes through here.

use std::ffi::OsStr;
use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result, bail};

/// Runs `git -C <dir> <args>` and returns its stdout as raw bytes. Fails when git does.
pub fn output<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> Result<Vec<u8>> {
    let out = invoke(dir, args)?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        let stderr = stderr.trim();
        if stderr.contains("not a git repository") {
            bail!("not inside a git repository: {}", dir.display());
        }
        bail!("git {} failed: {stderr}", display_args(args));
    }
    Ok(out.stdout)
}

/// Runs `git -C <dir> <args>` and returns its stdout, or `None` when git exits non-zero: for
/// questions git answers with its exit status, such as whether a ref exists.
pub fn probe<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> Result<Option<Vec<u8>>> {
    let out = invoke(dir, args)?;
    Ok(out.status.success().then_some(out.stdout))
}

/// Like [`output`], trimmed and decoded as UTF-8 text.
pub fn text<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> Result<String> {
    let raw = output(dir, args)?;
    let text = String::from_utf8(raw)
        .with_context(|| format!("git {} printed non-UTF-8 output", display_args(args)))?;
    Ok(text.trim_end_matches(['\n', '\r']).to_owned())
}

fn invoke<S: AsRef<OsStr>>(dir: &Path, args: &[S]) -> Result<Output> {
    Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .context("could not run git; is it installed and on PATH?")
}

fn display_args<S: AsRef<OsStr>>(args: &[S]) -> String {
    args.iter()
        .map(|a| a.as_ref().to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}
