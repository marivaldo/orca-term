//! Ops: `pr compose`, the message a worktree's new pull request starts from (#132,
//! `domain/pr/create.rs`): the title and body as `gh pr create --fill` would make them, or the
//! repository's template for the body, with the base, the branch, how many commits it has ahead
//! of the base and how many files have changes not committed. Only reads: it runs no gh, pushes
//! nothing and writes nothing. `pr create` finds the same facts through [`prepare`].

use std::path::Path;

use anyhow::{Result, bail};

use crate::adapters::{env, fs, git};
use crate::domain::branch::Branch;
use crate::domain::fleet::{PrimaryCheckout, Worktree};
use crate::domain::pr::create::{self, Composed};
use crate::domain::worktree::Target;
use crate::ops::{find_target, read_fleet};

/// The message of the new pull request of the worktree `target` names or locates in the
/// repository containing the current directory, into the default branch.
pub(crate) fn run(target: &Target) -> Result<Composed> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = find_target(&fleet, &cwd, target)?;
    prepare(&fleet.primary, worktree, None)
}

/// What a new pull request of `worktree` into `base` (the default branch when `None`) starts from.
/// Refuses a worktree whose directory is gone, and one on no branch.
pub(crate) fn prepare(
    primary: &PrimaryCheckout,
    worktree: &Worktree,
    base: Option<&Branch>,
) -> Result<Composed> {
    if worktree.is_gone() {
        bail!(
            "the directory of worktree {} is gone; run orca-term worktree prune",
            worktree.name
        );
    }
    let Some(branch) = worktree.branch.clone() else {
        bail!(
            "worktree {} is on no branch (a detached HEAD); check out a branch to open a pull \
             request",
            worktree.name
        );
    };
    let base = match base {
        Some(base) => base.clone(),
        None => git::default_branch(primary)?,
    };
    let base_point = git::base_point(primary, &base)?;
    let commits = create::parse_log(&git::commits_ahead(&worktree.path, &base_point)?);
    let template = read_template(&worktree.path)?;
    let message = create::compose(&branch, &commits, template.as_deref());
    let uncommitted = create::count_uncommitted(&git::uncommitted_status(&worktree.path)?);
    Ok(Composed {
        message,
        base,
        branch,
        commits: commits.len(),
        uncommitted,
    })
}

/// The text of the worktree's pull request template, from its files on disk: the first file named
/// `pull_request_template` (any case, any extension) in `.github/`, the root, then `docs/`.
fn read_template(worktree: &Path) -> Result<Option<String>> {
    for dir in create::TEMPLATE_DIRS {
        let dir = worktree.join(dir);
        let names = fs::list_file_names(&dir)?;
        if let Some(name) = names.iter().find(|name| create::is_template_name(name)) {
            return fs::read_text_optional(&dir.join(name));
        }
    }
    Ok(None)
}
