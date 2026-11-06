//! The prototype's state and what each key does to it (README, "Interações").

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Position;
use tui_term::vt100::{MouseProtocolEncoding, MouseProtocolMode};

use crate::draw::{Geometry, Pane};
use crate::fleet::{self, FileChange, Key, Project, Worktree};
use crate::keys;
use crate::pty::{Term, Wake};
use crate::theme::State;

/// What reaches the main loop: something the person did, or a terminal with something to draw.
pub enum Msg {
    Input(Event),
    Wake,
}

/// Where the keys go: the side menu, a terminal of the centre, or the right card.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Sidebar,
    Centre,
    Right,
}

/// The terminals every worktree owns, in the order of their keys `1` `2` `3`.
pub const TERMINALS: [&str; 3] = ["agent", "nvim", "shell"];
pub const AGENT: usize = 0;
pub const NVIM: usize = 1;
pub const SHELL: usize = 2;

/// How long the prefix waits for its key.
pub const PREFIX_FOR: Duration = Duration::from_millis(1500);
/// How long a message stays in the status bar.
pub const MESSAGE_FOR: Duration = Duration::from_secs(6);

pub struct App {
    pub fleet: Vec<Project>,
    /// Every worktree in the side menu's order, for `^a j/k` and the picker.
    pub order: Vec<Key>,
    /// The worktree the centre shows.
    pub open: Key,
    /// The side menu's cursor, while it has the focus.
    pub cursor: Key,
    pub focus: Focus,
    /// Which terminal of each worktree has the focus (or is shown): an index into `TERMINALS`.
    current: HashMap<Key, usize>,
    /// The right half of the split: nvim or the shell.
    side: HashMap<Key, usize>,
    terms: HashMap<Key, [Option<Term>; 3]>,
    pub split: bool,
    pub zoom: bool,
    /// When the prefix was pressed, while it waits for its key.
    pub pending: Option<Instant>,
    /// The floating worktree picker, with its cursor (an index into `order`).
    pub picker: Option<usize>,
    /// The right card as a column (150 columns and more), and on top of the centre (narrower).
    pub right_column: bool,
    pub right_overlay: bool,
    pub help: bool,
    /// The changed files of the right card (real ones, from git), and the one selected.
    pub files: Vec<FileChange>,
    pub file: usize,
    /// The file last opened in each worktree's nvim, for the split's header.
    pub opened: HashMap<Key, String>,
    /// The prefix's letter (`a` is Ctrl-a).
    pub prefix: char,
    pub message: Option<(String, Instant)>,
    /// Turns the working glyph (120 ms a frame).
    pub tick: usize,
    /// The command of each of the three terminals.
    pub commands: [Vec<String>; 3],
    cwd: PathBuf,
    /// Without it (snapshot mode) no process is ever started, and the panes show placeholders.
    wake: Option<Wake>,
    /// Where everything was drawn last, for the mouse and for the size of a fresh terminal.
    pub geometry: Option<Geometry>,
    /// The screen's width, which decides the layout.
    pub width: u16,
    pub quit: bool,
}

fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join(name).is_file()))
}

fn env_command(name: &str) -> Option<Vec<String>> {
    let value = std::env::var(name).ok()?;
    let argv: Vec<String> = value.split_whitespace().map(str::to_owned).collect();
    (!argv.is_empty()).then_some(argv)
}

/// The changed files of the start directory, relative to it: the last five commits' worth, or a
/// few tracked files when that gives nothing. Real paths, so opening one works.
fn changed_files(cwd: &Path) -> Vec<FileChange> {
    let git = |args: &[&str]| -> Vec<String> {
        Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .ok()
            .filter(|output| output.status.success())
            .map(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default()
    };
    let base = ["diff", "--relative", "--no-renames", "HEAD~5"];
    let kinds: HashMap<String, char> = git(&[&base[..], &["--name-status"]].concat())
        .iter()
        .filter_map(|line| {
            let (kind, path) = line.split_once('\t')?;
            Some((path.to_owned(), kind.chars().next()?))
        })
        .collect();
    let mut files: Vec<FileChange> = git(&[&base[..], &["--numstat"]].concat())
        .iter()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let add = parts.next()?.parse().unwrap_or(0);
            let del = parts.next()?.parse().unwrap_or(0);
            let path = parts.next()?.to_owned();
            let kind = match kinds.get(&path) {
                Some('A') => 'A',
                Some('D') => 'D',
                _ => 'M',
            };
            Some(FileChange {
                kind,
                path,
                add,
                del,
            })
        })
        .collect();
    if files.is_empty() {
        files = git(&["ls-files"])
            .into_iter()
            .take(8)
            .map(|path| FileChange {
                kind: 'M',
                path,
                add: 0,
                del: 0,
            })
            .collect();
    }
    files
}

