//! Main: the `orca-term` core as a library. It declares the five layers and the terminal
//! application above the operations (see `ARCHITECTURE.md`), and the binary only calls [`run`].

mod adapters;
mod app;
mod cli;
mod domain;
mod ops;
pub mod output;

pub use cli::{Cli, run};
