//! The prototype's state and what each key does to it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Position;
use tui_term::vt100::{MouseProtocolEncoding, MouseProtocolMode};

use crate::draw::{self, Geometry, Pane};
use crate::fleet::{self, Project, Stop};
use crate::keys;
use crate::pty::{Term, Wake};

/// What reaches the main loop: something the person did, or a terminal with something to draw.
pub enum Msg {
    Input(Event),
    Wake,
}

/// The three columns the focus moves between.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Focus {
    Menu,
    Centre,
    Right,
}

impl Focus {
    pub fn word(self) -> &'static str {
        match self {
            Focus::Menu => "menu",
            Focus::Centre => "centre",
            Focus::Right => "right",
        }
    }
}

/// The three layouts under judgement.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Variant {
    A,
    B,
    C,
}

impl Variant {
    pub fn label(self) -> &'static str {
        match self {
            Variant::A => "A Orca",
            Variant::B => "B Panes",
            Variant::C => "C Focus",
        }
    }

    fn next(self) -> Variant {
        match self {
            Variant::A => Variant::B,
            Variant::B => Variant::C,
            Variant::C => Variant::A,
        }
    }
}

/// The terminals every worktree owns, in the order of their keys `1` `2` `3`.
pub const TERMINALS: [&str; 3] = ["agent", "nvim", "shell"];
pub const AGENT: usize = 0;
pub const NVIM: usize = 1;
pub const SHELL: usize = 2;

pub struct App {
    pub fleet: Vec<Project>,
    pub stops: Vec<Stop>,
    /// An index into `stops`.
    pub selected: usize,
    pub focus: Focus,
    pub variant: Variant,
    /// Variants A and C: agent and nvim side by side.
    pub split: bool,
    pub zoom: bool,
    /// The side menu and the right column as columns (the right one only in variant A).
    pub show_menu: bool,
    pub show_right: bool,
    /// The side menu or the right column drawn on top of the centre (variant C, and the right
    /// column of variant B).
    pub overlay: Option<Focus>,
    pub help: bool,
    /// Which of its three terminals each worktree shows, or has the focus on.
    current: HashMap<(usize, usize), usize>,
    terms: HashMap<(usize, usize), Vec<Term>>,
    /// The changed files the right column lists, and the one selected.
    pub files: Vec<String>,
    pub file: usize,
    /// The prefix's letter (`a` is Ctrl-a) and whether it was just pressed.
    pub prefix: char,
    pub pending: bool,
    pub message: Option<(String, Instant)>,
    /// Turns the working glyph.
    pub tick: usize,
    /// The command of each of the three terminals.
    pub commands: [Vec<String>; 3],
    cwd: PathBuf,
    /// Without it (snapshot mode) no process is ever started.
    wake: Option<Wake>,
    /// Where everything was drawn last, for the mouse and for the size of a fresh terminal.
    pub geometry: Option<Geometry>,
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
fn changed_files(cwd: &Path) -> Vec<String> {
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
    let mut files = git(&[
        "diff",
        "--name-only",
        "--relative",
        "--diff-filter=d",
        "HEAD~5",
    ]);
    if files.is_empty() {
        files = git(&["ls-files"]);
        files.truncate(8);
    }
    files
}

impl App {
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
        let stops = fleet::stops(&fleet);
        let files = changed_files(&cwd);
        let mut app = App {
            fleet,
            stops,
            // The first worktree, under the first project's header.
            selected: 1,
            focus: Focus::Menu,
            variant: Variant::A,
            split: false,
            zoom: false,
            show_menu: true,
            show_right: true,
            overlay: None,
            help: false,
            current: HashMap::new(),
            terms: HashMap::new(),
            files,
            file: 0,
            prefix,
            pending: false,
            message: None,
            tick: 0,
            commands: [agent, nvim, vec![shell]],
            cwd,
            wake,
            geometry: None,
            quit: false,
        };
        app.start_terminals();
        app
    }

    /// The prefix as it is written in the hints: `^a`.
    pub fn prefix_label(&self) -> String {
        format!("^{}", self.prefix)
    }

