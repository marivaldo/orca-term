//! `orca-term`: the core CLI.

use std::process::ExitCode;

use clap::Parser;

mod cli;
mod contract;
mod fleet;
mod git;
mod output;

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
