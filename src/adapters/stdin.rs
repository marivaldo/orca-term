//! Adapter: the process's standard input, read whole. A hook gets claude's payload there.

use std::io::Read as _;

use anyhow::{Context, Result};

/// Everything on standard input, until it closes.
pub(crate) fn read_all() -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .read_to_end(&mut bytes)
        .context("could not read standard input")?;
    Ok(bytes)
}