    pub fn worktree(&self) -> Option<(usize, usize)> {
        match self.stops[self.selected] {
            Stop::Worktree(p, w) => Some((p, w)),
            Stop::Header(_) => None,
        }
    }

    /// The terminal the selected worktree shows, or has the focus on: an index into `TERMINALS`.
    pub fn current(&self) -> usize {
        self.worktree()
            .and_then(|key| self.current.get(&key).copied())
            .unwrap_or(AGENT)
    }

    pub fn term(&self, which: usize) -> Option<&Term> {
        self.terms.get(&self.worktree()?)?.get(which)
    }

    fn term_mut(&mut self, which: usize) -> Option<&mut Term> {
        let key = self.worktree()?;
        self.terms.get_mut(&key)?.get_mut(which)
    }

    /// Whether the side menu or the right column is on screen, as a column or on top.
    pub fn shows(&self, column: Focus) -> bool {
        if self.overlay == Some(column) {
            return true;
        }
        match (column, self.variant) {
            (Focus::Menu, Variant::A | Variant::B) => self.show_menu,
            (Focus::Right, Variant::A) => self.show_right,
            (Focus::Centre, _) => true,
            _ => false,
        }
    }

    pub fn say(&mut self, text: impl Into<String>) {
        self.message = Some((text.into(), Instant::now()));
    }

    /// The selected worktree's three terminals, started the first time it is selected and kept
    /// from then on.
    fn start_terminals(&mut self) {
        let (Some(key), Some(wake)) = (self.worktree(), self.wake.clone()) else {
            return;
        };
        if self.terms.contains_key(&key) {
            return;
        }
        // A fresh terminal takes the size of the first panel drawn; the next frame corrects it.
        let (rows, cols) = self
            .geometry
            .as_ref()
            .and_then(|geometry| geometry.panes.first())
            .map_or((30, 100), |pane| (pane.body.height, pane.body.width));
        let terms = self
            .commands
            .iter()
            .map(|argv| Term::spawn(argv, &self.cwd, rows, cols, &wake))
            .collect();
        self.terms.insert(key, terms);
    }

    /// Gives every terminal on screen the size of its panel.
    pub fn fit_terminals(&mut self, geometry: &Geometry) {
        for pane in &geometry.panes {
            if let Some(term) = self.term_mut(pane.which) {
                term.resize(pane.body.height, pane.body.width);
            }
        }
    }

    pub fn kill_all(&mut self) {
        for term in self.terms.values_mut().flatten() {
            term.hang_up();
        }
        for term in self.terms.values_mut().flatten() {
            term.kill();
        }
    }

    /// Puts the focus on a column, bringing it on screen first when the layout hides it.
    pub fn focus_on(&mut self, column: Focus) {
        if column == Focus::Centre {
            self.overlay = None;
        } else if !self.shows(column) {
            match (column, self.variant) {
                (Focus::Menu, Variant::A | Variant::B) => self.show_menu = true,
                (Focus::Right, Variant::A) => self.show_right = true,
                _ => self.overlay = Some(column),
            }
        }
        self.focus = column;
        self.see();
    }

    /// A worktree whose terminal has the focus has been seen.
    fn see(&mut self) {
        if self.focus == Focus::Centre
            && let Some((p, w)) = self.worktree()
        {
            self.fleet[p].worktrees[w].unseen = false;
        }
    }

    fn select(&mut self, step: isize) {
        let last = self.stops.len() - 1;
        self.selected = self.selected.saturating_add_signed(step).min(last);
        self.start_terminals();
        self.see();
    }

    fn show_terminal(&mut self, which: usize) {
        if let Some(key) = self.worktree() {
            self.current.insert(key, which);
        }
        if which == SHELL && self.variant != Variant::B {
            // The split holds the agent and nvim; the shell is shown alone.
            self.split = false;
        }
        self.focus_on(Focus::Centre);
    }

