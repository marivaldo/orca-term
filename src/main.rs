//! Main: the `orca-term` binary. It parses the arguments, runs the core and turns an error into
//! an exit code.

use std::process::ExitCode;

use clap::Parser;
use orca_term::{Cli, output};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match orca_term::run(&cli) {
        Ok(code) => code,
        Err(err) => output::error(&err, cli.format()),
    }
}
