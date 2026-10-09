//! `orca-term`: the core CLI.

use std::process::ExitCode;

use clap::Parser;

mod cli;
mod config;
mod contract;
mod fleet;
mod git;
mod include;
mod lane;
mod output;
mod state;

fn main() -> ExitCode {
    let cli = cli::Cli::parse();
    match cli::run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            output::error(&err);
            ExitCode::FAILURE
        }
    }
}
