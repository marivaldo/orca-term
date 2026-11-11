//! App: what the side menu types (#178): `/` filters its worktrees by what is typed, and `n` asks
//! the name of a new worktree, which the core creates in the project under the cursor. The new
//! worktree opens once a read of the fleets lists it.

use std::path::{Path, PathBuf};
use std::time::Instant;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Effect};
use crate::app::menu;
use crate::domain::worktree::WorktreeName;

/// What the side menu is typing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Typing {
    /// The filter, kept in [`App::filter`]: the list narrows as it is typed.
    Filter,
    /// The name of a new worktree for the project at `project`.
    NewWorktree { project: PathBuf, name: String },
}

impl App {
    /// The worktrees the side menu lists, in its order: those matching its filter.
    pub(super) fn side_menu_order(&self) -> Vec<PathBuf> {
        menu::order_matching(&self.fleets, &self.filter)
    }

    /// Filters the side menu by `filter`, keeping the cursor on a worktree it still lists.
    pub(super) fn set_filter(&mut self, filter: String) {
        self.filter = filter;
        let order = self.side_menu_order();
        let kept = self
            .cursor
            .as_ref()
            .is_some_and(|cursor| order.contains(cursor));
        if !kept && let Some(first) = order.first() {
            self.cursor = Some(first.clone());
        }
    }

    /// `n`: asks the name of a new worktree in the project of the cursor's worktree (else of the
    /// open one, else the first project that can be read).
    pub(super) fn ask_new_worktree(&mut self) {
        let under = self.cursor.as_deref().or(self.open.as_deref());
        let project = under
            .and_then(|path| menu::find(&self.fleets, path))
            .map(|(project, _)| project.path.clone())
            .or_else(|| {
                self.fleets
                    .iter()
                    .find(|project| project.fleet.is_ok())
                    .map(|project| project.path.clone())
            });
        if let Some(project) = project {
            self.typing = Some(Typing::NewWorktree {
                project,
                name: String::new(),
            });
        }
    }

    /// A key while the side menu types: letters type, `backspace` erases, `enter` ends (creating
    /// the worktree), `esc` gives up (clearing the filter).
    pub(super) fn typing_key(
        &mut self,
        typing: Typing,
        key: KeyEvent,
        now: Instant,
    ) -> Vec<Effect> {
        let plain = !key
            .modifiers
            .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT);
        match (typing, key.code) {
            (Typing::Filter, KeyCode::Esc) => self.set_filter(String::new()),
            // Enter keeps the filter typed; esc gives up a new worktree.
            (Typing::Filter, KeyCode::Enter) | (Typing::NewWorktree { .. }, KeyCode::Esc) => {}
            (Typing::Filter, code) => {
                let mut filter = self.filter.clone();
                edit(&mut filter, code, plain);
                self.set_filter(filter);
                self.typing = Some(Typing::Filter);
            }
            (Typing::NewWorktree { project, name }, KeyCode::Enter) => {
                return self.create(project, &name, now);
            }
            (Typing::NewWorktree { project, mut name }, code) => {
                edit(&mut name, code, plain);
                self.typing = Some(Typing::NewWorktree { project, name });
            }
        }
        Vec::new()
    }

    /// Creates the worktree `name` in the project at `project` through the core, once the name
    /// is one; an empty name creates nothing.
    fn create(&mut self, project: PathBuf, name: &str, now: Instant) -> Vec<Effect> {
        let name = name.trim();
        if name.is_empty() {
            return Vec::new();
        }
        match name.parse::<WorktreeName>() {
            Ok(name) => {
                self.say(format!("creating worktree {name}…"), now);
                vec![Effect::NewWorktree { project, name }]
            }
            Err(err) => {
                self.say(err.to_string(), now);
                Vec::new()
            }
        }
    }

    /// The core created the worktree `name` in the project at `project` (`Ok` says how it went)
    /// or could not (`Err` says why). A worktree created opens once the fleets list it.
    pub(crate) fn created(
        &mut self,
        project: &Path,
        name: &str,
        result: Result<String, String>,
        now: Instant,
    ) {
        match result {
            Ok(text) => {
                self.to_open = Some((project.to_owned(), name.to_owned()));
                self.say(text, now);
            }
            Err(why) => self.say(why, now),
        }
    }

    /// Opens the worktree just created, once the fleets list it, with the cursor on it.
    pub(super) fn open_created(&mut self) -> Vec<Effect> {
        let Some((project, name)) = &self.to_open else {
            return Vec::new();
        };
        let found = self
            .fleets
            .iter()
            .filter(|fleet| &fleet.path == project)
            .filter_map(|fleet| fleet.fleet.as_ref().ok())
            .flat_map(|fleet| &fleet.worktrees)
            .find(|worktree| &worktree.name == name)
            .map(|worktree| worktree.path.clone());
        let Some(path) = found else {
            return Vec::new();
        };
        self.to_open = None;
        self.set_filter(String::new());
        self.open(path)
    }
}

