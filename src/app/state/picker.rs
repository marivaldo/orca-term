//! App: the picker (#178), `^a w`: every worktree in a card floating over the screen, filtered by
//! what is typed. The arrows (or `Ctrl-j`/`Ctrl-k`, `Ctrl-n`/`Ctrl-p`) move, `enter` opens the
//! worktree under the cursor, `esc` closes. Letters, `j` and `k` too, type into the filter: the
//! picker is how a narrow screen, with no side menu, reaches every worktree by name.

use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Effect, Focus, Overlay};
use crate::app::menu;

/// The picker's filter and cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Picker {
    /// What was typed; empty lists every worktree.
    pub(crate) query: String,
    /// The worktree under the cursor, when any matches.
    pub(crate) cursor: Option<PathBuf>,
}

impl App {
    /// `^a w`: opens the picker with the cursor on the open worktree.
    pub(super) fn open_picker(&mut self) {
        self.overlay = Some(Overlay::Picker(Picker {
            query: String::new(),
            cursor: self.open.clone(),
        }));
    }

    /// The worktrees the picker lists for `query`, in the side menu's order.
    pub(crate) fn picker_items(&self, query: &str) -> Vec<PathBuf> {
        menu::order_matching(&self.fleets, query)
    }

    /// A key for the open `picker`. It stays open unless the key opens a worktree or closes it.
    pub(super) fn picker_key(&mut self, mut picker: Picker, key: KeyEvent) -> Vec<Effect> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let items = self.picker_items(&picker.query);
        match key.code {
            KeyCode::Esc => return Vec::new(),
            KeyCode::Enter => {
                let Some(path) = picker.cursor.filter(|cursor| items.contains(cursor)) else {
                    return Vec::new();
                };
                self.focus = Focus::Centre;
                return self.open(path);
            }
            code if step_of(code, ctrl).is_some() => {
                let by = step_of(code, ctrl).unwrap_or(0);
                picker.cursor = menu::step(&items, picker.cursor.as_deref(), by);
            }
            KeyCode::Backspace => {
                picker.query.pop();
                self.keep_cursor(&mut picker);
            }
            KeyCode::Char(c) if !ctrl && !key.modifiers.contains(KeyModifiers::ALT) => {
                picker.query.push(c);
                self.keep_cursor(&mut picker);
            }
            _ => {}
        }
        self.overlay = Some(Overlay::Picker(picker));
        Vec::new()
    }

    /// Keeps the cursor on a worktree the picker still lists, else puts it on the first one.
    fn keep_cursor(&self, picker: &mut Picker) {
        let items = self.picker_items(&picker.query);
        if !picker
            .cursor
            .as_ref()
            .is_some_and(|cursor| items.contains(cursor))
        {
            picker.cursor = items.first().cloned();
        }
    }
}

/// How far a key moves the picker's cursor: down for the down arrow, tab, `Ctrl-j` and `Ctrl-n`,
/// up for the up arrow, shift-tab, `Ctrl-k` and `Ctrl-p`; `None` for any other key.
fn step_of(code: KeyCode, ctrl: bool) -> Option<isize> {
    match code {
        KeyCode::Down | KeyCode::Tab => Some(1),
        KeyCode::Up | KeyCode::BackTab => Some(-1),
        KeyCode::Char('j' | 'n') if ctrl => Some(1),
        KeyCode::Char('k' | 'p') if ctrl => Some(-1),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Instant;

    use ratatui::layout::Rect;

    use super::*;
    use crate::app::fixture;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    /// The application with the picker open (`^a w`), on the open `fix`.
    fn picking() -> (App, Instant) {
        let mut app = App::new(Rect::new(0, 0, 90, 30), None);
        app.set_fleets(fixture::fleets());
        let now = Instant::now();
        app.key(ctrl('a'), now);
        app.key(key(KeyCode::Char('w')), now);
        (app, now)
    }

    fn cursor(app: &App) -> Option<&Path> {
        match &app.overlay {
            Some(Overlay::Picker(picker)) => picker.cursor.as_deref(),
            _ => None,
        }
    }

    #[test]
    fn the_picker_opens_on_the_open_worktree_and_moves_with_the_arrows_and_ctrl() {
        let (mut app, now) = picking();
        assert_eq!(cursor(&app), Some(Path::new("/p/api.wt/fix")));
        app.key(key(KeyCode::Down), now);
        assert_eq!(cursor(&app), Some(Path::new("/p/api.wt/docs")));
        app.key(ctrl('j'), now);
        assert_eq!(cursor(&app), Some(Path::new("/p/web.wt/ui")));
        app.key(ctrl('k'), now);
        app.key(key(KeyCode::Up), now);
        assert_eq!(cursor(&app), Some(Path::new("/p/api.wt/fix")));
        assert_eq!(app.key(key(KeyCode::Esc), now), []);
        assert!(app.overlay.is_none());
        assert_eq!(app.open.as_deref(), Some(Path::new("/p/api.wt/fix")));
    }

    #[test]
    fn typing_filters_the_picker_and_enter_opens_the_worktree_under_the_cursor() {
        let (mut app, now) = picking();
        for c in "ui".chars() {
            assert_eq!(app.key(key(KeyCode::Char(c)), now), [], "typed, not sent");
        }
        assert_eq!(cursor(&app), Some(Path::new("/p/web.wt/ui")));
        assert_eq!(app.picker_items("ui").len(), 1);
        app.key(key(KeyCode::Backspace), now);
        app.key(key(KeyCode::Char('j')), now);
        assert!(
            matches!(&app.overlay, Some(Overlay::Picker(picker)) if picker.query == "uj"),
            "j types too"
        );
        app.key(key(KeyCode::Backspace), now);
        app.key(key(KeyCode::Char('i')), now);
        app.key(key(KeyCode::Enter), now);
        assert!(app.overlay.is_none());
        assert_eq!(app.open.as_deref(), Some(Path::new("/p/web.wt/ui")));
        assert_eq!(app.focus, Focus::Centre);
    }

    #[test]
    fn help_opens_with_the_prefix_and_any_key_closes_it_and_does_nothing_else() {
        let mut app = App::new(Rect::new(0, 0, 120, 36), None);
        app.set_fleets(fixture::fleets());
        let now = Instant::now();
        app.key(ctrl('a'), now);
        app.key(key(KeyCode::Char('?')), now);
        assert_eq!(app.overlay, Some(Overlay::Help));
        assert_eq!(app.key(key(KeyCode::Char('x')), now), [], "not typed");
        assert!(app.overlay.is_none());
        assert_eq!(
            app.key(key(KeyCode::Char('x')), now),
            [Effect::Type(b"x".to_vec())]
        );
    }
}
