//! App: a worktree's Neovim and its shell, the `2 nvim` and `3 shell` panels (#176). Unlike the
//! agent, no holder keeps them: the application runs each in a terminal of its own
//! (`adapters/pty.rs`), starts it the first time its panel shows, keeps it while the application
//! runs, and ends it on quit. This module is what the state knows of one (its screen and whether
//! it runs), what runs, and where Neovim listens.
//!
//! Neovim is the person's own: `nvim` on PATH, or what `ORCA_TERM_NVIM` names (`domain/open.rs`),
//! started in the worktree with `--listen <socket>` so its address is known
//! without the registry (#172). Its socket goes in a short directory of its own, because macOS
//! takes at most 104 bytes for a unix socket's path, the ending NUL included: under
//! `$XDG_RUNTIME_DIR` when that leaves room, else under `/tmp`. The shell is `$SHELL`.

use std::path::{Path, PathBuf};

use ratatui::layout::Rect;

use crate::adapters::pty::Printed;
use crate::app::emulator::Emulator;
use crate::app::view::Tab;

/// The longest path a unix socket may have on macOS, in bytes, without the ending NUL; Linux
/// takes 107.
pub(crate) const SOCKET_PATH_MAX: usize = 103;

/// The longest name [`socket_path`] gives a socket.
const LONGEST_SOCKET_NAME: &str = "nvim-4294967295-18446744073709551615";

/// The shell run when `$SHELL` is unset or empty.
const FALLBACK_SHELL: &str = "/bin/sh";

/// Which of a worktree's programs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Kind {
    Nvim,
    Shell,
}

impl Kind {
    /// The program shown in `tab`; the agent's tab shows none of these.
    pub(crate) fn of(tab: Tab) -> Option<Self> {
        match tab {
            Tab::Agent => None,
            Tab::Nvim => Some(Self::Nvim),
            Tab::Shell => Some(Self::Shell),
        }
    }

    /// The tab that shows it.
    pub(crate) fn tab(self) -> Tab {
        match self {
            Self::Nvim => Tab::Nvim,
            Self::Shell => Tab::Shell,
        }
    }

    /// Its name, as the panel says it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Nvim => "nvim",
            Self::Shell => "shell",
        }
    }
}

/// One worktree's Neovim or shell.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct ProgramId {
    pub(crate) worktree: PathBuf,
    pub(crate) kind: Kind,
}

/// Whether the program runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    Running,
    /// It ended, as the text says (`exit 0`): `enter` starts it again.
    Ended(String),
    /// It could not start, for the reason the text gives: `enter` tries again.
    Failed(String),
}

/// A worktree's Neovim or shell, as the application shows it.
#[derive(Debug)]
pub(crate) struct Program {
    pub(crate) status: Status,
    pub(crate) screen: Emulator,
}

impl Program {
    /// A program just started, on an empty screen `size` cells large.
    pub(crate) fn new(size: Rect) -> Self {
        Self {
            status: Status::Running,
            screen: Emulator::new(size),
        }
    }

    /// What the program printed, or its end. Returns the answers to the questions its output
    /// asked, to type back into it.
    pub(crate) fn printed(&mut self, printed: Printed) -> Vec<u8> {
        match printed {
            Printed::Output(bytes) => self.screen.feed(&bytes),
            Printed::Ended(how) => {
                self.status = Status::Ended(how);
                Vec::new()
            }
        }
    }

    /// Whether it runs, so keys go to it.
    pub(crate) fn running(&self) -> bool {
        self.status == Status::Running
    }
}

/// The directory the application's Neovims listen in: `orca-term` under `runtime`
/// (`$XDG_RUNTIME_DIR`) when it is absolute and leaves room for the longest socket name, else
/// `/tmp/orca-term-<uid>`, which always does.
pub(crate) fn socket_dir(runtime: Option<&Path>, uid: u32) -> PathBuf {
    let fits =
        |dir: &Path| dir.as_os_str().len() + 1 + LONGEST_SOCKET_NAME.len() <= SOCKET_PATH_MAX;
    runtime
        .filter(|runtime| runtime.is_absolute())
        .map(|runtime| runtime.join("orca-term"))
        .filter(|dir| fits(dir))
        .unwrap_or_else(|| PathBuf::from(format!("/tmp/orca-term-{uid}")))
}

