//! Ops: the hidden `nvim register <address>` command, which a running Neovim runs so the core
//! tells it whenever a worktree changes: it records the Neovim's server address in the
//! repository's registry (`domain/nvim.rs`). Registering an address twice changes nothing.

use std::path::Path;

use anyhow::Result;

use crate::adapters::fs;
use crate::domain::nvim::Address;
use crate::ops::{nvim_registry, nvim_registry_of};

/// Records `address` in the registry of the repository containing the current directory.
/// Creating its file is the only write, so no lock is needed.
pub(crate) fn run(address: &Address) -> Result<()> {
    record(&nvim_registry()?, address)
}

/// Records `address` in the registry of the repository containing `dir`: what the application
/// does for each Neovim it starts in a worktree (#176).
pub(crate) fn run_in(dir: &Path, address: &Address) -> Result<()> {
    record(&nvim_registry_of(dir)?, address)
}

fn record(registry: &Path, address: &Address) -> Result<()> {
    fs::create_dir_all(registry)?;
    fs::write_atomically(&registry.join(address.entry_name()), &address.entry_bytes())
}
