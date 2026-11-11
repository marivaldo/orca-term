//! Ops: `pr refresh`, which asks the GitHub CLI about each worktree's pull request and its checks
//! (#87) and keeps the answer in the worktree's `pr.json` (`domain/pr.rs`), for `worktree ls` and
//! `worktree detail` to show. gh runs only from here and from `pr create` and `pr merge`, which
//! find it, ask about a branch and keep the answer through this module's functions (`cargo xtask
//! gh-guard`). `worktree push` reads a worktree's pull request again after a push through
//! [`refresh`], here (#146).
//!
//! gh runs in every worktree at once, one thread each, each stopped after 10 seconds, so a fleet
//! takes about as long as its slowest answer. Each `pr.json` is replaced whole, the last write
//! winning, holding only the shared lock that holds off a removal of the worktree meanwhile
//! (`ops::guard_against_removal`): a worktree removed while gh answered is left out, and nothing is
//! written for it. The running Neovims are told only about a worktree whose answer changed.
//! gh missing, not logged in or not pointed at GitHub is an answer too (`unavailable`): the
//! command itself fails only for a worktree it cannot find or write.

use std::path::{Path, PathBuf};
use std::thread;

use anyhow::{Result, bail};

use crate::adapters::{env, fs, git, program};
use crate::domain::branch::Branch;
use crate::domain::fleet::Worktree;
use crate::domain::pr::{self, Found, GhRun, PrFacts, Refreshed, RefreshedWorktree};
use crate::domain::state::AdminDir;
use crate::domain::worktree::Target;
use crate::ops::{find_target, guard_against_removal, notify_changed, read_fleet, read_pr};

/// The GitHub CLI, as found on PATH. Only this file may name it.
const GH: &str = "gh";

/// Refreshes the pull request of every worktree `targets` names or locates in the repository
/// containing the current directory, or of every worktree still on disk when it names none, the
/// primary checkout included. Refuses an unknown worktree and one whose directory is gone. A
/// worktree on the default branch has no pull request, and gh is not asked about it (ADR 0013).
pub(crate) fn run(targets: &[Target]) -> Result<Refreshed> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let default = git::default_branch(&fleet.primary).ok();
    let chosen: Vec<&Worktree> = if targets.is_empty() {
        fleet.worktrees.iter().filter(|w| !w.is_gone()).collect()
    } else {
        let mut named = Vec::new();
        for target in targets {
            let worktree = find_target(&fleet, &cwd, target)?;
            if worktree.is_gone() {
                bail!(
                    "the directory of worktree {} is gone; run orca-term worktree prune",
                    worktree.name
                );
            }
            named.push(worktree);
        }
        named
    };
    // Found while every chosen worktree is on disk: one may be removed while gh answers.
    let admins: Vec<AdminDir> = chosen
        .iter()
        .map(|worktree| git::admin_dir(&worktree.path))
        .collect::<Result<_>>()?;
    let gh = find_gh();
    let answers = ask_all(gh.as_deref(), &chosen, default.as_ref());
    let fetched_at = env::now_millis();
    let mut refreshed = Refreshed::default();
    for ((worktree, admin), found) in chosen.into_iter().zip(&admins).zip(answers) {
        let facts = PrFacts { fetched_at, found };
        if !keep_in(admin, &facts)? {
            continue;
        }
        refreshed.worktrees.push(RefreshedWorktree {
            name: worktree.name.clone(),
            path: worktree.path.clone(),
            facts,
        });
    }
    Ok(refreshed)
}

/// Asks gh about the pull request of `worktree` again and keeps the answer, as `pr refresh` of
/// that worktree alone does, given the repository's `default` branch when known. `worktree push`
/// calls it after a push, so the row's checks show the new run (#146). gh missing or not logged
/// in is an answer too: this fails only when the answer cannot be written.
pub(crate) fn refresh(worktree: &Worktree, default: Option<&Branch>) -> Result<PrFacts> {
    let admin = git::admin_dir(&worktree.path)?;
    let found = ask(find_gh().as_deref(), worktree, default);
    let facts = PrFacts {
        fetched_at: env::now_millis(),
        found,
    };
    keep_in(&admin, &facts)?;
    Ok(facts)
}

