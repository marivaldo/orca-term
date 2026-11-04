//! Ops: `pr create`, which pushes a worktree's branch and creates its pull request on GitHub
//! through the GitHub CLI, from the core and outside any agent's sandbox (#132,
//! `domain/pr/create.rs`). The only code that pushes (`cargo xtask gh-guard`).
//!
//! It refuses first, pushing nothing: an empty title, a branch with no commits ahead of the base
//! (`no_commits`), a branch whose pull request is already open (`pr_exists`, asked of gh as
//! `pr refresh` asks), and gh missing or not logged in (`prerequisite`). Then it runs
//! `git push -u origin <branch>` (a failure is `push_failed`, with git's words), then
//! `gh pr create --base --head --title --body-file|--body [--draft]`. Last it asks gh about the
//! branch again, as `pr refresh` does, and keeps the answer in `pr.json` through the same writer,
//! so `worktree ls` shows the pull request at once. Files with changes not committed do not stop
//! it: they are left out, and reported.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::adapters::git::{self, Pushed};
use crate::adapters::{env, fs, program};
use crate::domain::branch::Branch;
use crate::domain::error::TypedError;
use crate::domain::fleet::Worktree;
use crate::domain::pr::create::{self, Body, PrCreated, Readiness};
use crate::domain::pr::{Found, GH_MISSING, PrFacts};
use crate::domain::worktree::Target;
use crate::ops::{find_target, pr_compose, pr_refresh, read_fleet};

/// What `pr create` is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Wanted {
    pub(crate) title: String,
    /// The file holding the body, as given (relative to the current directory, or absolute).
    pub(crate) body_file: Option<PathBuf>,
    /// The branch to merge into; the default branch when `None`.
    pub(crate) base: Option<Branch>,
    pub(crate) readiness: Readiness,
}

/// Pushes the branch of the worktree `target` names or locates in the repository containing the
/// current directory, creates its pull request as `wanted` says, and records it.
pub(crate) fn run(target: &Target, wanted: &Wanted) -> Result<PrCreated> {
    create::check_title(&wanted.title)?;
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = find_target(&fleet, &cwd, target)?;
    let composed = pr_compose::prepare(&fleet.primary, worktree, wanted.base.as_ref())?;
    create::check_has_commits(&worktree.name, &composed)?;
    let Some(gh) = pr_refresh::find_gh() else {
        return Err(TypedError::GhUnavailable {
            name: worktree.name.clone(),
            reason: GH_MISSING.to_owned(),
        }
        .into());
    };
    create::check_no_open_pr(&worktree.name, &pr_refresh::ask(Some(&gh), worktree))?;

    push(worktree, &composed.branch)?;
    let body = body_of(&cwd, wanted.body_file.as_deref())?;
    let args = create::create_args(
        &composed.base,
        &composed.branch,
        &wanted.title,
        &body,
        wanted.readiness,
    );
    let printed = run_gh_create(&gh, &args, worktree, &composed.branch)?;

    let found = match pr_refresh::ask(Some(&gh), worktree) {
        Found::Pr(pr) => Found::Pr(pr),
        // GitHub may not show a pull request created a moment ago: keep what gh printed.
        Found::NoPr | Found::Unavailable(_) => {
            match create::created_pr(&printed, &wanted.title, wanted.readiness) {
                Some(pr) => Found::Pr(pr),
                None => bail!(
                    "gh created the pull request of worktree {} but printed no address for it; \
                     run orca-term pr refresh {} to read it",
                    worktree.name,
                    worktree.name
                ),
            }
        }
    };
    let facts = PrFacts {
        fetched_at: env::now_millis(),
        found,
    };
    pr_refresh::keep(worktree, &facts)?;
    Ok(PrCreated {
        name: worktree.name.clone(),
        path: worktree.path.clone(),
        facts,
        uncommitted: composed.uncommitted,
    })
}

/// Pushes `branch` of `worktree` to `origin`, setting its upstream, never forced. A push git
/// refuses is the typed `push_failed`, with git's words.
fn push(worktree: &Worktree, branch: &Branch) -> Result<()> {
    match git::push_branch(&worktree.path, branch)? {
        Pushed::Done => Ok(()),
        Pushed::Failed(reason) => Err(TypedError::PushFailed {
            name: worktree.name.clone(),
            branch: branch.to_string(),
            reason,
        }
        .into()),
    }
}

/// The body for gh: the file given (made absolute against `cwd`, since gh runs in the worktree)
/// when it exists and holds something, else no body.
fn body_of(cwd: &Path, body_file: Option<&Path>) -> Result<Body> {
    let Some(file) = body_file else {
        return Ok(Body::Empty);
    };
    let file = create::absolute(cwd, file);
    match fs::read_optional(&file)? {
        Some(bytes) if !bytes.is_empty() => Ok(Body::File(file)),
        Some(_) | None => Ok(Body::Empty),
    }
}

/// Runs `gh` with `args` in `worktree` and hands back what it printed on stdout. A failure says
/// gh's first line, and that the branch was pushed already.
fn run_gh_create(
    gh: &Path,
    args: &[String],
    worktree: &Worktree,
    branch: &Branch,
) -> Result<Vec<u8>> {
    let pushed = format!("branch {branch} was pushed to origin, but no pull request was created");
    match program::run_within(gh, args, &worktree.path, create::CREATE_TIMEOUT) {
        Ok(program::Finished::Exited {
            success: true,
            stdout,
            ..
        }) => Ok(stdout),
        Ok(program::Finished::Exited { stderr, .. }) => bail!(
            "gh pr create failed for worktree {}: {}; {pushed}",
            worktree.name,
            create::first_line(&stderr, "gh pr create failed")
        ),
        Ok(program::Finished::TimedOut) => bail!(
            "gh pr create did not answer within {} seconds for worktree {}, and was stopped; \
             branch {branch} was pushed to origin; run orca-term pr refresh {} to see whether the \
             pull request was created",
            create::CREATE_TIMEOUT.as_secs(),
            worktree.name,
            worktree.name
        ),
        Err(err) => bail!("could not run gh: {err}; {pushed}"),
    }
}
