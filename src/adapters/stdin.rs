//! Adapter: the process's standard input, read whole (a hook gets claude's payload there), or one
//! line at a time (the person's answer at a prompt).

use std::io::{BufRead as _, Read as _};

use anyhow::{Context, Result};

/// Everything on standard input, until it closes.
pub(crate) fn read_all() -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::io::stdin()
        .read_to_end(&mut bytes)
        .context("could not read standard input")?;
    Ok(bytes)
}

/// One line typed on standard input, with its line ending; empty when the input ended first.
pub(crate) fn read_line() -> Result<String> {
    let mut line = String::new();
    std::io::stdin()
        .lock()
        .read_line(&mut line)
        .context("could not read standard input")?;
    Ok(line)
}
