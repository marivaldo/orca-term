//! App: the application's state and what each key does to it (#174, #176, #177). Nothing here
//! touches the terminal, the holder or a process: a key gives back the effects the run loop carries
//! out (bytes for the agent or a program, a holder to attach to, an agent or a program to start, a
//! size to tell, a diff to open in Neovim, quitting), so every key is tested on its own.
//!
//! Every key goes to the focused part, the side menu, the focused panel or the right column,
//! except the prefix, `Ctrl-a`, which arms the application's own keys for one keystroke (ADR
//! 0012): `h` and `l` move the focus (between the split's halves and to the right column too), `j`
//! and `k` open the next or previous worktree, `1`, `2` and `3` show the agent, Neovim or the
//! shell, `|` splits, `z` zooms, `f` shows or hides the right column, `w` opens the picker, `P` a
//! draft pull request, `?` help, `a` sends a literal `Ctrl-a`, `q` quits, and `esc` (or waiting
//! 1.5 s) disarms it. While the picker or help floats over the screen, every key goes to it.

mod failure;
mod picker;
mod programs;
mod right_column;
mod side_menu;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

pub(crate) use crate::app::state::failure::Log;
pub(crate) use crate::app::state::picker::Picker;
pub(crate) use crate::app::state::side_menu::Typing;

use crate::app::layout::{self, Layout};
use crate::app::panel::{Panel, Status};
use crate::app::program::{Kind, Program, ProgramId};
use crate::app::view::{Tab, View};
use crate::app::{keys, menu};
use crate::domain::agent::{Size, StartMode};
use crate::domain::project::ProjectFleet;
use crate::domain::worktree::WorktreeName;

pub(crate) use right_column::ChangesRead;

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
    /// The centre's focused panel, the view's tab.
    Centre,
    /// The right column, whose cursor moves over the changed files.
    RightColumn,
}

/// What the run loop must do after a key or a change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Effect {
    /// Type these bytes into the open worktree's agent.
    Type(Vec<u8>),
    /// Attach the agent's panel to the holder of the worktree at this path.
    Attach(PathBuf),
    /// Start the agent of the worktree at this path through the core, at this size, resuming its
    /// session or on a new one.
    Start(PathBuf, Size, StartMode),
    /// Tell the agent's holder the panel is now this size.
    SizeAgent(Size),
    /// Start this worktree's Neovim or shell, at this size.
    Run(ProgramId, Size),
    /// Type these bytes into this Neovim or shell.
    TypeInto(ProgramId, Vec<u8>),
    /// Tell this Neovim or shell its panel is now this size.
    Resize(ProgramId, Size),
    /// Ask the Neovims working in the worktree at `worktree`, of the repository whose primary
    /// checkout is `primary`, to open the diff of `file` (relative to the worktree).
    OpenDiff {
        primary: PathBuf,
        worktree: PathBuf,
        file: String,
    },
    /// Read the last output the agent of the worktree at this path left, for [`App::log_read`].
    ReadLog(PathBuf),
    /// Create the worktree `name` in the project at `project` through the core, then tell
    /// [`App::created`].
    NewWorktree {
        project: PathBuf,
        name: WorktreeName,
    },
    /// Open a draft pull request for the worktree at this path through the core, then tell
    /// [`App::drafted`].
    DraftPr(PathBuf),
    /// Quit the application. The agents keep running; the Neovims and shells end.
    Quit,
}

/// What floats over the screen and takes every key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Overlay {
    /// `^a w`: every worktree, filtered by what is typed.
    Picker(Picker),
    /// `^a ?`: the keys; any key closes it.
    Help,
}