    /// The terminals of the centre from left to right, as `h` and `l` walk them.
    fn centre_order(&self) -> Vec<usize> {
        let current = self.current();
        match self.variant {
            Variant::B => vec![if current == SHELL { SHELL } else { AGENT }, NVIM],
            _ if self.split => vec![AGENT, NVIM],
            _ => vec![current],
        }
    }

    /// Moves the focus one column to the left (-1) or to the right (1). At the edge of the
    /// centre it brings in the side menu or the right column, even when the layout hides it.
    fn move_focus(&mut self, step: isize) {
        if let Some(over) = self.overlay {
            // A column on top closes towards the centre and has nothing on its other side.
            let towards_centre = if over == Focus::Menu { 1 } else { -1 };
            if step == towards_centre {
                self.focus_on(Focus::Centre);
            }
            return;
        }
        let order = self.centre_order();
        match self.focus {
            Focus::Menu if step > 0 => self.show_terminal(order[0]),
            Focus::Right if step < 0 => self.show_terminal(order[order.len() - 1]),
            Focus::Centre => {
                let at = order
                    .iter()
                    .position(|which| *which == self.current())
                    .unwrap_or(0);
                match at.checked_add_signed(step) {
                    None => self.focus_on(Focus::Menu),
                    Some(next) if next >= order.len() => self.focus_on(Focus::Right),
                    Some(next) => self.show_terminal(order[next]),
                }
            }
            _ => {}
        }
    }

    /// Shows or hides the side menu or the right column: a column where the variant has one, on
    /// top of the centre where it has not.
    fn toggle(&mut self, column: Focus) {
        if self.shows(column) {
            self.overlay = None;
            match column {
                Focus::Menu => self.show_menu = false,
                _ if self.variant == Variant::A => self.show_right = false,
                _ => {}
            }
            if self.focus == column || self.variant == Variant::C {
                self.focus_on(Focus::Centre);
            }
        } else if self.variant == Variant::C || (column, self.variant) == (Focus::Right, Variant::B)
        {
            // On top of the centre, it takes the focus until it closes.
            self.overlay = None;
            self.focus_on(column);
        } else {
            match column {
                Focus::Menu => self.show_menu = true,
                _ => self.show_right = true,
            }
        }
    }

    pub fn set_variant(&mut self, variant: Variant) {
        self.variant = variant;
        self.overlay = None;
        if variant != Variant::C {
            self.show_menu = true;
        }
        if !self.shows(self.focus) {
            self.focus = Focus::Centre;
        }
        self.see();
    }