/// `code` typed into `text`: a character added (when typed `plain`, with no Ctrl nor Alt), or the
/// last one erased.
fn edit(text: &mut String, code: KeyCode, plain: bool) {
    match code {
        KeyCode::Char(c) if plain => text.push(c),
        KeyCode::Backspace => {
            text.pop();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::*;
    use crate::app::fixture;
    use crate::app::state::Focus;
    use crate::domain::state::State;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn type_text(app: &mut App, text: &str, now: Instant) {
        for c in text.chars() {
            app.key(key(KeyCode::Char(c)), now);
        }
    }

    /// The application with the side menu focused, its cursor on the open `fix`.
    fn in_side_menu() -> (App, Instant) {
        let mut app = App::new(Rect::new(0, 0, 120, 36), None);
        app.set_fleets(fixture::fleets());
        let now = Instant::now();
        app.key(
            KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL),
            now,
        );
        app.key(key(KeyCode::Char('h')), now);
        assert_eq!(app.focus, Focus::SideMenu);
        (app, now)
    }

    fn said(app: &App) -> &str {
        app.message.as_ref().map_or("", |(text, _)| text.as_str())
    }

    #[test]
    fn slash_filters_the_side_menu_and_esc_clears_it() {
        let (mut app, now) = in_side_menu();
        app.key(key(KeyCode::Char('/')), now);
        type_text(&mut app, "ui", now);
        assert_eq!(app.filter, "ui");
        assert_eq!(app.typing, Some(Typing::Filter));
        assert_eq!(app.cursor.as_deref(), Some(Path::new("/p/web.wt/ui")));
        app.key(key(KeyCode::Enter), now);
        assert_eq!(app.typing, None, "enter keeps the filter");
        app.key(key(KeyCode::Char('j')), now);
        assert_eq!(
            app.cursor.as_deref(),
            Some(Path::new("/p/web.wt/ui")),
            "the only one listed"
        );
        app.key(key(KeyCode::Esc), now);
        assert_eq!(app.filter, "");
        app.key(key(KeyCode::Char('j')), now);
        assert_eq!(app.cursor.as_deref(), Some(Path::new("/p/api.wt/fix")));
    }

    #[test]
    fn n_asks_a_name_and_creates_the_worktree_in_the_cursors_project_then_opens_it() {
        let (mut app, now) = in_side_menu();
        app.key(key(KeyCode::Char('j')), now);
        app.key(key(KeyCode::Char('j')), now);
        assert_eq!(app.cursor.as_deref(), Some(Path::new("/p/web.wt/ui")));
        app.key(key(KeyCode::Char('n')), now);
        type_text(&mut app, "a/b", now);
        assert_eq!(app.key(key(KeyCode::Enter), now), [], "not a name");
        assert!(
            said(&app).contains("invalid worktree name"),
            "{}",
            said(&app)
        );

        app.key(key(KeyCode::Char('n')), now);
        type_text(&mut app, "new-thing", now);
        let name: WorktreeName = "new-thing".parse().unwrap();
        assert_eq!(
            app.key(key(KeyCode::Enter), now),
            [Effect::NewWorktree {
                project: PathBuf::from("/p/web"),
                name,
            }]
        );
        assert_eq!(said(&app), "creating worktree new-thing…");

        app.created(
            Path::new("/p/web"),
            "new-thing",
            Ok("created worktree new-thing".to_owned()),
            now,
        );
        let mut fleets = fixture::fleets();
        if let Ok(fleet) = fleets[1].fleet.as_mut() {
            fleet
                .worktrees
                .push(fixture::worktree("/p/web.wt/new-thing", State::NoAgent));
        }
        app.set_fleets(fleets);
        assert_eq!(app.open.as_deref(), Some(Path::new("/p/web.wt/new-thing")));
        assert_eq!(
            app.cursor.as_deref(),
            Some(Path::new("/p/web.wt/new-thing"))
        );
    }

    #[test]
    fn esc_gives_up_a_new_worktree() {
        let (mut app, now) = in_side_menu();
        app.key(key(KeyCode::Char('n')), now);
        type_text(&mut app, "x", now);
        assert_eq!(app.key(key(KeyCode::Esc), now), []);
        assert_eq!(app.typing, None);
    }
}
