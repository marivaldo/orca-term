//! Domain: what `orca-term` with no arguments opens, and which Neovim the application runs.
//! With no arguments it opens the terminal application only at a terminal and outside Neovim, and
//! prints the usage otherwise (#148, #193). The application's Neovim is the person's own: `nvim`
//! on PATH, or the program [`NVIM_VAR`] names (#176).

/// The variable that names the Neovim to run instead of `nvim` on PATH: a path, or a name looked
/// up on PATH.
pub(crate) const NVIM_VAR: &str = "ORCA_TERM_NVIM";

/// The program run when [`NVIM_VAR`] is unset, looked up on PATH.
pub(crate) const NVIM: &str = "nvim";

/// Whether `orca-term` with no arguments opens the application: only outside Neovim
/// (`inside_nvim` false) and at a terminal (`at_terminal`: stdin and stdout are both terminals).
/// Otherwise it prints the usage, as a restored session's `term://` buffer and a script expect.
pub(crate) fn opens_with_no_arguments(inside_nvim: bool, at_terminal: bool) -> bool {
    !inside_nvim && at_terminal
}

/// The Neovim to run, as [`NVIM_VAR`] names it (`overridden`), or [`NVIM`] when it is unset.
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_arguments_open_the_application_only_at_a_terminal_outside_neovim() {
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
}
