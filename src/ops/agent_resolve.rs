//! Ops: `agent resolve`, which types into a worktree's running agent one prompt asking it to
//! resolve the conflicts its rebase stopped on (#141, `domain/rebase.rs`). The prompt names the
//! files in conflict, says to resolve them, `git add` them and continue the rebase, and forbids
//! aborting, resetting, stashing and pushing, as Orca's conflict prompt does.
//!
//! It is typed as `note send` types notes: through the worktree's holder, a bracketed paste, then
//! Enter on its own a moment later. It refuses a worktree with no rebase in progress (`no_rebase`)
//! and one whose agent is not running (`no_holder`); nothing is typed then.

use anyhow::{Result, bail};

use crate::adapters::{env, git, holder};
use crate::domain::error::{RebaseAsk, TypedError};
use crate::domain::note::prompt;
use crate::domain::operation::Operation;
use crate::domain::rebase::{self, ResolveSent};
use crate::domain::worktree::Target;
use crate::ops::{find_target_admin, read_fleet};

/// Types the prompt to resolve the rebase's conflicts into the agent of the worktree `target`
/// names or locates in the repository containing the current directory.
pub(crate) fn run(target: &Target) -> Result<ResolveSent> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let (worktree, admin) = find_target_admin(&fleet, &cwd, target)?;
    if worktree.is_gone() {
        bail!(
            "the directory of worktree {} is gone; run orca-term worktree prune",
            worktree.name
        );
    }
    if worktree.operation != Some(Operation::Rebase) {
        return Err(TypedError::NoRebase {
            name: worktree.name.clone(),
            asked: RebaseAsk::Resolve,
        }
        .into());
    }
    if !worktree.holder.is_alive() {
        return Err(TypedError::ResolveNoHolder {
            name: worktree.name.clone(),
            path: worktree.path.clone(),
        }
        .into());
    }
    let files = rebase::paths(&git::conflicted_files(&worktree.path)?);
    let text = rebase::resolve_prompt(&files);

    // Changes this process's directory; nothing after it uses a relative path.
    holder::type_input(
        &admin,
        &prompt::paste(&text),
        prompt::PASTE_SUBMIT_PAUSE,
        prompt::SUBMIT,
    )?;
    Ok(ResolveSent {
        name: worktree.name.clone(),
        path: worktree.path.clone(),
        prompt: text,
        files,
    })
}
