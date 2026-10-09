//! The `orca-term` core as a library: every module lives here, and the binary only calls [`run`].

mod cli;
mod config;
mod contract;
mod fleet;
mod git;
mod include;
pub mod output;
mod state;
mod worktree;

pub use cli::{Cli, run};