impl App {
    /// The live prototype (`wake` given) or a snapshot (`None`: nothing starts, placeholders).
    pub fn new(wake: Option<Wake>) -> App {
        let shell = std::env::var("SHELL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "/bin/sh".to_owned());
        let agent = env_command("ORCA_PROTO_AGENT_CMD").unwrap_or_else(|| {
            if on_path("claude") {
                vec!["claude".to_owned()]
            } else {
                vec![shell.clone()]
            }
        });
        let nvim = env_command("ORCA_PROTO_EDITOR_CMD").unwrap_or_else(|| vec!["nvim".to_owned()]);
        let prefix = std::env::var("ORCA_PROTO_PREFIX")
            .ok()
            .and_then(|value| value.chars().next())
            .filter(char::is_ascii_alphabetic)
            .map_or('a', |c| c.to_ascii_lowercase());
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let fleet = fleet::fake();
        let order = fleet::order(&fleet);
        let files = if wake.is_some() {
            changed_files(&cwd)
        } else {
            Vec::new()
        };
        let mut app = App {
            fleet,
            order,
            open: (0, 1),
            cursor: (0, 1),
            focus: Focus::Centre,
            current: HashMap::new(),
            side: HashMap::new(),
            terms: HashMap::new(),
            split: false,
            zoom: false,
            pending: None,
            picker: None,
            right_column: true,
            right_overlay: false,
            help: false,
            files,
            file: 0,
            opened: HashMap::new(),
            prefix,
            message: None,
            tick: 0,
            commands: [agent, nvim, vec![shell]],
            cwd,
            wake,
            geometry: None,
            width: 180,
            quit: false,
        };
        app.open_worktree((0, 1));
        app
    }

    /// Snapshot mode: no process, placeholders in the panes.
    pub fn mock(&self) -> bool {
        self.wake.is_none()
    }

    pub fn worktree(&self, key: Key) -> &Worktree {
        &self.fleet[key.0].worktrees[key.1]
    }

    fn worktree_mut(&mut self, key: Key) -> &mut Worktree {
        &mut self.fleet[key.0].worktrees[key.1]
    }

    /// The open worktree's changed files: the real ones live, the design's in a snapshot.
    pub fn open_files(&self) -> Vec<FileChange> {
        if self.mock() {
            fleet::mock_files(self.open_wt().name)
        } else {
            self.files.clone()
        }
    }

    pub fn open_wt(&self) -> &Worktree {
        self.worktree(self.open)
    }

    /// `project/worktree`.
    pub fn location(&self, key: Key) -> String {
        format!("{}/{}", self.fleet[key.0].name, self.worktree(key).name)
    }

    /// The terminal of the open worktree that has the focus, or is shown.
    pub fn current(&self) -> usize {
        self.current.get(&self.open).copied().unwrap_or(AGENT)
    }

    /// The terminal of the split's right half.
    pub fn side(&self) -> usize {
        self.side.get(&self.open).copied().unwrap_or(NVIM)
    }

    pub fn term(&self, which: usize) -> Option<&Term> {
        self.terms.get(&self.open)?.get(which)?.as_ref()
    }

    fn term_mut(&mut self, which: usize) -> Option<&mut Term> {
        self.terms.get_mut(&self.open)?.get_mut(which)?.as_mut()
    }

    pub fn say(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), Instant::now()));
    }

    /// The side menu is a column from 100 columns up; narrower it is reached through `^a w`.
    pub fn sidebar_shown(&self) -> bool {
        self.width >= 100 && !self.zoom
    }

    /// The right card on screen: a column from 150 columns up, on top of the centre narrower.
    pub fn right_shown(&self) -> bool {
        self.right_as_column() || self.right_overlay
    }

    pub fn right_as_column(&self) -> bool {
        self.width >= 150 && self.right_column && !self.zoom
    }

    /// Drops the prefix once it has waited long enough, and an old message.
    pub fn expire(&mut self) {
        if self.pending.is_some_and(|at| at.elapsed() >= PREFIX_FOR) {
            self.pending = None;
        }
        if self
            .message
            .as_ref()
            .is_some_and(|(_, at)| at.elapsed() >= MESSAGE_FOR)
        {
            self.message = None;
        }
    }

    /// Whether anything on screen moves by itself: the spinner, the prefix's timeout.
    pub fn animated(&self) -> bool {
        self.pending.is_some()
            || self
                .fleet
                .iter()
                .flat_map(|p| &p.worktrees)
                .any(|w| w.state == State::Working)
    }

    /// The size a fresh terminal starts at: the first pane drawn; the next frame corrects it.
    fn fresh_size(&self) -> (u16, u16) {
        self.geometry
            .as_ref()
            .and_then(|geometry| geometry.panes.first())
            .map_or((30, 100), |pane| (pane.body.height, pane.body.width))
    }

    fn spawn(&self, which: usize) -> Option<Term> {
        let wake = self.wake.as_ref()?;
        let (rows, cols) = self.fresh_size();
        Some(Term::spawn(
            &self.commands[which],
            &self.cwd,
            rows,
            cols,
            wake,
        ))
    }

    /// A worktree's terminals, started the first time it is opened and kept from then on. The
    /// agent only where one runs and has not stopped.
    fn start_terminals(&mut self, key: Key) {
        if self.wake.is_none() || self.terms.contains_key(&key) {
            return;
        }
        let worktree = self.worktree(key);
        let agent = (worktree.agent.is_some() && !worktree.failure)
            .then(|| self.spawn(AGENT))
            .flatten();
        let terms = [agent, self.spawn(NVIM), self.spawn(SHELL)];
        self.terms.insert(key, terms);
    }

    /// Starts the open worktree's agent: Enter on a worktree with none, `r`/`R` after a failure.
    fn start_agent(&mut self) {
        let key = self.open;
        let term = self.spawn(AGENT);
        let worktree = self.worktree_mut(key);
        worktree.failure = false;
        if worktree.agent.is_none() {
            worktree.agent = Some("claude");
        }
        worktree.state = State::Idle;
        worktree.last_message = "agent started";
        worktree.age = "now";
        if let Some(terms) = self.terms.get_mut(&key) {
            terms[AGENT] = term;
        }
    }

    /// Gives every terminal on screen the size of its pane.
    pub fn fit_terminals(&mut self, geometry: &Geometry) {
        for pane in &geometry.panes {
            if let Some(term) = self.term_mut(pane.which) {
                term.resize(pane.body.height, pane.body.width);
            }
        }
    }

    pub fn kill_all(&mut self) {
        for term in self.terms.values_mut().flatten().flatten() {
            term.hang_up();
        }
        for term in self.terms.values_mut().flatten().flatten() {
            term.kill();
        }
    }

    /// Opens a worktree in the centre: it has been seen, and its terminals start.
    pub fn open_worktree(&mut self, key: Key) {
        self.open = key;
        self.cursor = key;
        self.worktree_mut(key).unseen = false;
        self.start_terminals(key);
    }

    /// The next (1) or previous (-1) worktree in the side menu's order.
    fn step(&self, from: Key, step: isize) -> Key {
        let at = self.order.iter().position(|k| *k == from).unwrap_or(0);
        let n = self.order.len() as isize;
        self.order[(at as isize + step).rem_euclid(n) as usize]
    }

    pub fn focus_on(&mut self, focus: Focus) {
        if focus == Focus::Sidebar {
            self.cursor = self.open;
        }
        if focus != Focus::Right {
            self.right_overlay = false;
        }
        self.focus = focus;
    }

    /// Shows one of the open worktree's terminals, with the focus.
    pub fn show_terminal(&mut self, which: usize) {
        self.current.insert(self.open, which);
        if which != AGENT {
            self.side.insert(self.open, which);
        }
        self.focus_on(Focus::Centre);
    }

    /// `^a h` (-1) and `^a l` (1): side menu, centre, right card; in the split agent and nvim;
    /// in zoom only between the split's two terminals.
    fn move_focus(&mut self, step: isize) {
        match self.focus {
            Focus::Sidebar if step > 0 => {
                self.show_terminal(if self.split { AGENT } else { self.current() });
            }
            Focus::Right if step < 0 => {
                let which = if self.split {
                    self.side()
                } else {
                    self.current()
                };
                self.show_terminal(which);
            }
            Focus::Centre => {
                let left = self.split && self.current() == AGENT;
                let right = self.split && self.current() != AGENT;
                if step < 0 && right {
                    self.show_terminal(AGENT);
                } else if step > 0 && left {
                    self.show_terminal(self.side());
                } else if self.zoom {
                    // Zoomed, the focus stays in the centre.
                } else if step < 0 && self.sidebar_shown() {
                    self.focus_on(Focus::Sidebar);
                } else if step > 0 && self.right_shown() {
                    self.focus_on(Focus::Right);
                }
            }
            _ => {}
        }
    }

    /// `^a f`: the right card's column on and off, or, narrower, the right card on top.
    fn toggle_files(&mut self) {
        if self.width >= 150 && !self.zoom {
            self.right_column = !self.right_column;
            if !self.right_column && self.focus == Focus::Right {
                self.show_terminal(self.current());
            }
        } else if self.right_overlay {
            self.right_overlay = false;
            if self.focus == Focus::Right {
                self.show_terminal(self.current());
            }
        } else {
            self.right_overlay = true;
            self.focus = Focus::Right;
        }
    }

    /// Opens the selected changed file in the worktree's nvim, crudely: by typing `:e`.
    fn open_file(&mut self) {
        let Some(file) = self.files.get(self.file).cloned() else {
            return;
        };
        if file.kind == 'D' {
            self.say(format!("{} was deleted: nothing to open", file.path));
            return;
        }
        self.right_overlay = false;
        self.show_terminal(NVIM);
        if let Some(term) = self.term(NVIM) {
            term.write(format!("\x1b:e {}\r", file.path).as_bytes());
        }
        self.opened.insert(self.open, file.path.clone());
        self.say(format!(
            "typed `:e {}` into nvim (the real application opens the file's diff)",
            file.path
        ));
    }

    pub fn on_event(&mut self, event: Event) {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => self.on_key(key),
            Event::Paste(text) => self.on_paste(&text),
            Event::Mouse(mouse) => self.on_mouse(mouse),
            _ => {}
        }
    }

    fn is_prefix(&self, key: KeyEvent) -> bool {
        key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char(self.prefix)
    }

    fn on_key(&mut self, key: KeyEvent) {
        if self.help {
            self.help = false;
            return;
        }
        if self.pending.take().is_some() {
            self.command(key);
            return;
        }
        if self.is_prefix(key) {
            self.pending = Some(Instant::now());
            return;
        }
        if self.picker.is_some() {
            self.picker_key(key);
            return;
        }
        match self.focus {
            Focus::Sidebar => self.sidebar_key(key),
            Focus::Right => self.right_key(key),
            Focus::Centre => self.centre_key(key),
        }
    }

    /// The key after the prefix.
    fn command(&mut self, key: KeyEvent) {
        if self.is_prefix(key) {
            self.send_prefix();
            return;
        }
        match key.code {
            KeyCode::Esc => {}
            KeyCode::Char('h') | KeyCode::Left => self.move_focus(-1),
            KeyCode::Char('l') | KeyCode::Right => self.move_focus(1),
            KeyCode::Char('j') | KeyCode::Down => self.open_worktree(self.step(self.open, 1)),
            KeyCode::Char('k') | KeyCode::Up => self.open_worktree(self.step(self.open, -1)),
            KeyCode::Char('1') => self.show_terminal(AGENT),
            KeyCode::Char('2') => self.show_terminal(NVIM),
            KeyCode::Char('3') => self.show_terminal(SHELL),
            KeyCode::Char('z') => {
                self.zoom = !self.zoom;
                self.right_overlay = false;
                if self.focus != Focus::Centre {
                    self.show_terminal(self.current());
                }
            }
            KeyCode::Char('|' | '\\') => self.split = !self.split,
            KeyCode::Char('w') => {
                self.picker = match self.picker {
                    Some(_) => None,
                    None => self.order.iter().position(|k| *k == self.open),
                };
            }
            KeyCode::Char('f') => self.toggle_files(),
            KeyCode::Char('P') => {
                let at = self.location(self.open);
                self.say(format!(
                    "^{} P: the real application opens a draft pull request for {at}",
                    self.prefix
                ));
            }
            KeyCode::Char('a') => self.send_prefix(),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('q') => self.quit = true,
            _ => {}
        }
    }

    /// `^a a`: the prefix's control key itself, for the terminal with the focus.
    fn send_prefix(&mut self) {
        if self.focus == Focus::Centre {
            let key = KeyEvent::new(KeyCode::Char(self.prefix), KeyModifiers::CONTROL);
            self.type_key(key);
        }
    }

    fn sidebar_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.cursor = self.step(self.cursor, 1),
            KeyCode::Char('k') | KeyCode::Up => self.cursor = self.step(self.cursor, -1),
            KeyCode::Enter => {
                self.open_worktree(self.cursor);
                self.show_terminal(self.current());
            }
            KeyCode::Char('n') => {
                let project = self.fleet[self.cursor.0].name;
                self.say(format!(
                    "n: the real application creates a new worktree in {project}"
                ));
            }
            KeyCode::Char('/') => self.say("/: the real application filters the list"),
            _ => {}
        }
    }

    fn right_key(&mut self, key: KeyEvent) {
        let last = self.files.len().saturating_sub(1);
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.file = (self.file + 1).min(last),
            KeyCode::Char('k') | KeyCode::Up => self.file = self.file.saturating_sub(1),
            KeyCode::Enter => self.open_file(),
            KeyCode::Esc if self.right_overlay => {
                self.right_overlay = false;
                self.show_terminal(self.current());
            }
            _ => {}
        }
    }

    fn picker_key(&mut self, key: KeyEvent) {
        let Some(at) = self.picker else {
            return;
        };
        let n = self.order.len();
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.picker = Some((at + 1) % n),
            KeyCode::Char('k') | KeyCode::Up => self.picker = Some((at + n - 1) % n),
            KeyCode::Enter => {
                self.picker = None;
                self.open_worktree(self.order[at]);
                self.show_terminal(self.current());
            }
            KeyCode::Esc => self.picker = None,
            _ => {}
        }
    }

    /// A key for the centre: the failure card's actions, Enter where no agent runs, everything
    /// else to the terminal with the focus.
    fn centre_key(&mut self, key: KeyEvent) {
        let which = self.current();
        if which == AGENT && self.open_wt().failure {
            match key.code {
                KeyCode::Char('r') => {
                    self.start_agent();
                    self.say("r: resumed (the real application resumes the session)");
                }
                KeyCode::Char('R') => {
                    self.start_agent();
                    self.say("R: restarted clean");
                }
                KeyCode::Char('l') => self.say("l: the real application shows the log"),
                KeyCode::Char('x') => self.worktree_mut(self.open).failure = false,
                _ => {}
            }
            return;
        }
        if which == AGENT && self.term(AGENT).is_none() {
            if key.code == KeyCode::Enter {
                self.start_agent();
            }
            return;
        }
        self.type_key(key);
    }

    /// A key for the terminal that has the focus. One whose child has exited only takes Enter,
    /// which starts the child again.
    fn type_key(&mut self, key: KeyEvent) {
        let which = self.current();
        let Some(term) = self.term_mut(which) else {
            return;
        };
        if term.exited() {
            if key.code == KeyCode::Enter {
                term.respawn();
            }
            return;
        }
        let application_cursor = term.parser().screen().application_cursor();
        term.scroll_to_bottom();
        term.write(&keys::encode_key(key, application_cursor));
    }

    /// Pasted text, wrapped in the bracketed-paste marks only when the child asked for them.
    fn on_paste(&mut self, text: &str) {
        if self.focus != Focus::Centre || self.picker.is_some() {
            return;
        }
        let Some(term) = self.term(self.current()) else {
            return;
        };
        if term.exited() {
            return;
        }
        if term.parser().screen().bracketed_paste() {
            term.write(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        } else {
            term.write(text.replace('\n', "\r").as_bytes());
        }
    }

    fn on_mouse(&mut self, mouse: MouseEvent) {
        let Some(geometry) = self.geometry.clone() else {
            return;
        };
        if self.help || self.picker.is_some() {
            return;
        }
        let at = Position::new(mouse.column, mouse.row);
        let on_right = geometry.right.is_some_and(|r| r.contains(at));
        let on_sidebar = geometry.sidebar.is_some_and(|r| r.contains(at));
        // The right card on top of the centre hides the pane under it.
        let pane = geometry
            .panes
            .iter()
            .find(|pane| !on_right && pane.card.contains(at))
            .copied();

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(pane) = pane {
                    let had_focus = self.focus == Focus::Centre && self.current() == pane.which;
                    let tab = geometry
                        .tabs
                        .iter()
                        .find(|(rect, _)| rect.contains(at))
                        .map(|(_, which)| *which);
                    self.show_terminal(tab.unwrap_or(pane.which));
                    if had_focus && tab.is_none() {
                        self.forward_mouse(&pane, mouse);
                    }
                } else if on_sidebar {
                    if let Some((_, _, key)) = geometry
                        .items
                        .iter()
                        .find(|(top, bottom, _)| at.y >= *top && at.y < *bottom)
                    {
                        self.open_worktree(*key);
                    }
                    self.focus_on(Focus::Sidebar);
                } else if on_right {
                    if let Some((_, index)) = geometry.files.iter().find(|(y, _)| *y == at.y) {
                        self.file = *index;
                    }
                    self.focus = Focus::Right;
                }
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                let up = mouse.kind == MouseEventKind::ScrollUp;
                if let Some(pane) = pane {
                    // The wheel belongs to the child when it asked for the mouse; otherwise it
                    // moves over the lines above the screen.
                    if !self.forward_mouse(&pane, mouse)
                        && let Some(term) = self.term(pane.which)
                    {
                        term.scroll(if up { 3 } else { -3 });
                    }
                } else if on_sidebar {
                    self.cursor = self.step(self.cursor, if up { -1 } else { 1 });
                } else if on_right {
                    let last = self.files.len().saturating_sub(1);
                    self.file = if up {
                        self.file.saturating_sub(1)
                    } else {
                        (self.file + 1).min(last)
                    };
                }
            }
            _ => {
                if let Some(pane) = pane
                    && self.focus == Focus::Centre
                    && self.current() == pane.which
                {
                    self.forward_mouse(&pane, mouse);
                }
            }
        }
    }

    /// Passes a mouse event on to the child of a pane, when the child turned mouse reporting on
    /// and asked for this kind of event. Says whether the child has the mouse at all.
    fn forward_mouse(&self, pane: &Pane, mouse: MouseEvent) -> bool {
        let Some(term) = self.term(pane.which) else {
            return false;
        };
        let (mode, encoding) = {
            let parser = term.parser();
            let screen = parser.screen();
            (
                screen.mouse_protocol_mode(),
                screen.mouse_protocol_encoding(),
            )
        };
        if mode == MouseProtocolMode::None || term.exited() {
            return false;
        }
        let at = Position::new(mouse.column, mouse.row);
        if !pane.body.contains(at) {
            return true;
        }
        let wanted = match mouse.kind {
            MouseEventKind::Down(_)
            | MouseEventKind::ScrollUp
            | MouseEventKind::ScrollDown
            | MouseEventKind::ScrollLeft
            | MouseEventKind::ScrollRight => true,
            MouseEventKind::Up(_) => mode != MouseProtocolMode::Press,
            MouseEventKind::Drag(_) => matches!(
                mode,
                MouseProtocolMode::ButtonMotion | MouseProtocolMode::AnyMotion
            ),
            MouseEventKind::Moved => mode == MouseProtocolMode::AnyMotion,
        };
        if wanted
            && let Some(bytes) = keys::encode_mouse(
                mouse.kind,
                mouse.modifiers,
                mouse.column - pane.body.x,
                mouse.row - pane.body.y,
                encoding == MouseProtocolEncoding::Sgr,
            )
        {
            term.write(&bytes);
        }
        true
    }
}
