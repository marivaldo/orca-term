//! The `orca-term` binary: parses the arguments, runs the core and turns an error into an exit code.

use std::process::ExitCode;

use clap::Parser;
use orca_term::{Cli, output};

fn main() -> ExitCode {
    match orca_term::run(&Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            output::error(&err);
            ExitCode::FAILURE
        }
    }
}
