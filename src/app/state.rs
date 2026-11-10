//! App: the application's state and what each key does to it (#174). Nothing here touches the
//! terminal or the holder: a key gives back the effects the run loop carries out (bytes for the
//! agent, a holder to attach to, an agent to start, quitting), so every key is tested on its own.
//!
//! Every key goes to the focused part, the side menu or the agent's panel, except the prefix,
//! `Ctrl-a`, which arms the application's own keys for one keystroke (ADR 0012): `h` and `l` move
//! the focus, `j` and `k` open the next or previous worktree, `a` sends a literal `Ctrl-a`, `q`
//! quits, and `esc` (or waiting 1.5 s) disarms it.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use crate::app::layout::{self, Layout};
use crate::app::panel::{Panel, Status};
use crate::app::{keys, menu};
use crate::domain::agent::Size;
use crate::domain::project::ProjectFleet;

/// How long the prefix waits for its key.
pub(crate) const PREFIX_FOR: Duration = Duration::from_millis(1500);

/// How long a message stays in the status bar.
const MESSAGE_FOR: Duration = Duration::from_secs(6);

/// The prefix's letter: `Ctrl` and it arm the prefix, and after the prefix it sends itself.
const PREFIX: char = 'a';

/// Where the keys go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    SideMenu,
    Centre,
}

/// What the run loop must do after a key or a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Type these bytes into the open worktree's agent.
    Type(Vec<u8>),
    /// Attach the agent's panel to the holder of the worktree at this path.
    Attach(PathBuf),
    /// Start the agent of the worktree at this path through the core, at this size.
    Start(PathBuf, Size),
    /// Quit the application. The agents keep running.
    Quit,
}

/// Everything the application shows and remembers.
#[derive(Debug)]
pub(crate) struct App {
    /// Every project's fleet, as the core last read it.
    pub(crate) fleets: Vec<ProjectFleet>,
    /// The worktree whose agent the centre shows.
    pub(crate) open: Option<PathBuf>,
    /// The side menu's cursor, while it has the focus.
    pub(crate) cursor: Option<PathBuf>,
    pub(crate) focus: Focus,
    /// When the prefix was armed, while it waits for its key.
    pub(crate) prefix: Option<Instant>,
    /// The open worktree's agent terminal.
    pub(crate) panel: Option<Panel>,
    /// A word for the person in the status bar, and since when.
    pub(crate) message: Option<(String, Instant)>,
    /// Where everything went in the last frame.
    pub(crate) layout: Layout,
    /// The working glyph's frame.
    pub(crate) frame: usize,
    /// The worktree to open first, when the fleets come: the one the application was started in.
    start_in: Option<PathBuf>,
}

impl App {
    /// The application on a screen of `area`, before the fleets are read; it opens the worktree
    /// containing `start_in` first when there is one.
    pub(crate) fn new(area: Rect, start_in: Option<PathBuf>) -> Self {
        Self {
            fleets: Vec::new(),
            open: None,
            cursor: None,
            focus: Focus::Centre,
            prefix: None,
            panel: None,
            message: None,
            layout: layout::layout(area),
            frame: 0,
            start_in,
        }
    }

    /// The screen is now `area` large. Returns the agent's new size when the panel changed size,
    /// for its holder.
    pub(crate) fn fit(&mut self, area: Rect) -> Option<Size> {
        self.layout = layout::layout(area);
        if self.layout.side_menu.is_none() && self.focus == Focus::SideMenu {
            self.focus = Focus::Centre;
        }
        let body = self.layout.body;
        let panel = self.panel.as_mut()?;
        panel.resize(body).then(|| size_of(panel))
    }

    /// The fleets the core read now. Opens a worktree when none is open (or the open one is gone),
    /// and attaches to the open one's agent when it started meanwhile.
    pub(crate) fn set_fleets(&mut self, fleets: Vec<ProjectFleet>) -> Vec<Effect> {
        self.fleets = fleets;
        let still_there = self
            .open
            .as_deref()
            .is_some_and(|open| menu::find(&self.fleets, open).is_some());
        if !still_there {
            if let Some(path) = self.first_to_open() {
                return self.open(path);
            }
            self.open = None;
            self.panel = None;
            return Vec::new();
        }
        let (Some(open), Some(panel)) = (&self.open, &self.panel) else {
            return Vec::new();
        };
        if panel.status == Status::NoAgent && self.holder_alive(open) {
            return vec![Effect::Attach(open.clone())];
        }
        Vec::new()
    }

    /// The worktree containing the directory the application started in, else the first one.
    fn first_to_open(&self) -> Option<PathBuf> {
        let order = menu::order(&self.fleets);
        let started_in = self
            .start_in
            .as_deref()
            .and_then(|dir| order.iter().find(|path| dir.starts_with(path)));
        started_in.or(order.first()).cloned()
    }

