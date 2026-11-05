//! Ops: `orca-term open` and `orca-term` with no arguments, which replace orca-term with the
//! person's own Neovim running `:OrcaTerm sidebar` (#148). `domain/open.rs` decides when and with
//! what; this module asks the environment and the terminal, finds the program and runs it. Just
//! before it runs it, it adds the repository of the current directory to the project list (#152).

use std::convert::Infallible;
use std::path::PathBuf;

use anyhow::{Result, anyhow, bail};

use crate::adapters::{env, fs, program, terminal};
use crate::domain::open::{self, Program};
use crate::ops::project_add;

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

/// Replaces orca-term with Neovim running `:OrcaTerm sidebar`, once the repository of the current
/// directory is on the project list. It returns only with an error: the typed `prerequisite` when
/// the program is not there (nothing is added then), or why it could not be started.
fn open_neovim() -> Result<Infallible> {
    let nvim = find_nvim()?;
    add_current_repository();
    let why = program::replace_with(&nvim, &open::SIDEBAR_ARGS);
    Err(anyhow!(why).context(format!("could not run {}", nvim.display())))
}

/// Adds the repository containing the current directory to the project list, as `project add`
/// does (#152). It says nothing, whether it added it, it was already there or the directory is
/// outside every repository. A project list that cannot be changed (a broken `projects.json`, a
/// newer schema, its lock held, git missing) never stops the launch: nothing is added, and the
/// sidebar shows the problem when it reads the same list through `fleets`.
fn add_current_repository() {
    let Ok(dir) = env::current_dir() else {
        return;
    };
    // Whatever happened, the launch goes on: the result is ignored on purpose.
    let _ = project_add::add_repository_of(&dir);
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
