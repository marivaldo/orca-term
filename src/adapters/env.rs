//! Adapter: the process environment, meaning the current directory and the variables the
//! configuration depends on.
#![expect(
    clippy::disallowed_methods,
    reason = "this adapter is the one place that reads the process environment"
)]

use std::path::PathBuf;

use anyhow::{Context, Result};

use crate::domain::config::Env;

/// The directory the core was started in.
pub(crate) fn current_dir() -> Result<PathBuf> {
    std::env::current_dir().context("could not read the current directory")
}

/// `HOME` and `XDG_CONFIG_HOME`, each `None` when unset or empty.
pub(crate) fn config_env() -> Env {
    let var = |name| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    Env {
        home: var("HOME"),
        xdg_config_home: var("XDG_CONFIG_HOME"),
    }
}
