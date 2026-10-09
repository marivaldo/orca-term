//! The command-line surface.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::config::Env;
use crate::fleet::Fleet;
use crate::{output, worktree};

#[derive(Debug, Parser)]
#[command(
    name = "orca-term",
    version,
    about = "A fleet of parallel CLI coding agents"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Work with the worktrees of the current repository.
    #[command(visible_alias = "wt")]
    Worktree {
        #[command(subcommand)]
        command: WorktreeCommand,
    },
}

#[derive(Debug, Subcommand)]
enum WorktreeCommand {
    /// List the fleet: every worktree of the current repository.
    Ls {
        /// Print JSON carrying the contract integer instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Create a worktree on a new branch from the default branch.
    New {
        /// The worktree's name, which is also its branch and its directory's name.
        #[arg(allow_hyphen_values = true)]
        name: String,
    },
    /// Remove a worktree; its branch goes too only when merged into the default branch.
    Rm {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: String,
        /// Remove a worktree with changes and delete an unmerged branch.
        #[arg(long)]
        force: bool,
    },
    /// Clean the worktrees whose directory is gone (`git worktree prune`). Nothing else prunes.
    Prune,
}

pub fn run(cli: &Cli) -> Result<()> {
    let cwd = std::env::current_dir().context("could not read the current directory")?;
    match &cli.command {
        Command::Worktree {
            command: WorktreeCommand::New { name },
        } => {
            let created = worktree::create(&cwd, name, &Env::from_process())?;
            output::worktree_created(&created);
            Ok(())
        }
        Command::Worktree {
            command: WorktreeCommand::Rm { worktree, force },
        } => {
            let removed = worktree::remove(&cwd, worktree, *force)?;
            output::worktree_removed(&removed);
            Ok(())
        }
        Command::Worktree {
            command: WorktreeCommand::Prune,
        } => {
            output::worktrees_pruned(&worktree::prune(&cwd)?);
            Ok(())
        }
        Command::Worktree {
            command: WorktreeCommand::Ls { json },
        } => {
            let json = *json;
            let fleet = Fleet::discover(&cwd)?;
            if json {
                output::json(&fleet)
            } else {
                output::fleet_table(&fleet);
                Ok(())
            }
        }
    }
}
