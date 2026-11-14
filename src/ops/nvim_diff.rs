//! Ops: the hidden `nvim diff <worktree> <file>` command, and what the application does on enter
//! on a changed file of its right column (#177, #194): it asks every Neovim registered in the
//! repository (`domain/nvim.rs`) to open the file beside its version at the merge base, in
//! Neovim's built-in diff mode. The core finds the merge base and reads the file's content there
//! through git, and sends them with the Lua that opens the diff (`domain/nvim/open_diff.lua`), so
//! a Neovim needs no plugin and runs no git. That code decides, in each Neovim, whether that
//! Neovim works in the worktree; only those open the diff. Writes nothing but a gone Neovim's
//! registry file, and never waits for the diff itself.

use std::path::Path;

use anyhow::Result;

use crate::adapters::{env, git};
use crate::domain::fleet::{PrimaryCheckout, Worktree};
use crate::domain::nvim::{self, BaseVersion};
use crate::domain::worktree::Target;
use crate::ops::{find_target, nvim_registry_of, read_fleet, send_to_registered};

/// Asks the Neovims of the repository containing the current directory to open the diff of `file`
/// in the worktree `target` names or locates. Returns that worktree. Refuses an unknown worktree.
pub(crate) fn run(target: &Target, file: &str) -> Result<Worktree> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = find_target(&fleet, &cwd, target)?;
    ask(&fleet.primary, &worktree.path, file)?;
    Ok(worktree.clone())
}

/// Asks the Neovims of the repository whose primary checkout is at `primary` to open the diff of
/// `file` in the worktree at `worktree`: what the application does.
pub(crate) fn run_in(primary: &Path, worktree: &Path, file: &str) -> Result<()> {
    ask(&git::primary_checkout(primary)?, worktree, file)
}

/// Reads the base version of `file` and sends the request to every registered Neovim.
fn ask(primary: &PrimaryCheckout, worktree: &Path, file: &str) -> Result<()> {
    let base = read_base(primary, worktree, file)?;
    let message = nvim::open_diff(&worktree.to_string_lossy(), file, &base);
    send_to_registered(&nvim_registry_of(&primary.path)?, &message);
    Ok(())
}

/// The version of `file` the diff compares with: its content at the merge base of the worktree
/// and the start point, the same base the right column counts the changes against
/// (`ops/worktree_changes.rs`). A worktree with no merge base, or a file that is not in it (a new
/// file), has an empty one. Bytes that are not UTF-8 are shown as `U+FFFD`.
fn read_base(primary: &PrimaryCheckout, worktree: &Path, file: &str) -> Result<BaseVersion> {
    let start = git::start_point(primary)?;
    let Some(commit) = git::merge_base(worktree, &start)? else {
        return Ok(BaseVersion::default());
    };
    let content = git::file_at(worktree, commit.as_str(), file)?.unwrap_or_default();
    Ok(BaseVersion {
        commit: commit.short().to_owned(),
        text: String::from_utf8_lossy(&content).into_owned(),
    })
}
