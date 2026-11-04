//! Ops: `orca-term open` and `orca-term` with no arguments, which replace orca-term with the
//! person's own Neovim running `:OrcaTerm sidebar` (#148). `domain/open.rs` decides when and with
//! what; this module asks the environment and the terminal, finds the program and runs it.

use std::convert::Infallible;
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};

use crate::adapters::{env, fs, program, terminal};
use crate::domain::open::{self, Program};

/// Runs `orca-term` with no arguments: opens Neovim when it runs at a terminal and outside Neovim,
/// and never returns then. Otherwise it returns at once, and the caller prints the usage.
pub(crate) fn run_with_no_arguments() -> Result<()> {
    if open::opens_with_no_arguments(
        env::inside_nvim(),
        terminal::stdin_and_stdout_are_terminals(),
    ) {
        open_neovim()?;
    }
    Ok(())
}

/// Runs `orca-term open`: opens Neovim whatever stdin and stdout are, and never returns then.
/// Inside Neovim it refuses, since it would open a Neovim inside Neovim.
pub(crate) fn run() -> Result<Infallible> {
    if env::inside_nvim() {
        bail!(open::ALREADY_INSIDE);
    }
    open_neovim()
}

/// Replaces orca-term with Neovim running `:OrcaTerm sidebar`. It returns only with an error: the
/// typed `prerequisite` when the program is not there, or why it could not be started.
fn open_neovim() -> Result<Infallible> {
    let nvim = find_nvim()?;
    let why = program::replace_with(&nvim, &open::SIDEBAR_ARGS);
    Err(anyhow!(why).context(format!("could not run {}", nvim.display())))
}

/// The Neovim to open: `ORCA_TERM_NVIM` when it is set (a path as it is, a name on PATH), else
/// `nvim` on PATH. Refuses as the typed `prerequisite` when it is not there.
fn find_nvim() -> Result<PathBuf> {
    let program = Program::from_override(env::nvim_override().as_deref());
    let found = match &program {
        Program::Path(path) => Some(PathBuf::from(path)).filter(|path| fs::is_file(path)),
        Program::Name(name) => fs::executable_in(&env::path_dirs(), name),
    };
    found.ok_or_else(|| program.missing().into())
}