    fn holder_alive(&self, path: &Path) -> bool {
        menu::find(&self.fleets, path).is_some_and(|(_, worktree)| worktree.holder.is_alive())
    }

    /// Opens the worktree at `path` in the centre, with a fresh panel attached to its agent when
    /// one runs.
    pub(crate) fn open(&mut self, path: PathBuf) -> Vec<Effect> {
        if self.open.as_ref() == Some(&path) && self.panel.is_some() {
            return Vec::new();
        }
        let alive = self.holder_alive(&path);
        self.open = Some(path.clone());
        self.cursor = Some(path.clone());
        let status = if alive {
            Status::Attached
        } else {
            Status::NoAgent
        };
        self.panel = Some(Panel::new(path.clone(), status, self.layout.body));
        if alive {
            vec![Effect::Attach(path)]
        } else {
            Vec::new()
        }
    }

    /// A key the person pressed at `now`.
    pub(crate) fn key(&mut self, key: KeyEvent, now: Instant) -> Vec<Effect> {
        let armed = self
            .prefix
            .take()
            .is_some_and(|at| now.duration_since(at) < PREFIX_FOR);
        if armed {
            return self.command(key);
        }
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char(PREFIX) {
            self.prefix = Some(now);
            return Vec::new();
        }
        match self.focus {
            Focus::SideMenu => self.side_menu_key(key),
            Focus::Centre => self.centre_key(key),
        }
    }

    /// The key after the prefix.
    fn command(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => {
                if self.layout.side_menu.is_some() {
                    self.focus = Focus::SideMenu;
                    self.cursor.clone_from(&self.open);
                }
                Vec::new()
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.focus = Focus::Centre;
                Vec::new()
            }
            KeyCode::Char('j') | KeyCode::Down => self.open_step(1),
            KeyCode::Char('k') | KeyCode::Up => self.open_step(-1),
            KeyCode::Char(PREFIX) if self.typing() => {
                vec![Effect::Type(keys::encode(
                    KeyEvent::new(KeyCode::Char(PREFIX), KeyModifiers::CONTROL),
                    false,
                ))]
            }
            KeyCode::Char('q') => vec![Effect::Quit],
            _ => Vec::new(),
        }
    }

    /// Opens the worktree `by` steps after (or before) the open one in the side menu's order.
    fn open_step(&mut self, by: isize) -> Vec<Effect> {
        let order = menu::order(&self.fleets);
        match menu::step(&order, self.open.as_deref(), by) {
            Some(path) => self.open(path),
            None => Vec::new(),
        }
    }

    fn side_menu_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let order = menu::order(&self.fleets);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.cursor = menu::step(&order, self.cursor.as_deref(), 1);
                Vec::new()
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.cursor = menu::step(&order, self.cursor.as_deref(), -1);
                Vec::new()
            }
            KeyCode::Enter => {
                self.focus = Focus::Centre;
                self.cursor
                    .clone()
                    .map(|path| self.open(path))
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        }
    }

    /// A key for the agent's panel: every key goes to a running agent; where none runs, `enter`
    /// starts it.
    fn centre_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        let size = self.panel.as_ref().map(size_of);
        let Some(panel) = self.panel.as_mut() else {
            return Vec::new();
        };
        match &panel.status {
            Status::Attached => vec![Effect::Type(keys::encode(key, panel.application_cursor()))],
            Status::NoAgent | Status::Ended(_) if key.code == KeyCode::Enter => {
                panel.status = Status::Starting;
                let size = size.unwrap_or(Size::DEFAULT);
                vec![Effect::Start(panel.worktree.clone(), size)]
            }
            Status::NoAgent | Status::Ended(_) | Status::Starting => Vec::new(),
        }
    }

    /// Whether keys go to a running agent right now.
    fn typing(&self) -> bool {
        self.focus == Focus::Centre
            && self
                .panel
                .as_ref()
                .is_some_and(|panel| panel.status == Status::Attached)
    }

    /// Text the person pasted: the agent's, when keys go to it.
    pub(crate) fn paste(&mut self, text: &str) -> Vec<Effect> {
        match &self.panel {
            Some(panel) if self.typing() => {
                vec![Effect::Type(keys::paste(text, panel.bracketed_paste()))]
            }
            _ => Vec::new(),
        }
    }

    /// Time passed: the prefix and the message expire, and the working glyph turns.
    pub(crate) fn tick(&mut self, now: Instant) {
        if self
            .prefix
            .is_some_and(|at| now.duration_since(at) >= PREFIX_FOR)
        {
            self.prefix = None;
        }
        if self
            .message
            .as_ref()
            .is_some_and(|(_, at)| now.duration_since(*at) >= MESSAGE_FOR)
        {
            self.message = None;
        }
        if menu::any_working(&self.fleets) {
            self.frame = self.frame.wrapping_add(1);
        }
    }

    /// Shows `text` in the status bar for a while.
    pub(crate) fn say(&mut self, text: impl Into<String>, now: Instant) {
        self.message = Some((text.into(), now));
    }

    /// The core started the agent of the worktree at `path`, or could not (`Err` says why).
    pub(crate) fn started(
        &mut self,
        path: &Path,
        result: Result<(), String>,
        now: Instant,
    ) -> Vec<Effect> {
        let Some(panel) = self.panel.as_mut().filter(|panel| panel.worktree == path) else {
            return Vec::new();
        };
        match result {
            Ok(()) => vec![Effect::Attach(path.to_owned())],
            Err(why) => {
                panel.status = Status::NoAgent;
                self.say(why, now);
                Vec::new()
            }
        }
    }

    /// The panel of the worktree at `path` is now attached to its holder, which replays its
    /// screen from the start, on a fresh screen.
    pub(crate) fn attached(&mut self, path: &Path) {
        if let Some(panel) = self.panel.as_mut().filter(|panel| panel.worktree == path) {
            *panel = Panel::new(path.to_owned(), Status::Attached, self.layout.body);
        }
    }

    /// Attaching to the holder of the worktree at `path` failed, for `why`.
    pub(crate) fn attach_failed(&mut self, path: &Path, why: String, now: Instant) {
        if let Some(panel) = self.panel.as_mut().filter(|panel| panel.worktree == path) {
            panel.status = Status::NoAgent;
        }
        self.say(why, now);
    }
}

