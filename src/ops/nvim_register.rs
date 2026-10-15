//! Ops: the hidden `nvim register <address>` command, which a running Neovim runs so the core
//! tells it whenever a worktree changes: it records the Neovim's server address in the
//! repository's registry (`domain/nvim.rs`). Registering an address twice changes nothing.

use anyhow::Result;

use crate::adapters::fs;
use crate::domain::nvim::Address;
use crate::ops::nvim_registry;

/// Records `address` in the registry of the repository containing the current directory.
/// Creating its file is the only write, so no lock is needed.
pub(crate) fn run(address: &Address) -> Result<()> {
    let registry = nvim_registry()?;
    fs::create_dir_all(&registry)?;
    fs::write_atomically(&registry.join(address.entry_name()), &address.entry_bytes())
}
