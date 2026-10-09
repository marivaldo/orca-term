//! Edge: the command-line surface. It parses the arguments, hands each command to its ops module
//! and gives the result to `output`.

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::ops::{worktree_ls, worktree_new, worktree_prune, worktree_rm};
use crate::output;

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

/// Runs the command `cli` names.
pub fn run(cli: &Cli) -> Result<()> {
    match &cli.command {
        Command::Worktree { command } => run_worktree(command),
    }
}

fn run_worktree(command: &WorktreeCommand) -> Result<()> {
    match command {
        WorktreeCommand::New { name } => output::worktree_created(&worktree_new::run(name)?),
        WorktreeCommand::Rm { worktree, force } => {
            output::worktree_removed(&worktree_rm::run(worktree, *force)?);
        }
        WorktreeCommand::Prune => output::worktrees_pruned(&worktree_prune::run()?),
        WorktreeCommand::Ls { json: true } => output::fleet_json(&worktree_ls::run()?)?,
        WorktreeCommand::Ls { json: false } => output::fleet_table(&worktree_ls::run()?),
    }
    Ok(())
}
