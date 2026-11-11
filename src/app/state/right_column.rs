//! App: what the state does with the right column (#177): `^a f` shows or hides it (a column from
//! 150 columns, an overlay on the centre under that), the focus reaches it with `^a l`, `j` and `k`
//! move its cursor over the changed files, and enter opens the file's diff in the worktree's
//! Neovim, which then shows with the focus, zoomed when the right column was an overlay. `esc` or
//! `^a h` leaves it.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent};

use super::{App, Effect, Focus};
use crate::app::menu;
use crate::app::view::Tab;
use crate::domain::changes::Changes;

/// A worktree's changes, as last read, or why they could not be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChangesRead {
    /// The worktree they are of.
    pub(crate) worktree: PathBuf,
    pub(crate) read: Result<Changes, String>,
}

impl App {
    /// The open worktree's changes, once read.
    pub(crate) fn open_changes(&self) -> Option<&Changes> {
        self.read_of_open()?.read.as_ref().ok()
    }

    /// Why the open worktree's changes could not be read, when they could not.
    pub(crate) fn changes_error(&self) -> Option<&str> {
        self.read_of_open()?.read.as_ref().err().map(String::as_str)
    }

    fn read_of_open(&self) -> Option<&ChangesRead> {
        self.changes
            .as_ref()
            .filter(|changes| self.open.as_ref() == Some(&changes.worktree))
    }

    /// The changes of the worktree at `worktree`, read now, or why they could not be. The cursor
    /// stays on the list.
    pub(crate) fn changes_read(&mut self, worktree: PathBuf, read: Result<Changes, String>) {
        self.changes = Some(ChangesRead { worktree, read });
        let files = self.open_changes().map_or(0, |changes| changes.files.len());
        self.file_cursor = self.file_cursor.min(files.saturating_sub(1));
    }

    /// `^a f`: the column on or off where it fits; else the overlay on, with the focus, or off.
    pub(super) fn toggle_files(&mut self) {
        if self.layout.column_fits {
            self.view.files.column = !self.view.files.column;
            return;
        }
        if self.view.files.overlay {
            self.leave_right_column();
        } else {
            self.view.files.overlay = true;
            self.focus = Focus::RightColumn;
        }
    }

    /// A key for the focused right column.
    pub(super) fn right_column_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let files = self.open_changes().map_or(0, |changes| changes.files.len());
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.file_cursor = (self.file_cursor + 1).min(files.saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.file_cursor = self.file_cursor.saturating_sub(1);
            }
            KeyCode::Enter => return self.open_diff(),
            KeyCode::Esc => self.leave_right_column(),
            _ => {}
        }
        Vec::new()
    }

    /// Leaves the right column for the centre, closing the overlay.
    pub(super) fn leave_right_column(&mut self) {
        self.view.files.overlay = false;
        self.focus = Focus::Centre;
    }

    /// Enter on a changed file: its diff opens in the worktree's Neovim, shown with the focus, and
    /// zoomed when the right column was an overlay.
    fn open_diff(&mut self) -> Vec<Effect> {
        let Some(worktree) = self.open.clone() else {
            return Vec::new();
        };
        let Some((project, _)) = menu::find(&self.fleets, &worktree) else {
            return Vec::new();
        };
        let primary = project.path.clone();
        let Some(file) = self
            .open_changes()
            .and_then(|changes| changes.files.get(self.file_cursor))
            .map(|file| file.path.clone())
        else {
            return Vec::new();
        };
        if self.view.files.overlay {
            self.view.zoom = true;
        }
        self.view.files.overlay = false;
        self.view.show(Tab::Nvim);
        self.focus = Focus::Centre;
        // The worktree's Neovim starts first, so the run loop knows to keep the diff for it until
        // it listens.
        self.relayout();
        let mut effects = self.start_shown();
        effects.push(Effect::OpenDiff {
            primary,
            worktree,
            file,
        });
        effects
    }
}