/// Everything the application shows and remembers.
#[derive(Debug)]
pub(crate) struct App {
    /// Every project's fleet, as the core last read it.
    pub(crate) fleets: Vec<ProjectFleet>,
    /// The worktree whose panels the centre shows.
    pub(crate) open: Option<PathBuf>,
    /// The side menu's cursor, while it has the focus.
    pub(crate) cursor: Option<PathBuf>,
    pub(crate) focus: Focus,
    /// Which panels the centre shows, and which has the focus.
    pub(crate) view: View,
    /// When the prefix was armed, while it waits for its key.
    pub(crate) prefix: Option<Instant>,
    /// The open worktree's agent terminal.
    pub(crate) panel: Option<Panel>,
    /// Every worktree's Neovim and shell started so far, kept while the application runs.
    pub(crate) programs: BTreeMap<ProgramId, Program>,
    /// The open worktree's changes, as last read off the run loop's thread.
    pub(crate) changes: Option<ChangesRead>,
    /// The right column's cursor: the index of a changed file.
    pub(crate) file_cursor: usize,
    /// A word for the person in the status bar, and since when.
    pub(crate) message: Option<(String, Instant)>,
    /// Where everything went in the last frame.
    pub(crate) layout: Layout,
    /// The working glyph's frame.
    pub(crate) frame: usize,
    /// The picker or help, while it floats over the screen.
    pub(crate) overlay: Option<Overlay>,
    /// What the side menu's worktrees are filtered by (`/`); empty for all of them.
    pub(crate) filter: String,
    /// What the side menu is typing: its filter, or a new worktree's name.
    pub(crate) typing: Option<Typing>,
    /// The failure cards dismissed (`x`): each worktree's, by when its agent ended.
    pub(crate) dismissed: BTreeSet<(PathBuf, u64)>,
    /// The failed agent's last output, read for its card and shown as the log (`l`).
    pub(crate) log: Option<Log>,
    /// A worktree just created (its project and name), opened once a read of the fleets lists it.
    to_open: Option<(PathBuf, String)>,
    /// The screen's size, which the layout is made for.
    area: Rect,
    /// The worktree to open first, when the fleets come: the one the application was started in.
    start_in: Option<PathBuf>,
}

impl App {
    /// The application on a screen of `area`, before the fleets are read; it opens the worktree
    /// containing `start_in` first when there is one.
    pub(crate) fn new(area: Rect, start_in: Option<PathBuf>) -> Self {
        let view = View::default();
        Self {
            fleets: Vec::new(),
            open: None,
            cursor: None,
            focus: Focus::Centre,
            view,
            prefix: None,
            panel: None,
            programs: BTreeMap::new(),
            changes: None,
            file_cursor: 0,
            message: None,
            layout: layout::layout(area, view),
            frame: 0,
            overlay: None,
            filter: String::new(),
            typing: None,
            dismissed: BTreeSet::new(),
            log: None,
            to_open: None,
            area,
            start_in,
        }
    }

    /// The screen is now `area` large. Returns the sizes to tell the panels on screen whose size
    /// changed: the agent's holder, a Neovim, a shell.
    pub(crate) fn fit(&mut self, area: Rect) -> Vec<Effect> {
        self.area = area;
        self.relayout();
        let cards = self.layout.cards.clone();
        cards
            .into_iter()
            .filter_map(|card| match Kind::of(card.tab) {
                None => self.resize_agent(card.body),
                Some(kind) => self.resize_program(kind, card.body),
            })
            .collect()
    }

    /// Makes the agent's panel `body` large, telling its holder when that changed.
    fn resize_agent(&mut self, body: Rect) -> Option<Effect> {
        let panel = self.panel.as_mut()?;
        panel
            .screen
            .resize(body)
            .then(|| Effect::SizeAgent(size_of(body)))
    }

    /// Lays the screen out again for the view, and keeps the focus where it can be.
    fn relayout(&mut self) {
        self.layout = layout::layout(self.area, self.view);
        let gone = match self.focus {
            Focus::SideMenu => self.layout.side_menu.is_none(),
            Focus::RightColumn => self.layout.right.is_none(),
            Focus::Centre => false,
        };
        if gone {
            self.focus = Focus::Centre;
        }
    }

    /// The fleets the core read now. Opens a worktree when none is open (or the open one is gone),
    /// and attaches to the open one's agent when it started meanwhile.
    pub(crate) fn set_fleets(&mut self, fleets: Vec<ProjectFleet>) -> Vec<Effect> {
        let mut effects = self.read_fleets(fleets);
        effects.extend(self.open_created());
        effects.extend(self.start_shown());
        effects.extend(self.want_log());
        effects
    }

