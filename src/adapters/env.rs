//! Adapter: the process environment, meaning the current directory, the variables the
//! configuration depends on, the directories of PATH, whether it runs inside Neovim and which
//! Neovim to open, the shell, the directory for sockets, the process's user and id, the running
//! binary's path, the clock, the live checks' recording tap and the
//! socket of the person's ssh agent.
#![expect(
    clippy::disallowed_methods,
    reason = "this adapter is the one place that reads the process environment"
)]

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

use crate::domain::config::Env;
use crate::domain::hook::record;
use crate::domain::launch::opencode::DirsEnv;
use crate::domain::open::NVIM_VAR;

/// The directory the core was started in.
pub(crate) fn current_dir() -> Result<PathBuf> {
    std::env::current_dir().context("could not read the current directory")
}

/// Makes `dir` the process's current directory.
pub(crate) fn enter_dir(dir: &Path) -> Result<()> {
    std::env::set_current_dir(dir).with_context(|| format!("could not enter {}", dir.display()))
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

/// Whether this process runs inside Neovim: Neovim sets `NVIM` to its server address in the
/// terminals it opens, so a non-empty `NVIM` means inside (#148).
pub(crate) fn inside_nvim() -> bool {
    std::env::var_os("NVIM").is_some_and(|value| !value.is_empty())
}

/// The Neovim the person asks the application to run (`ORCA_TERM_NVIM`), or `None` when it is
/// unset or not text (#148, #176).
pub(crate) fn nvim_override() -> Option<String> {
    std::env::var(NVIM_VAR).ok()
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

/// `HOME`, `XDG_CONFIG_HOME` and `XDG_STATE_HOME`, each `None` when unset or empty.
pub(crate) fn config_env() -> Env {
    let var = |name| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    Env {
        home: var("HOME"),
        xdg_config_home: var("XDG_CONFIG_HOME"),
        xdg_state_home: var("XDG_STATE_HOME"),
    }
}

/// What the terminal says of its colours: whether `NO_COLOR` is set and not empty, and
/// `COLORTERM` (`truecolor`, `24bit`), when it is text (#174).
pub(crate) fn colour_env() -> (bool, Option<String>) {
    let no_color = std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty());
    (no_color, std::env::var("COLORTERM").ok())
}

/// The person's shell, `SHELL`, or `None` when it is unset, empty or not text (#176).
pub(crate) fn shell() -> Option<String> {
    std::env::var("SHELL")
        .ok()
        .filter(|value| !value.is_empty())
}

/// `XDG_RUNTIME_DIR`, the directory for this user's sockets, or `None` when it is unset or empty
/// (#176).
pub(crate) fn runtime_dir() -> Option<PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// This process's user id.
pub(crate) fn uid() -> u32 {
    rustix::process::getuid().as_raw()
}

/// This process's id.
pub(crate) fn pid() -> u32 {
    std::process::id()
}

/// The socket of the person's ssh agent, as `SSH_AUTH_SOCK` names it, or `None` when it is unset
/// or empty: the one `sandbox.agent_socket` lets the agent reach (#105). Never printed.
pub(crate) fn ssh_auth_sock() -> Option<PathBuf> {
    std::env::var_os("SSH_AUTH_SOCK")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

/// The file the live checks' recording tap appends to (`ORCA_TERM_RECORD`), or `None`, the tap
/// off, when the variable is unset or empty (#44).
pub(crate) fn record_file() -> Option<PathBuf> {
    record::target(std::env::var_os(record::RECORD_VAR).as_deref())
}
