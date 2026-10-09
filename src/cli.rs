//! The command-line surface.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::config::Env;
use crate::fleet::Fleet;
use crate::{lane, output};

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
    /// Work with the lanes of the current repository.
    Lane {
        #[command(subcommand)]
        command: LaneCommand,
    },
}

#[derive(Debug, Subcommand)]
enum LaneCommand {
    /// List the fleet: every lane of the current repository.
    Ls {
        /// Print JSON carrying the contract integer instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Create a lane: a worktree on a new branch from the default branch.
    New {
        /// The lane's name, which is also its branch and its directory's name.
        #[arg(allow_hyphen_values = true)]
        name: String,
    },
}

pub fn run(cli: &Cli) -> Result<()> {
    let cwd = std::env::current_dir().context("could not read the current directory")?;
    match &cli.command {
        Command::Lane {
            command: LaneCommand::New { name },
        } => {
            let created = lane::create(&cwd, name, &Env::from_process())?;
            output::lane_created(&created);
            Ok(())
        }
        Command::Lane {
            command: LaneCommand::Ls { json },
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
