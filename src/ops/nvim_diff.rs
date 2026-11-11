//! Ops: the hidden `nvim diff <worktree> <file>` command, and what the application does on enter
//! on a changed file of its right column (#177): it asks every Neovim registered in the
//! repository (`domain/nvim.rs`) to open the diff of the file in the worktree's review. The
//! Neovim client decides, in each Neovim, whether that Neovim works in the worktree; only those
//! open the review (`lua/orca-term/actions.lua`, `diff_from_core`). Writes nothing but a gone
//! Neovim's registry file, and never waits for the review itself.

use std::path::Path;

use anyhow::Result;

use crate::adapters::env;
use crate::domain::fleet::Worktree;
use crate::domain::nvim;
use crate::domain::worktree::Target;
use crate::ops::{find_target, nvim_registry_of, read_fleet, send_to_registered};

/// Asks the Neovims of the repository containing the current directory to open the diff of `file`
/// in the review of the worktree `target` names or locates. Returns that worktree. Refuses an
/// unknown worktree.
pub(crate) fn run(target: &Target, file: &str) -> Result<Worktree> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = find_target(&fleet, &cwd, target)?;
    run_in(&fleet.primary.path, &worktree.path, file)?;
    Ok(worktree.clone())
}

/// Asks the Neovims of the repository whose primary checkout is `primary` to open the diff of
/// `file` in the review of the worktree at `worktree`: what the application does.
pub(crate) fn run_in(primary: &Path, worktree: &Path, file: &str) -> Result<()> {
    let message = nvim::open_diff(
        &worktree.to_string_lossy(),
        &primary.to_string_lossy(),
        file,
    );
    send_to_registered(&nvim_registry_of(primary)?, &message);
    Ok(())
}
