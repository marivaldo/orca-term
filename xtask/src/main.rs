//! The repository's own lints, which no off-the-shelf tool covers. Run with `cargo xtask`.

use std::path::Path;
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};

mod commit;
mod pr_title;
mod vocab;

const USAGE: &str = "usage: cargo xtask commit-lint (--file <path> | --range <revs>) | pr-title-lint <title> | vocab";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match run(&args) {
        Ok(problems) if problems.is_empty() => ExitCode::SUCCESS,
        Ok(problems) => {
            for p in &problems {
                report(p);
            }
            ExitCode::FAILURE
        }
        Err(err) => {
            report(&format!("{err:#}"));
            ExitCode::from(2)
        }
    }
}

fn run(args: &[&str]) -> Result<Vec<String>> {
    match args {
        ["commit-lint", "--file", path] => {
            let raw = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
            Ok(commit::lint(&commit::strip_git_comments(&raw)))
        }
        ["commit-lint", "--range", revs] => {
            let mut problems = Vec::new();
            for sha in git(&["rev-list", "--no-merges", revs])?.lines() {
                let msg = git(&["log", "-1", "--format=%B", sha])?;
                let short = sha.get(..8).unwrap_or(sha);
                problems.extend(
                    commit::lint(&msg)
                        .into_iter()
                        .map(|p| format!("{short}: {p}")),
                );
            }
            Ok(problems)
        }
        ["pr-title-lint", title] => Ok(pr_title::lint(title)),
        ["vocab"] => vocab::lint_repository(Path::new(".")),
        _ => bail!(USAGE),
    }
}

fn git(args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .args(args)
        .output()
        .context("running git")?;
    if !out.status.success() {
        bail!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8(out.stdout)?)
}

fn report(line: &str) {
    #[expect(clippy::print_stderr, reason = "a lint reports its findings on stderr")]
    {
        eprintln!("{line}");
    }
}