/// The socket of the `number`th Neovim the application with process id `pid` starts, in `dir`.
pub(crate) fn socket_path(dir: &Path, pid: u32, number: u64) -> PathBuf {
    dir.join(format!("nvim-{pid}-{number}"))
}

/// What runs Neovim: `nvim` (the program `ORCA_TERM_NVIM` names, given as `nvim`) listening on
/// `socket`.
pub(crate) fn nvim_argv(nvim: &str, socket: &Path) -> Vec<String> {
    vec![
        nvim.to_owned(),
        "--listen".to_owned(),
        socket.to_string_lossy().into_owned(),
    ]
}

/// What runs the shell: `$SHELL` (given as `shell`), else `/bin/sh`.
pub(crate) fn shell_argv(shell: Option<&str>) -> Vec<String> {
    let shell = shell.filter(|shell| !shell.is_empty());
    vec![shell.unwrap_or(FALLBACK_SHELL).to_owned()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neovim_listens_under_the_runtime_dir_when_it_leaves_room() {
        let dir = socket_dir(Some(Path::new("/run/user/1000")), 1000);
        assert_eq!(dir, Path::new("/run/user/1000/orca-term"));
        assert_eq!(
            socket_path(&dir, 4242, 1),
            Path::new("/run/user/1000/orca-term/nvim-4242-1")
        );
    }

    #[test]
    fn a_runtime_dir_too_long_or_relative_falls_back_to_tmp() {
        let long = format!("/{}", "x".repeat(70));
        assert_eq!(
            socket_dir(Some(Path::new(&long)), 501),
            Path::new("/tmp/orca-term-501")
        );
        assert_eq!(
            socket_dir(Some(Path::new("run")), 501),
            Path::new("/tmp/orca-term-501")
        );
        assert_eq!(socket_dir(None, 501), Path::new("/tmp/orca-term-501"));
    }

    #[test]
    fn every_socket_path_fits_in_a_socket_address() {
        let longest_runtime = format!("/{}", "x".repeat(SOCKET_PATH_MAX));
        for runtime in [None, Some(longest_runtime.as_str()), Some("/run/user/1")] {
            for uid in [0, u32::MAX] {
                let dir = socket_dir(runtime.map(Path::new), uid);
                let path = socket_path(&dir, u32::MAX, u64::MAX);
                assert!(
                    path.as_os_str().len() <= SOCKET_PATH_MAX,
                    "{} is too long",
                    path.display()
                );
            }
        }
        let edge = format!("/{}", "x".repeat(SOCKET_PATH_MAX - 1 - 37 - 10));
        let path = socket_path(&socket_dir(Some(Path::new(&edge)), 1), u32::MAX, u64::MAX);
        assert_eq!(path.as_os_str().len(), SOCKET_PATH_MAX, "the longest kept");
    }

    #[test]
    fn neovim_listens_on_its_socket_and_the_shell_falls_back_to_sh() {
        assert_eq!(
            nvim_argv("nvim", Path::new("/tmp/orca-term-1/nvim-2-3")),
            ["nvim", "--listen", "/tmp/orca-term-1/nvim-2-3"]
        );
        assert_eq!(
            shell_argv(Some("/opt/homebrew/bin/fish")),
            ["/opt/homebrew/bin/fish"]
        );
        assert_eq!(shell_argv(Some("")), ["/bin/sh"]);
        assert_eq!(shell_argv(None), ["/bin/sh"]);
    }

    #[test]
    fn a_program_answers_through_its_screen_and_ends() {
        let mut program = Program::new(Rect::new(0, 0, 20, 4));
        assert_eq!(
            program.printed(Printed::Output(b"\x1b[c".to_vec())),
            b"\x1b[?62;22c"
        );
        assert!(program.running());
        program.printed(Printed::Ended("exit 0".to_owned()));
        assert_eq!(program.status, Status::Ended("exit 0".to_owned()));
        assert!(!program.running());
    }
}
