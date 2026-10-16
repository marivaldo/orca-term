//! Ops: the hidden `nvim unregister <address>` command, which a Neovim runs as it exits: it takes
//! the Neovim's server address out of the repository's registry (`domain/nvim.rs`). An address
//! that is not registered is not an error.

use anyhow::Result;

use crate::adapters::fs;
use crate::domain::nvim::Address;
use crate::ops::nvim_registry;

/// Removes `address` from the registry of the repository containing the current directory.
/// Removing its file is the only write, so no lock is needed.
pub(crate) fn run(address: &Address) -> Result<()> {
    fs::remove_if_exists(&nvim_registry()?.join(address.entry_name()))
}