/// The agent's size: its panel's.
fn size_of(panel: &Panel) -> Size {
    let (rows, cols) = panel.size();
    Size { rows, cols }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::fixture;
    use crate::domain::agent::HolderStatus;
    use crate::domain::state::State;

    const SCREEN: Rect = Rect::new(0, 0, 120, 36);

    fn press(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl_a() -> KeyEvent {
        KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)
    }

    /// The application on two projects: `api` with `fix` (its agent running) and `docs` (none),
    /// and `web` with `ui`.
    fn app() -> (App, Instant) {
        let mut fix = fixture::worktree("/p/api.wt/fix", State::Working);
        fix.holder = HolderStatus::Alive;
        let fleets = vec![
            fixture::project(
                "/p/api",
                vec![fix, fixture::worktree("/p/api.wt/docs", State::Idle)],
            ),
            fixture::project(
                "/p/web",
                vec![fixture::worktree("/p/web.wt/ui", State::NoAgent)],
            ),
        ];
        let mut app = App::new(SCREEN, None);
        let effects = app.set_fleets(fleets);
        assert_eq!(effects, [Effect::Attach(PathBuf::from("/p/api.wt/fix"))]);
        (app, Instant::now())
    }

    fn open(app: &App) -> &str {
        app.open
            .as_deref()
            .and_then(Path::to_str)
            .unwrap_or_default()
    }

    #[test]
    fn keys_go_to_the_agent_until_the_prefix_is_armed() {
        let (mut app, now) = app();
        assert_eq!(
            app.key(press(KeyCode::Char('j')), now),
            [Effect::Type(b"j".to_vec())]
        );
        assert_eq!(app.key(ctrl_a(), now), []);
        assert!(app.prefix.is_some());
        assert_eq!(app.key(press(KeyCode::Char('j')), now), []);
        assert_eq!(open(&app), "/p/api.wt/docs", "^a j opens the next worktree");
        assert!(app.prefix.is_none(), "the prefix lasts one key");
    }

    #[test]
    fn the_prefix_sends_a_literal_ctrl_a_quits_and_cancels() {
        let (mut app, now) = app();
        app.key(ctrl_a(), now);
        assert_eq!(
            app.key(press(KeyCode::Char('a')), now),
            [Effect::Type(vec![0x01])]
        );
        app.key(ctrl_a(), now);
        assert_eq!(app.key(press(KeyCode::Esc), now), []);
        assert_eq!(
            app.key(press(KeyCode::Char('q')), now),
            [Effect::Type(b"q".to_vec())]
        );
        app.key(ctrl_a(), now);
        assert_eq!(app.key(press(KeyCode::Char('q')), now), [Effect::Quit]);
    }

    #[test]
    fn the_prefix_expires() {
        let (mut app, now) = app();
        app.key(ctrl_a(), now);
        app.tick(now + PREFIX_FOR);
        assert!(app.prefix.is_none());
        app.key(ctrl_a(), now);
        let later = now + PREFIX_FOR + Duration::from_millis(1);
        assert_eq!(
            app.key(press(KeyCode::Char('q')), later),
            [Effect::Type(b"q".to_vec())]
        );
    }

    #[test]
    fn the_previous_worktree_of_the_first_is_the_last() {
        let (mut app, now) = app();
        app.key(ctrl_a(), now);
        app.key(press(KeyCode::Char('k')), now);
        assert_eq!(open(&app), "/p/web.wt/ui");
    }

    #[test]
    fn the_side_menu_moves_its_cursor_and_opens_with_enter() {
        let (mut app, now) = app();
        app.key(ctrl_a(), now);
        app.key(press(KeyCode::Char('h')), now);
        assert_eq!(app.focus, Focus::SideMenu);
        assert_eq!(app.key(press(KeyCode::Char('j')), now), []);
        assert_eq!(app.key(press(KeyCode::Down), now), []);
        assert_eq!(app.cursor, Some(PathBuf::from("/p/web.wt/ui")));
        assert_eq!(
            open(&app),
            "/p/api.wt/fix",
            "moving the cursor opens nothing"
        );
        assert_eq!(app.key(press(KeyCode::Enter), now), []);
        assert_eq!(open(&app), "/p/web.wt/ui");
        assert_eq!(app.focus, Focus::Centre);
        app.key(ctrl_a(), now);
        app.key(press(KeyCode::Char('l')), now);
        assert_eq!(app.focus, Focus::Centre);
    }

    #[test]
    fn enter_starts_the_agent_where_none_runs() {
        let (mut app, now) = app();
        app.open(PathBuf::from("/p/api.wt/docs"));
        assert_eq!(app.key(press(KeyCode::Char('x')), now), []);
        let effects = app.key(press(KeyCode::Enter), now);
        assert!(
            matches!(&effects[..], [Effect::Start(path, _)] if path == Path::new("/p/api.wt/docs")),
            "{effects:?}"
        );
        assert_eq!(app.key(press(KeyCode::Enter), now), [], "already starting");
        let path = Path::new("/p/api.wt/docs");
        assert_eq!(
            app.started(path, Ok(()), now),
            [Effect::Attach(path.to_owned())]
        );
        app.attached(path);
        assert_eq!(
            app.key(press(KeyCode::Char('x')), now),
            [Effect::Type(b"x".to_vec())]
        );
    }

    #[test]
    fn a_failed_start_says_why() {
        let (mut app, now) = app();
        let path = PathBuf::from("/p/api.wt/docs");
        app.open(path.clone());
        app.key(press(KeyCode::Enter), now);
        app.started(&path, Err("claude is not installed".to_owned()), now);
        assert_eq!(
            app.panel.as_ref().map(|panel| &panel.status),
            Some(&Status::NoAgent)
        );
        assert_eq!(
            app.message.as_ref().map(|(text, _)| text.as_str()),
            Some("claude is not installed")
        );
    }

    #[test]
    fn an_agent_started_elsewhere_is_attached_on_the_next_read() {
        let (mut app, _) = app();
        app.open(PathBuf::from("/p/api.wt/docs"));
        let mut fleets = std::mem::take(&mut app.fleets);
        if let Ok(fleet) = fleets[0].fleet.as_mut() {
            fleet.worktrees[1].holder = HolderStatus::Alive;
        }
        assert_eq!(
            app.set_fleets(fleets),
            [Effect::Attach(PathBuf::from("/p/api.wt/docs"))]
        );
    }

    #[test]
    fn under_100_columns_the_focus_stays_in_the_centre() {
        let (mut app, now) = app();
        app.key(ctrl_a(), now);
        app.key(press(KeyCode::Char('h')), now);
        assert_eq!(app.focus, Focus::SideMenu);
        app.fit(Rect::new(0, 0, 90, 30));
        assert_eq!(app.focus, Focus::Centre);
        app.key(ctrl_a(), now);
        app.key(press(KeyCode::Char('h')), now);
        assert_eq!(app.focus, Focus::Centre);
    }

    #[test]
    fn a_paste_reaches_the_agent_bracketed_when_it_asked() {
        let (mut app, now) = app();
        assert_eq!(app.paste("a\nb"), [Effect::Type(b"a\rb".to_vec())]);
        app.key(ctrl_a(), now);
        app.key(press(KeyCode::Char('h')), now);
        assert_eq!(app.paste("a"), [], "the side menu takes no paste");
    }

    #[test]
    fn the_panel_follows_the_screens_size() {
        let (mut app, _) = app();
        assert_eq!(app.fit(SCREEN), None);
        assert_eq!(
            app.fit(Rect::new(0, 0, 160, 40)),
            Some(Size {
                rows: 35,
                cols: 119
            })
        );
    }

    #[test]
    fn the_application_opens_the_worktree_it_was_started_in() {
        let (app, _) = app();
        let mut started = App::new(SCREEN, Some(PathBuf::from("/p/web.wt/ui/src")));
        started.set_fleets(std::mem::take(&mut { app }.fleets));
        assert_eq!(open(&started), "/p/web.wt/ui");
    }
}
