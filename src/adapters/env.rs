//! Adapter: the process environment, meaning the current directory, the variables the
//! configuration depends on, the directories of PATH, the running binary's path and the clock.
#![expect(
    clippy::disallowed_methods,
    reason = "this adapter is the one place that reads the process environment"
)]

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::domain::config::Env;
use crate::domain::launch::opencode::DirsEnv;

/// The directory the core was started in.
pub(crate) fn current_dir() -> Result<PathBuf> {
    std::env::current_dir().context("could not read the current directory")
}

/// The absolute path of the running orca-term, for the hooks to call it back.
pub(crate) fn current_exe() -> Result<PathBuf> {
    std::env::current_exe().context("could not find the path of the running orca-term")
}

/// The time now, in milliseconds since the Unix epoch (0 for a clock set before it).
pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The directories of `PATH`, in order; empty when it is unset.
pub(crate) fn path_dirs() -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect())
        .unwrap_or_default()
}

/// The variables opencode finds its own directories by (`HOME`, `XDG_DATA_HOME`,
/// `XDG_CACHE_HOME`, `XDG_CONFIG_HOME`, `XDG_STATE_HOME`, `TMPDIR`), each `None` when unset, empty
/// or not text.
pub(crate) fn opencode_dirs_env() -> DirsEnv {
    let var = |name| std::env::var(name).ok().filter(|value| !value.is_empty());
    DirsEnv {
        home: var("HOME"),
        xdg_data_home: var("XDG_DATA_HOME"),
        xdg_cache_home: var("XDG_CACHE_HOME"),
        xdg_config_home: var("XDG_CONFIG_HOME"),
        xdg_state_home: var("XDG_STATE_HOME"),
        tmpdir: var("TMPDIR"),
    }
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
