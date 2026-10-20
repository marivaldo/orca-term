//! Ops: `worktree prune`, which cleans the worktrees whose directory is gone, and the chains of
//! snapshots (`domain/snapshot.rs`) of worktrees git no longer has. Only ever invoked by the
//! person: no other command prunes, and nothing else deletes a chain but `worktree rm`.

use anyhow::Result;

use crate::adapters::{env, fs, git};
use crate::domain::fleet::{self, PrimaryCheckout, Pruned};
use crate::domain::snapshot;
use crate::ops::read_fleet;

/// Runs `git worktree prune` from the primary checkout, then deletes the orphan chains, and
/// returns the paths of the worktrees it cleaned and the refs it deleted.
pub(crate) fn run() -> Result<Pruned> {
    let cwd = env::current_dir()?;
    let before = read_fleet(&cwd)?;
    git::worktree_prune(&before.primary)?;
    let after = read_fleet(&before.primary.path)?;
    Ok(Pruned {
        worktrees: fleet::pruned(&before, &after),
        chains: delete_orphan_chains(&before.primary)?,
    })
}

/// Deletes every chain `refs/orca-term/worktrees/<id>` whose git admin directory
/// (`.git/worktrees/<id>`) is gone: a worktree removed by hand left it. Returns the refs deleted.
fn delete_orphan_chains(primary: &PrimaryCheckout) -> Result<Vec<String>> {
    let common = git::common_dir(primary)?;
    let mut deleted = Vec::new();
    for name in git::refs_under(&primary.path, snapshot::CHAIN_REFS)? {
        let Some(id) = snapshot::chain_id(&name) else {
            continue;
        };
        if !fs::is_dir(&snapshot::git_admin_dir(&common, id)) {
            git::delete_ref(&primary.path, &name)?;
            deleted.push(name);
        }
    }
    Ok(deleted)
}
