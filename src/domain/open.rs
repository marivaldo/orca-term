//! Domain: opening the person's Neovim with the sidebar showing (#148). `orca-term` with no
//! arguments opens it only at a terminal and outside Neovim, and prints the usage otherwise;
//! `orca-term open` opens it whenever it runs outside Neovim. The ops find the program and replace
//! the process with it; this module decides when, with which program and arguments, and what a
//! refusal says.

use crate::domain::error::TypedError;

/// The variable that names the Neovim to open instead of `nvim` on PATH: a path, or a name looked
/// up on PATH.
pub(crate) const NVIM_VAR: &str = "ORCA_TERM_NVIM";

/// The program opened when [`NVIM_VAR`] is unset, looked up on PATH.
pub(crate) const NVIM: &str = "nvim";

/// The arguments Neovim is opened with: run `:OrcaTerm sidebar` once the config is loaded.
pub(crate) const SIDEBAR_ARGS: [&str; 2] = ["-c", "OrcaTerm sidebar"];

/// What `orca-term open` says inside Neovim, where it must not open a second Neovim.
pub(crate) const ALREADY_INSIDE: &str = "already inside Neovim; run :OrcaTerm sidebar";

/// Whether `orca-term` with no arguments opens Neovim: only outside Neovim (`inside_nvim` false)
/// and at a terminal (`at_terminal`: stdin and stdout are both terminals). Otherwise it prints the
/// usage, as a restored session's `term://` buffer and a script expect.
pub(crate) fn opens_with_no_arguments(inside_nvim: bool, at_terminal: bool) -> bool {
    !inside_nvim && at_terminal
}

/// The Neovim to open, as [`NVIM_VAR`] names it (`overridden`), or [`NVIM`] when it is unset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Program {
    /// A path, used as it is: the value holds a `/`.
    Path(String),
    /// A name, looked up on PATH.
    Name(String),
}

impl Program {
    /// The program `overridden`, the value of [`NVIM_VAR`], names; [`NVIM`] when it is unset or
    /// empty.
    pub(crate) fn from_override(overridden: Option<&str>) -> Self {
        match overridden.filter(|value| !value.is_empty()) {
            Some(value) if value.contains('/') => Self::Path(value.to_owned()),
            Some(value) => Self::Name(value.to_owned()),
            None => Self::Name(NVIM.to_owned()),
        }
    }

    /// The `prerequisite` error for this program not being there.
    pub(crate) fn missing(&self) -> TypedError {
        let problem = match self {
            Self::Path(path) => format!("{path}: not an executable file (set by {NVIM_VAR})"),
            Self::Name(name) => format!(
                "{name}: not found on PATH; install Neovim 0.11 or later (https://neovim.io), \
                 or set {NVIM_VAR} to its path"
            ),
        };
        TypedError::Prerequisite {
            problems: vec![problem],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error;

    #[test]
    fn no_arguments_open_neovim_only_at_a_terminal_outside_neovim() {
        assert!(opens_with_no_arguments(false, true));
        assert!(!opens_with_no_arguments(true, true), "inside Neovim");
        assert!(!opens_with_no_arguments(false, false), "not at a terminal");
        assert!(!opens_with_no_arguments(true, false));
    }

    #[test]
    fn the_override_is_a_path_with_a_slash_and_a_name_without() {
        assert_eq!(
            Program::from_override(None),
            Program::Name("nvim".to_owned())
        );
        assert_eq!(
            Program::from_override(Some("")),
            Program::Name("nvim".to_owned())
        );
        assert_eq!(
            Program::from_override(Some("nvim-nightly")),
            Program::Name("nvim-nightly".to_owned())
        );
        assert_eq!(
            Program::from_override(Some("/opt/nvim/bin/nvim")),
            Program::Path("/opt/nvim/bin/nvim".to_owned())
        );
    }

    #[test]
    fn a_missing_neovim_is_a_prerequisite_naming_the_fix() {
        let err = anyhow::Error::new(Program::Name("nvim".to_owned()).missing());
        assert_eq!(error::exit_code(&err), 9);
        assert_eq!(
            err.to_string(),
            "nvim: not found on PATH; install Neovim 0.11 or later (https://neovim.io), or set \
             ORCA_TERM_NVIM to its path; run orca-term doctor for the full report"
        );
        let err = anyhow::Error::new(Program::Path("/no/nvim".to_owned()).missing());
        assert_eq!(error::exit_code(&err), 9);
        assert!(
            err.to_string()
                .starts_with("/no/nvim: not an executable file")
        );
    }
}
