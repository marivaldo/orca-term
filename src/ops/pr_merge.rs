//! Ops: `pr merge`, which merges a worktree's open pull request on GitHub through the GitHub CLI,
//! from the core (#136, `domain/pr/merge.rs`).
//!
//! It first asks gh about the branch's pull request (`gh pr view <branch>`, with the fields a
//! merge needs) and refuses, merging nothing: gh missing or not logged in (`prerequisite`), no
//! open pull request (`no_open_pr`), a draft (`pr_draft`), a conflict with the base
//! (`pr_conflicting`) and a review that blocks it (`not_approved`). Then it runs
//! `gh pr merge <number> --squash|--merge|--rebase`, never `--delete-branch`. Last it asks gh
//! again, as `pr refresh` does, and keeps the merged pull request in `pr.json` through the same
//! writer, so the sidebar shows it merged at once. The worktree and its local branch are left as
//! they are.

use std::path::Path;

use anyhow::{Result, bail};

use crate::adapters::{env, program};
use crate::domain::error::{NotOpen, TypedError};
use crate::domain::fleet::Worktree;
use crate::domain::pr::create::first_line;
use crate::domain::pr::merge::{self, Method, PrMerged};
use crate::domain::pr::{Found, GH_MISSING, PrFacts, PullRequest};
use crate::domain::worktree::Target;
use crate::ops::{find_target, pr_refresh, read_fleet};

/// Merges the open pull request of the worktree `target` names or locates in the repository
/// containing the current directory, with `method`, and records it merged.
pub(crate) fn run(target: &Target, method: Method) -> Result<PrMerged> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = find_target(&fleet, &cwd, target)?;
    if worktree.is_gone() {
        bail!(
            "the directory of worktree {} is gone; run orca-term worktree prune",
            worktree.name
        );
    }
    let Some(branch) = &worktree.branch else {
        return Err(TypedError::NoOpenPr {
            name: worktree.name.clone(),
            found: NotOpen::None,
        }
        .into());
    };
    let Some(gh) = pr_refresh::find_gh() else {
        return Err(TypedError::MergeGhUnavailable {
            name: worktree.name.clone(),
            reason: GH_MISSING.to_owned(),
        }
        .into());
    };
    let asked = pr_refresh::view(Some(&gh), &merge::view_args(branch.as_str()), worktree);
    let before = merge::check_mergeable(&worktree.name, &asked)?;

    run_gh_merge(&gh, &before, method, worktree)?;

    let after = pr_refresh::ask(Some(&gh), worktree);
    let facts = PrFacts {
        fetched_at: env::now_millis(),
        found: Found::Pr(merge::merged_pr(&before, after)),
    };
    pr_refresh::keep(worktree, &facts)?;
    Ok(PrMerged {
        name: worktree.name.clone(),
        path: worktree.path.clone(),
        facts,
        method,
    })
}

/// Runs `gh pr merge` for `pr` with `method` in `worktree`. A failure says gh's first line: what
/// GitHub refused beyond the checks made before, such as a merge queue or a required check.
fn run_gh_merge(gh: &Path, pr: &PullRequest, method: Method, worktree: &Worktree) -> Result<()> {
    let args = merge::merge_args(pr.number, method);
    match program::run_within(gh, &args, &worktree.path, merge::MERGE_TIMEOUT) {
        Ok(program::Finished::Exited { success: true, .. }) => Ok(()),
        Ok(program::Finished::Exited { stderr, .. }) => bail!(
            "gh pr merge failed for PR #{} of worktree {}: {}",
            pr.number,
            worktree.name,
            first_line(&stderr, "gh pr merge failed")
        ),
        Ok(program::Finished::TimedOut) => bail!(
            "gh pr merge did not answer within {} seconds for PR #{} of worktree {}, and was \
             stopped; run orca-term pr refresh {} to see whether it was merged",
            merge::MERGE_TIMEOUT.as_secs(),
            pr.number,
            worktree.name,
            worktree.name
        ),
        Err(err) => bail!("could not run gh: {err}; nothing was merged"),
    }
}