    fn read_fleets(&mut self, fleets: Vec<ProjectFleet>) -> Vec<Effect> {
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

    /// The worktree containing the directory the application started in, else the first one. The
    /// deepest one wins: a worktree may sit inside the primary checkout, which is a worktree too
    /// (ADR 0013).
    fn first_to_open(&self) -> Option<PathBuf> {
        let order = menu::order(&self.fleets);
        let started_in = self.start_in.as_deref().and_then(|dir| {
            order
                .iter()
                .filter(|path| dir.starts_with(path))
                .max_by_key(|path| path.components().count())
        });
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
        if self.open.as_ref() != Some(&path) {
            self.file_cursor = 0;
        }
        self.open = Some(path.clone());
        self.cursor = Some(path.clone());
        let status = if alive {
            Status::Attached
        } else {
            Status::NoAgent
        };
        let body = self.layout.body_of(Tab::Agent);
        self.panel = Some(Panel::new(path.clone(), status, body));
        if alive {
            vec![Effect::Attach(path)]
        } else {
            Vec::new()
        }
    }

    /// A key the person pressed at `now`.
    pub(crate) fn key(&mut self, key: KeyEvent, now: Instant) -> Vec<Effect> {
        let mut effects = self.press(key, now);
        self.relayout();
        effects.extend(self.start_shown());
        effects.extend(self.want_log());
        effects
    }

    fn press(&mut self, key: KeyEvent, now: Instant) -> Vec<Effect> {
        if let Some(overlay) = self.overlay.take() {
            return match overlay {
                Overlay::Picker(picker) => self.picker_key(picker, key),
                // Any key closes help, and does nothing else.
                Overlay::Help => Vec::new(),
            };
        }
        let armed = self
            .prefix
            .take()
            .is_some_and(|at| now.duration_since(at) < PREFIX_FOR);
        if armed {
            return self.command(key, now);
        }
        if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char(PREFIX) {
            self.prefix = Some(now);
            return Vec::new();
        }
        match self.focus {
            Focus::SideMenu => self.side_menu_key(key, now),
            Focus::Centre => self.centre_key(key),
            Focus::RightColumn => self.right_column_key(key),
        }
    }

    /// The key after the prefix.
    fn command(&mut self, key: KeyEvent, now: Instant) -> Vec<Effect> {
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => self.focus_left(),
            KeyCode::Char('l') | KeyCode::Right => self.focus_right(),
            KeyCode::Char('j') | KeyCode::Down => return self.open_step(1),
            KeyCode::Char('k') | KeyCode::Up => return self.open_step(-1),
            KeyCode::Char('1') => self.show(Tab::Agent),
            KeyCode::Char('2') => self.show(Tab::Nvim),
            KeyCode::Char('3') => self.show(Tab::Shell),
            KeyCode::Char('|' | '\\') => self.view.toggle_split(),
            KeyCode::Char('z') => {
                self.view.toggle_zoom();
                self.view.files.overlay = false;
                self.focus = Focus::Centre;
            }
            KeyCode::Char('f') => self.toggle_files(),
            KeyCode::Char('w') => self.open_picker(),
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help),
            KeyCode::Char('P') => return self.draft_pr(now),
            KeyCode::Char(PREFIX) => {
                let ctrl_a = KeyEvent::new(KeyCode::Char(PREFIX), KeyModifiers::CONTROL);
                return self.type_focused(ctrl_a);
            }
            KeyCode::Char('q') => return vec![Effect::Quit],
            _ => {}
        }
        Vec::new()
    }

    /// `^a h`: from the right column to the centre, from the split's right half to the agent, else
    /// to the side menu when it shows.
    fn focus_left(&mut self) {
        if self.focus == Focus::RightColumn {
            self.leave_right_column();
            return;
        }
        if self.focus == Focus::Centre && self.view.left() {
            return;
        }
        if self.layout.side_menu.is_some() {
            self.focus = Focus::SideMenu;
            self.cursor.clone_from(&self.open);
        }
    }

    /// `^a l`: from the side menu to the centre, from the agent to the split's right half, else to
    /// the right column when it shows.
    fn focus_right(&mut self) {
        match self.focus {
            Focus::SideMenu => self.focus = Focus::Centre,
            Focus::Centre => {
                if !self.view.right() && self.layout.right.is_some() {
                    self.focus = Focus::RightColumn;
                }
            }
            Focus::RightColumn => {}
        }
    }

    /// `^a 1`, `2`, `3`: shows `tab`, with the focus.
    fn show(&mut self, tab: Tab) {
        self.view.show(tab);
        self.view.files.overlay = false;
        self.focus = Focus::Centre;
    }

    /// Opens the worktree `by` steps after (or before) the open one in the side menu's order.
    fn open_step(&mut self, by: isize) -> Vec<Effect> {
        let order = menu::order(&self.fleets);
        match menu::step(&order, self.open.as_deref(), by) {
            Some(path) => self.open(path),
            None => Vec::new(),
        }
    }

    fn side_menu_key(&mut self, key: KeyEvent, now: Instant) -> Vec<Effect> {
        if let Some(typing) = self.typing.take() {
            return self.typing_key(typing, key, now);
        }
        let order = self.side_menu_order();
        match key.code {
            KeyCode::Char('n') => {
                self.ask_new_worktree();
                Vec::new()
            }
            KeyCode::Char('/') => {
                self.typing = Some(Typing::Filter);
                Vec::new()
            }
            KeyCode::Esc => {
                self.set_filter(String::new());
                Vec::new()
            }
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

    /// A key for the focused panel: the agent's, or a Neovim's or a shell's.
    fn centre_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match Kind::of(self.view.tab) {
            None => self.agent_key(key),
            Some(kind) => self.program_key(kind, key),
        }
    }

    /// A key for the agent's panel: every key goes to a running agent; where none runs, `enter`
    /// starts it, and a failed agent's card and log take their own keys.
    fn agent_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        if self.log_shown().is_some() {
            self.log_key(key);
            return Vec::new();
        }
        if self.failure().is_some() {
            return self.failure_key(key);
        }
        let Some(panel) = self.panel.as_mut() else {
            return Vec::new();
        };
        match &panel.status {
            Status::Attached => vec![Effect::Type(keys::encode(
                key,
                panel.screen.application_cursor(),
            ))],
            Status::NoAgent | Status::Ended(_) if key.code == KeyCode::Enter => {
                self.start_agent(StartMode::Resume)
            }
            Status::NoAgent | Status::Ended(_) | Status::Starting => Vec::new(),
        }
    }

    /// Starts the open worktree's agent through the core, on the session `mode` says, where none
    /// runs.
    fn start_agent(&mut self, mode: StartMode) -> Vec<Effect> {
        let body = self.layout.body_of(Tab::Agent);
        let Some(panel) = self.panel.as_mut() else {
            return Vec::new();
        };
        if matches!(panel.status, Status::Attached | Status::Starting) {
            return Vec::new();
        }
        panel.status = Status::Starting;
        vec![Effect::Start(panel.worktree.clone(), size_of(body), mode)]
    }

    /// `^a P`: opens a draft pull request for the open worktree through the core.
    fn draft_pr(&mut self, now: Instant) -> Vec<Effect> {
        let Some(open) = self.open.clone() else {
            return Vec::new();
        };
        self.say("opening a draft pull request…", now);
        vec![Effect::DraftPr(open)]
    }

    /// The core opened the draft pull request (`Ok` says which) or could not (`Err` says why).
    pub(crate) fn drafted(&mut self, result: Result<String, String>, now: Instant) {
        match result {
            Ok(text) | Err(text) => self.say(text, now),
        }
    }

    /// `key` typed into the focused panel, when something runs there to read it.
    fn type_focused(&mut self, key: KeyEvent) -> Vec<Effect> {
        if self.focus != Focus::Centre {
            return Vec::new();
        }
        match Kind::of(self.view.tab) {
            None => match &self.panel {
                Some(panel) if panel.status == Status::Attached => vec![Effect::Type(
                    keys::encode(key, panel.screen.application_cursor()),
                )],
                _ => Vec::new(),
            },
            Some(kind) => self.type_into(kind, |program| {
                keys::encode(key, program.screen.application_cursor())
            }),
        }
    }

    /// Text the person pasted: the focused panel's, when something runs there.
    pub(crate) fn paste(&mut self, text: &str) -> Vec<Effect> {
        if self.focus != Focus::Centre {
            return Vec::new();
        }
        match Kind::of(self.view.tab) {
            None => match &self.panel {
                Some(panel) if panel.status == Status::Attached => {
                    vec![Effect::Type(keys::paste(
                        text,
                        panel.screen.bracketed_paste(),
                    ))]
                }
                _ => Vec::new(),
            },
            Some(kind) => self.type_into(kind, |program| {
                keys::paste(text, program.screen.bracketed_paste())
            }),
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
        let body = self.layout.body_of(Tab::Agent);
        if let Some(panel) = self.panel.as_mut().filter(|panel| panel.worktree == path) {
            *panel = Panel::new(path.to_owned(), Status::Attached, body);
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

/// A terminal's size: its area's.
fn size_of(area: Rect) -> Size {
    Size {
        rows: area.height.max(1),
        cols: area.width.max(1),
    }
}

#[cfg(test)]
mod tests;
