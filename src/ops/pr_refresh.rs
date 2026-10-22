//! Ops: `pr refresh`, which asks the GitHub CLI about each worktree's pull request and its checks
//! (#87) and keeps the answer in the worktree's `pr.json` (`domain/pr.rs`), for `worktree ls` and
//! `worktree detail` to show. This is the only code that runs gh (`cargo xtask gh-guard`).
//!
//! gh runs in every worktree at once, one thread each, each stopped after 10 seconds, so a fleet
//! takes about as long as its slowest answer. Each `pr.json` is replaced whole with no lock, the
//! last write winning; the running Neovims are told only about a worktree whose answer changed.
//! gh missing, not logged in or not pointed at GitHub is an answer too (`unavailable`): the
//! command itself fails only for a worktree it cannot find or write.

use std::path::Path;
use std::thread;

use anyhow::{Result, bail};

use crate::adapters::{env, fs, git, program};
use crate::domain::fleet::Worktree;
use crate::domain::pr::{self, Found, GhRun, PrFacts, Refreshed, RefreshedWorktree};
use crate::domain::worktree::Target;
use crate::ops::{find_target, notify_changed, read_fleet, read_pr};

/// The GitHub CLI, as found on PATH. Only this file may name it.
const GH: &str = "gh";

/// Refreshes the pull request of every worktree `targets` names or locates in the repository
/// containing the current directory, or of every worktree still on disk when it names none.
/// Refuses the primary checkout, an unknown worktree and one whose directory is gone.
pub(crate) fn run(targets: &[Target]) -> Result<Refreshed> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
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
    let gh = fs::executable_in(&env::path_dirs(), GH);
    let answers = ask_all(gh.as_deref(), &chosen);
    let fetched_at = env::now_millis();
    let mut refreshed = Refreshed::default();
    for (worktree, found) in chosen.into_iter().zip(answers) {
        let facts = PrFacts { fetched_at, found };
        keep(worktree, &facts)?;
        refreshed.worktrees.push(RefreshedWorktree {
            name: worktree.name.clone(),
            path: worktree.path.clone(),
            facts,
        });
    }
    Ok(refreshed)
}

/// Asks gh, found at `gh` (`None` when it is not on PATH), about every worktree of `worktrees` at
/// once, one thread each. The answers come back in the same order.
fn ask_all(gh: Option<&Path>, worktrees: &[&Worktree]) -> Vec<Found> {
    thread::scope(|scope| {
        let asking: Vec<_> = worktrees
            .iter()
            .map(|worktree| scope.spawn(move || ask(gh, worktree)))
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

/// What gh says about the pull request of `worktree`'s branch. A worktree on no branch has none.
fn ask(gh: Option<&Path>, worktree: &Worktree) -> Found {
    let Some(branch) = &worktree.branch else {
        return Found::NoPr;
    };
    let run = match gh {
        None => GhRun::Missing,
        Some(gh) => {
            let args = pr::view_args(branch.as_str());
            match program::run_within(gh, &args, &worktree.path, pr::GH_TIMEOUT) {
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
    };
    pr::found(&run)
}

/// Writes `facts` to the `pr.json` of `worktree`, never over a newer schema, and tells the running
/// Neovims when the answer changed.
fn keep(worktree: &Worktree, facts: &PrFacts) -> Result<()> {
    let admin = git::admin_dir(&worktree.path)?;
    let before = read_pr(&admin);
    pr::check_overwrite(&before)?;
    // A worktree made by hand with git has no admin directory of ours yet.
    fs::create_dir_all(admin.path())?;
    fs::write_atomically(&admin.pr_file(), &pr::to_bytes(facts)?)?;
    if pr::changed(&before, &facts.found) {
        notify_changed(&admin);
    }
    Ok(())
}
