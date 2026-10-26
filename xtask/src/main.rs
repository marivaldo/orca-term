//! The repository's own lints, which no off-the-shelf tool covers, and the live checks against the
//! real agents (`live.rs`). Run with `cargo xtask`.
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "the layer rule binds the core; the xtask is a tool that reads files and runs git"
)]

use std::path::Path;
use std::process::{Command, ExitCode};

use anyhow::{Context, Result, bail};

mod client_boundary;
mod commit;
mod gh_guard;
mod launch_guard;
mod layers;
mod live;
mod pr_title;
mod release_version;
mod vocab;

const USAGE: &str = "usage: cargo xtask commit-lint (--file <path> | --range <revs>) | pr-title-lint <title> | release-version <tag> | vocab | layers | client-boundary | launch-guard | gh-guard | check-live [--orca-term <path>] [<scenario>...] | record-live [--orca-term <path>] <scenario>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    // The live checks print their own summary and choose their own exit code.
    match args.as_slice() {
        ["check-live", rest @ ..] => return live::check_live(rest),
        ["record-live", rest @ ..] => return live::record_live(rest),
        _ => {}
    }
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
        ["release-version", tag] => {
            let manifest = std::fs::read_to_string("Cargo.toml").context("reading Cargo.toml")?;
            let version = release_version::package_version(&manifest)
                .context("Cargo.toml has no `version` in its [package] table")?;
            Ok(release_version::lint(tag, version))
        }
        ["vocab"] => vocab::lint_repository(Path::new(".")),
        ["layers"] => layers::lint_repository(Path::new(".")),
        ["client-boundary"] => client_boundary::lint_repository(Path::new(".")),
        ["launch-guard"] => launch_guard::lint_repository(Path::new(".")),
        ["gh-guard"] => gh_guard::lint_repository(Path::new(".")),
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
