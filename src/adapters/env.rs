//! Adapter: the process environment, meaning the current directory, the variables the
//! configuration depends on and the directories of PATH.
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

/// The directories of `PATH`, in order; empty when it is unset.
pub(crate) fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default()
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
