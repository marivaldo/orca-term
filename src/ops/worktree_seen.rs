//! Ops: `worktree seen` and `worktree unseen`, which the person runs to clear or set a worktree's
//! unseen mark (`domain/seen.rs`). Both write only `seen.json`, replacing it whole, and take no
//! lock: not the worktree's (a mark while a setup runs is never busy) nor `hook.lock`. Doing either
//! twice changes nothing more. `worktree review` marks through [`mark`] too.

use anyhow::Result;

use crate::adapters::{env, fs, git};
use crate::domain::fleet::Worktree;
use crate::domain::seen::{self, Mark, Marked};
use crate::domain::worktree::{self, Target};
use crate::ops::{find_target, notify_changed, read_fleet, read_status};

/// Marks the worktree `target` names or locates in the repository containing the current
/// directory as seen or unseen, as `mark` says, then tells the running Neovims. Works on any
/// worktree still on disk, ready or not, with an agent or none; refuses the primary checkout.
pub(crate) fn run(target: &Target, mark: Mark) -> Result<Marked> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = find_target(&fleet, &cwd, target)?;
    worktree::check_can_mark(worktree)?;
    let unseen = self::mark(worktree, mark)?;
    Ok(Marked {
        name: worktree.name.clone(),
        path: worktree.path.clone(),
        unseen,
    })
}

/// Writes `mark` to the `seen.json` of `worktree`, which must still be on disk, then tells the
/// running Neovims. Returns whether the worktree is unseen now.
pub(crate) fn mark(worktree: &Worktree, mark: Mark) -> Result<bool> {
    let admin = git::admin_dir(&worktree.path)?;
    // The latest turn that ended, as `worktree ls` sees it: `seen` takes everything up to it.
    let ended = seen::latest_ended(read_status(&admin).as_ref(), worktree.holder);
    let path = admin.seen_file();
    let current = seen::read(fs::read_optional(&path)?.as_deref());
    seen::check_overwrite(&current)?;
    let record = mark.apply(current.usable(), ended);
    // A worktree made by hand with git has no admin directory of ours yet.
    fs::create_dir_all(admin.path())?;
    fs::write_atomically(&path, &seen::to_bytes(&record)?)?;
    notify_changed(&admin);
    Ok(seen::is_unseen(record, ended))
}