/// gh as found on PATH, or `None` when it is not there. `pr create` and `pr merge` find it here
/// too.
pub(crate) fn find_gh() -> Option<PathBuf> {
    fs::executable_in(&env::path_dirs(), GH)
}

/// Asks gh, found at `gh` (`None` when it is not on PATH), about every worktree of `worktrees` at
/// once, one thread each, given the repository's `default` branch when known. The answers come
/// back in the same order.
fn ask_all(gh: Option<&Path>, worktrees: &[&Worktree], default: Option<&Branch>) -> Vec<Found> {
    thread::scope(|scope| {
        let asking: Vec<_> = worktrees
            .iter()
            .map(|worktree| scope.spawn(move || ask(gh, worktree, default)))
            .collect();
        asking
            .into_iter()
            .map(|handle| {
                handle.join().unwrap_or_else(|_| {
                    Found::Unavailable("asking gh failed unexpectedly".to_owned())
                })
            })
            .collect()
    })
}

/// What gh says about the pull request of `worktree`'s branch, given the repository's `default`
/// branch when known. A worktree on no branch, or on the default branch, has none, and gh is not
/// asked (`pr::branch_to_ask`).
pub(crate) fn ask(gh: Option<&Path>, worktree: &Worktree, default: Option<&Branch>) -> Found {
    let Some(branch) = pr::branch_to_ask(worktree.branch.as_ref(), default) else {
        return Found::NoPr;
    };
    pr::found(&view(gh, &pr::view_args(branch.as_str()), worktree))
}

/// Runs gh, found at `gh` (`None` when it is not on PATH), with `args` (a `gh pr view`) in
/// `worktree`, stopped after [`pr::GH_TIMEOUT`]. `pr merge` asks through it too, for more fields.
pub(crate) fn view(gh: Option<&Path>, args: &[String], worktree: &Worktree) -> GhRun {
    let Some(gh) = gh else {
        return GhRun::Missing;
    };
    match program::run_within(gh, args, &worktree.path, pr::GH_TIMEOUT) {
        Ok(program::Finished::Exited {
            success,
            stdout,
            stderr,
        }) => GhRun::Exited {
            success,
            stdout,
            stderr,
        },
        Ok(program::Finished::TimedOut) => GhRun::TimedOut,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => GhRun::Missing,
        Err(err) => GhRun::NotStarted(err.to_string()),
    }
}

/// Writes `facts` to the `pr.json` of `worktree`, as [`keep_in`] does.
pub(crate) fn keep(worktree: &Worktree, facts: &PrFacts) -> Result<()> {
    keep_in(&git::admin_dir(&worktree.path)?, facts)?;
    Ok(())
}

/// Writes `facts` to the `pr.json` in `admin`, never over a newer schema, and tells the running
/// Neovims when the answer changed. Returns `false`, writing nothing, when the worktree was
/// removed meanwhile (`guard_against_removal`).
fn keep_in(admin: &AdminDir, facts: &PrFacts) -> Result<bool> {
    let Some(still_there) = guard_against_removal(admin)? else {
        return Ok(false);
    };
    let before = read_pr(admin);
    pr::check_overwrite(&before)?;
    // A worktree made by hand with git has no admin directory of ours yet.
    fs::create_dir_all(admin.path())?;
    fs::write_atomically(&admin.pr_file(), &pr::to_bytes(facts)?)?;
    // Written: a removal may go ahead while the Neovims are told.
    drop(still_there);
    if pr::changed(&before, &facts.found) {
        notify_changed(admin);
    }
    Ok(true)
}