    /// Opens the selected changed file in the worktree's nvim, crudely: by typing `:e`.
    fn open_file(&mut self) {
        let Some(path) = self.files.get(self.file).cloned() else {
            return;
        };
        if self.worktree().is_none() {
            return;
        }
        self.show_terminal(NVIM);
        if let Some(term) = self.term(NVIM) {
            term.write(format!("\x1b:e {path}\r").as_bytes());
        }
        self.say(format!(
            "typed `:e {path}` into nvim (the real application would open the file's diff)"
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
        if self.pending {
            self.pending = false;
            if self.is_prefix(key) {
                // The prefix twice is the control key itself, for the terminal.
                if self.focus == Focus::Centre {
                    self.type_key(key);
                }
            } else {
                self.command(key);
            }
            return;
        }
        if self.is_prefix(key) {
            self.pending = true;
            return;
        }
        match self.focus {
            Focus::Menu => self.menu_key(key),
            Focus::Right => self.right_key(key),
            Focus::Centre => self.type_key(key),
        }
    }

    /// The key after the prefix.
    fn command(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => self.move_focus(-1),
            KeyCode::Char('l') | KeyCode::Right => self.move_focus(1),
            KeyCode::Char('j') | KeyCode::Down => self.select(1),
            KeyCode::Char('k') | KeyCode::Up => self.select(-1),
            KeyCode::Char('1') => self.show_terminal(AGENT),
            KeyCode::Char('2') => self.show_terminal(NVIM),
            KeyCode::Char('3') => self.show_terminal(SHELL),
            KeyCode::Char('n') => self.show_terminal((self.current() + 1) % TERMINALS.len()),
            KeyCode::Char('p') => {
                self.show_terminal((self.current() + TERMINALS.len() - 1) % TERMINALS.len());
            }
            KeyCode::Char('z') => self.zoom = !self.zoom,
            KeyCode::Char('|' | '\\') => {
                if self.variant == Variant::B {
                    self.say("variant B is always split");
                } else {
                    self.split = !self.split;
                    if self.split && self.current() == SHELL {
                        self.show_terminal(AGENT);
                    }
                }
            }
            KeyCode::Char('s') => self.toggle(Focus::Menu),
            KeyCode::Char('r') => self.toggle(Focus::Right),
            KeyCode::Char('v') => self.set_variant(self.variant.next()),
            KeyCode::Char('?') => self.help = true,
            KeyCode::Char('q') => self.quit = true,
            _ => {}
        }
    }

    fn menu_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => self.select(1),
            KeyCode::Char('k') | KeyCode::Up => self.select(-1),
            KeyCode::Enter | KeyCode::Char('l') | KeyCode::Right => self.focus_on(Focus::Centre),
            KeyCode::Esc if self.overlay.is_some() => self.focus_on(Focus::Centre),
            KeyCode::Char('q') => self.quit = true,
            _ => {}
        }
    }

    fn right_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.file = (self.file + 1).min(self.files.len().saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => self.file = self.file.saturating_sub(1),
            KeyCode::Enter => self.open_file(),
            KeyCode::Char('h') | KeyCode::Left => self.move_focus(-1),
            KeyCode::Esc if self.overlay.is_some() => self.focus_on(Focus::Centre),
            _ => {}
        }
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
        if self.focus != Focus::Centre {
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
        if self.help {
            return;
        }
        let at = Position::new(mouse.column, mouse.row);
        let over = |frame: Option<ratatui::layout::Rect>| frame.is_some_and(|r| r.contains(at));
        let on_menu = over(geometry.menu_frame.or(geometry.menu));
        let on_right = over(geometry.right_frame.or(geometry.right));
        // A column drawn on top of the centre hides the panel under it.
        let on_top = (on_menu && geometry.menu_frame.is_some())
            || (on_right && geometry.right_frame.is_some());
        let pane = geometry
            .panes
            .iter()
            .find(|pane| !on_top && (pane.body.contains(at) || over(pane.title)))
            .copied();

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(pane) = pane {
                    let had_focus = self.focus == Focus::Centre && self.current() == pane.which;
                    match pane.title {
                        Some(title) if title.contains(at) && geometry.title_lists_all => {
                            // A click on a name in the list of the three terminals.
                            let mut x = title.x;
                            for (which, name) in TERMINALS.iter().enumerate() {
                                let width = draw::terminal_label(which, name).len() as u16;
                                if at.x >= x && at.x < x + width {
                                    self.show_terminal(which);
                                }
                                x += width;
                            }
                        }
                        _ => self.show_terminal(pane.which),
                    }
                    if had_focus {
                        self.forward_mouse(&pane, mouse);
                    }
                } else if on_menu && let Some(area) = geometry.menu {
                    self.focus_on(Focus::Menu);
                    let view = draw::menu_view(self, area, geometry.menu_frame.is_none());
                    let row = usize::from(at.y.saturating_sub(view.first_row)) + view.offset;
                    if at.y >= view.first_row
                        && at.y < view.first_row + view.rows
                        && let Some(Some(stop)) = view.menu.stops.get(row)
                    {
                        self.selected = *stop;
                        self.start_terminals();
                    }
                } else if on_right {
                    self.focus_on(Focus::Right);
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
                } else if on_menu {
                    self.select(if up { -1 } else { 1 });
                } else if on_right {
                    self.file = if up {
                        self.file.saturating_sub(1)
                    } else {
                        (self.file + 1).min(self.files.len().saturating_sub(1))
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

    /// Passes a mouse event on to the child of a panel, when the child turned mouse reporting on
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
