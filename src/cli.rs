//! The command-line surface.

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

use crate::fleet::Fleet;
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
}

pub fn run(cli: &Cli) -> Result<()> {
    match cli.command {
        Command::Lane {
            command: LaneCommand::Ls { json },
        } => {
            let cwd = std::env::current_dir().context("could not read the current directory")?;
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
