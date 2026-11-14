//! PROTOTYPE (#203, throwaway): the ruled navigation on top of today's state. `Cmd` keys from a
//! panel (and from anywhere), `Alt+digit`, the fallback prefix's extra letters, the bare keys on
//! the side menu and the right column, the command palette, and the mouse.

use std::time::Instant;

use ratatui::crossterm::event::{
    KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Position;

use super::{App, Effect, Focus, Overlay};
use crate::app::emulator::Emulator;
use crate::app::keys;
use crate::app::menu;
use crate::app::panel::Status;
use crate::app::program::Kind;
use crate::app::proto::{self, Action, Hit, Palette};
use crate::app::view::Tab;
use crate::domain::agent::StartMode;

impl App {
    /// `F12`, `F9`, `F10`: the prototype's own keys, in every scheme.
    pub(super) fn proto_key(&mut self, key: KeyEvent, now: Instant) -> Option<Vec<Effect>> {
        match key.code {
            KeyCode::F(12) => {
                self.proto.next_scheme();
                let text = format!("scheme: {}", self.proto.scheme.name());
                self.say(text, now);
            }
            KeyCode::F(9) => {
                self.proto.spelling = self.proto.spelling.next();
                let text = format!("key spelling: {}", self.proto.spelling.name());
                self.say(text, now);
            }
            KeyCode::F(10) => self.proto.show_probe = !self.proto.show_probe,
            _ => return None,
        }
        self.proto.did("prototype key");
        Some(Vec::new())
    }

    /// A key with the super modifier, in the Cmd schemes. Super keys that are not the
    /// application's are dropped (a panel's program would read the bare letter otherwise).
    pub(super) fn cmd_key(&mut self, key: KeyEvent, now: Instant) -> Vec<Effect> {
        self.proto.super_seen = true;
        let KeyCode::Char(c) = key.code else {
            self.proto
                .did("dropped: super + a key that is not the application's");
            return Vec::new();
        };
        match c.to_ascii_lowercase() {
            'e' => {
                self.leave_to_side_menu(now);
                self.proto.did("ours: ⌘e leave the panel to the side menu");
            }
            d @ '1'..='3' => {
                self.proto.super_digit_seen = true;
                let tab = Tab::ALL[usize::from(d as u8 - b'1')];
                self.show(tab);
                self.proto.did(format!("ours: ⌘{d} the {} tab", tab.name()));
            }
            '[' => {
                self.cycle_tab(-1);
                self.proto.did("ours: ⌘[ previous tab");
            }
            ']' => {
                self.cycle_tab(1);
                self.proto.did("ours: ⌘] next tab");
            }
            'j' | 'd' => {
                self.proto.did("ours: next worktree");
                return self.open_step(1);
            }
            'k' | 'u' => {
                if c.eq_ignore_ascii_case(&'k') {
                    self.proto.super_k_seen = true;
                }
                self.proto.did("ours: previous worktree");
                return self.open_step(-1);
            }
            'p' => {
                self.overlay = Some(Overlay::Palette(Palette::default()));
                self.proto.did("ours: ⌘p palette");
            }
            '/' | '?' => {
                self.overlay = Some(Overlay::Help);
                self.proto.did("ours: help");
            }
            _ => self
                .proto
                .did("dropped: super + a key that is not the application's"),
        }
        Vec::new()
    }

    /// The fallback prefix's letters the ruled scheme adds to today's: `e` leaves the panel, `p`
    /// opens the palette.
    pub(super) fn prefix_extra(&mut self, key: KeyEvent, now: Instant) -> Option<Vec<Effect>> {
        match key.code {
            KeyCode::Char('e') => {
                self.leave_to_side_menu(now);
                self.proto.did("ours: ^a e leave the panel");
            }
            KeyCode::Char('p') => {
                self.overlay = Some(Overlay::Palette(Palette::default()));
                self.proto.did("ours: ^a p palette");
            }
            _ => return None,
        }
        Some(Vec::new())
    }

    /// `⌘e`: the focus goes to the side menu, with its cursor on the open worktree. Zoomed, the
    /// zoom goes first; where the side menu is hidden (under 100 columns) the picker opens.
    fn leave_to_side_menu(&mut self, now: Instant) {
        self.view.files.overlay = false;
        if self.layout.side_menu.is_none() && self.view.zoom {
            self.view.toggle_zoom();
            self.relayout();
        }
        if self.layout.side_menu.is_none() {
            self.say("no side menu at this width: the picker instead", now);
            self.open_picker();
            return;
        }
        self.focus = Focus::SideMenu;
        self.cursor.clone_from(&self.open);
    }

    fn cycle_tab(&mut self, by: isize) {
        let at = Tab::ALL
            .iter()
            .position(|tab| *tab == self.view.tab)
            .unwrap_or(0);
        let next = (at.cast_signed() + by).rem_euclid(3).cast_unsigned();
        self.show(Tab::ALL[next]);
    }

    /// Bare keys the ruled scheme adds on the side menu: `?` help, `w` the picker, `1`-`9` a
    /// numbered worktree, `l` or right into the panel, `tab` to the right column.
    pub(super) fn bare_side_menu_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        if self.typing.is_some()
            || key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT)
        {
            return None;
        }
        match key.code {
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help),
            KeyCode::Char('w') => self.open_picker(),
            KeyCode::Char(d @ '1'..='9') => {
                let order = self.side_menu_order();
                let path = order.get(usize::from(d as u8 - b'1')).cloned()?;
                self.cursor = Some(path.clone());
                self.proto.did(format!("ours: worktree {d}"));
                return Some(self.open(path));
            }
            KeyCode::Char('l') | KeyCode::Right => self.focus = Focus::Centre,
            KeyCode::Tab if self.layout.right.is_some() => self.focus = Focus::RightColumn,
            KeyCode::Tab => self.toggle_files(),
            _ => return None,
        }
        self.proto.did("ours: side menu key");
        Some(Vec::new())
    }

    /// Bare keys the ruled scheme adds on the right column: `?` help, `h` or left to the side
    /// menu.
    pub(super) fn bare_right_column_key(&mut self, key: KeyEvent) -> Option<Vec<Effect>> {
        match key.code {
            KeyCode::Char('?') => self.overlay = Some(Overlay::Help),
            KeyCode::Char('h') | KeyCode::Left if self.layout.side_menu.is_some() => {
                self.view.files.overlay = false;
                self.focus = Focus::SideMenu;
                self.cursor.clone_from(&self.open);
            }
            _ => return None,
        }
        Some(Vec::new())
    }

    /// A key for the open palette.
    pub(super) fn palette_key(
        &mut self,
        mut palette: Palette,
        key: KeyEvent,
        now: Instant,
    ) -> Vec<Effect> {
        let items = palette.matching();
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let last = items.len().saturating_sub(1);
        match key.code {
            KeyCode::Esc => return Vec::new(),
            KeyCode::Enter => {
                return match items.get(palette.cursor) {
                    Some((action, _)) => self.run_action(*action, now),
                    None => Vec::new(),
                };
            }
            KeyCode::Down | KeyCode::Tab => palette.cursor = (palette.cursor + 1).min(last),
            KeyCode::Char('n' | 'j') if ctrl => palette.cursor = (palette.cursor + 1).min(last),
            KeyCode::Up | KeyCode::BackTab => palette.cursor = palette.cursor.saturating_sub(1),
            KeyCode::Char('p' | 'k') if ctrl => palette.cursor = palette.cursor.saturating_sub(1),
            KeyCode::Backspace => {
                palette.query.pop();
                palette.cursor = 0;
            }
            KeyCode::Char(c)
                if !key.modifiers.intersects(
                    KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
                ) =>
            {
                palette.query.push(c);
                palette.cursor = 0;
            }
            _ => {}
        }
        self.overlay = Some(Overlay::Palette(palette));
        Vec::new()
    }

    /// Carries out one of the palette's actions.
    pub(super) fn run_action(&mut self, action: Action, now: Instant) -> Vec<Effect> {
        self.proto.did(format!("palette: {action:?}"));
        match action {
            Action::DraftPr => return self.draft_pr(now),
            Action::NewWorktree => {
                if self.layout.side_menu.is_some() {
                    self.focus = Focus::SideMenu;
                }
                self.ask_new_worktree();
            }
            Action::Picker => self.open_picker(),
            Action::Split => self.view.toggle_split(),
            Action::Zoom => {
                self.view.toggle_zoom();
                self.view.files.overlay = false;
                self.focus = Focus::Centre;
            }
            Action::Files => self.toggle_files(),
            Action::StartAgent => {
                self.show(Tab::Agent);
                return self.start_agent(StartMode::Resume);
            }
            Action::SendCtrlA => {
                let ctrl_a = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL);
                return self.type_focused(ctrl_a);
            }
            Action::Help => self.overlay = Some(Overlay::Help),
            Action::NextScheme => {
                self.proto.next_scheme();
                let text = format!("scheme: {}", self.proto.scheme.name());
                self.say(text, now);
            }
            Action::NextSpelling => {
                self.proto.spelling = self.proto.spelling.next();
                let text = format!("key spelling: {}", self.proto.spelling.name());
                self.say(text, now);
            }
            Action::ToggleProbe => self.proto.show_probe = !self.proto.show_probe,
            Action::ToggleMouse => {
                self.proto.mouse = !self.proto.mouse;
                let text = format!(
                    "mouse capture {}",
                    if self.proto.mouse { "on" } else { "off" }
                );
                self.say(text, now);
            }
            Action::Quit => return vec![Effect::Quit],
        }
        Vec::new()
    }

    /// A mouse event (#201): the first click on an unfocused panel only focuses it; a click on
    /// what the application drew acts; in the focused panel, clicks and the wheel go to a program
    /// that asked for the mouse.
    pub(crate) fn mouse(&mut self, event: MouseEvent) -> Vec<Effect> {
        let mut effects = self.mouse_event(event);
        self.relayout();
        effects.extend(self.start_shown());
        effects
    }

    fn mouse_event(&mut self, event: MouseEvent) -> Vec<Effect> {
        let (x, y) = (event.column, event.row);
        let at = Position { x, y };
        let down = matches!(event.kind, MouseEventKind::Down(_));
        // A press a panel took: its drag and release go there too, wherever they are.
        if let Some(tab) = self.proto.held
            && matches!(event.kind, MouseEventKind::Drag(_) | MouseEventKind::Up(_))
        {
            if matches!(event.kind, MouseEventKind::Up(_)) {
                self.proto.held = None;
            }
            return self.forward_mouse(tab, event);
        }
        if let Some(overlay) = &self.overlay {
            if down && matches!(overlay, Overlay::Help) {
                self.overlay = None;
            }
            return Vec::new();
        }
        if event.kind == MouseEventKind::Down(MouseButton::Left) {
            let hit = proto::hit_at(&self.hits.borrow(), x, y);
            if let Some(hit) = hit {
                return self.click(hit);
            }
        }
        if let Some(menu) = self.layout.side_menu
            && menu.contains(at)
        {
            let order = self.side_menu_order();
            match event.kind {
                MouseEventKind::ScrollDown | MouseEventKind::ScrollUp => {
                    let by = if event.kind == MouseEventKind::ScrollDown {
                        1
                    } else {
                        -1
                    };
                    if self.focus != Focus::SideMenu {
                        self.cursor.clone_from(&self.open);
                    }
                    self.focus = Focus::SideMenu;
                    self.cursor = menu::step(&order, self.cursor.as_deref(), by);
                    self.proto.did("ours: wheel on the side menu");
                }
                MouseEventKind::Down(_) => {
                    if self.focus != Focus::SideMenu {
                        self.cursor.clone_from(&self.open);
                    }
                    self.focus = Focus::SideMenu;
                    self.proto.did("ours: click focuses the side menu");
                }
                _ => {}
            }
            return Vec::new();
        }
        if let Some(right) = self.layout.right
            && right.area.contains(at)
        {
            let files = self.open_changes().map_or(0, |changes| changes.files.len());
            match event.kind {
                MouseEventKind::ScrollDown => {
                    self.file_cursor = (self.file_cursor + 1).min(files.saturating_sub(1));
                }
                MouseEventKind::ScrollUp => self.file_cursor = self.file_cursor.saturating_sub(1),
                MouseEventKind::Down(_) => {
                    self.focus = Focus::RightColumn;
                    self.proto.did("ours: click focuses the right column");
                }
                _ => {}
            }
            return Vec::new();
        }
        let card = self
            .layout
            .cards
            .iter()
            .find(|card| card.area.contains(at))
            .copied();
        let Some(card) = card else {
            return Vec::new();
        };
        let focused = self.focus == Focus::Centre && self.view.tab == card.tab;
        if !focused {
            if down {
                self.show(card.tab);
                self.proto
                    .did("ours: first click on a panel only focuses it");
            }
            return Vec::new();
        }
        if card.body.contains(at) {
            return self.forward_mouse(card.tab, event);
        }
        Vec::new()
    }

    /// A click on something the application drew.
    fn click(&mut self, hit: Hit) -> Vec<Effect> {
        self.proto.did(format!("ours: click on {hit:?}"));
        match hit {
            Hit::Worktree(path) => {
                self.focus = Focus::Centre;
                self.cursor = Some(path.clone());
                self.open(path)
            }
            Hit::Tab(tab) => {
                self.show(tab);
                Vec::new()
            }
            Hit::File(index) => {
                self.file_cursor = index;
                self.focus = Focus::RightColumn;
                self.open_diff()
            }
        }
    }

    /// `event` sent to the program in `tab`'s panel, in its panel's coordinates (kept inside it),
    /// when the program asked for it.
    fn forward_mouse(&mut self, tab: Tab, event: MouseEvent) -> Vec<Effect> {
        let body = self.layout.body_of(tab);
        let col = event
            .column
            .saturating_sub(body.x)
            .min(body.width.saturating_sub(1));
        let row = event
            .row
            .saturating_sub(body.y)
            .min(body.height.saturating_sub(1));
        let encode = |screen: &Emulator| {
            let (mode, encoding) = screen.mouse();
            keys::mouse(event.kind, event.modifiers, col, row, mode, encoding)
                .map(|bytes| (bytes, format!("{mode:?}/{encoding:?}")))
        };
        let sent = match Kind::of(tab) {
            None => self
                .panel
                .as_ref()
                .filter(|panel| panel.status == Status::Attached)
                .and_then(|panel| encode(&panel.screen))
                .map(|(bytes, how)| (Effect::Type(bytes.clone()), bytes, how)),
            Some(kind) => self.program_id(kind).and_then(|id| {
                let program = self.programs.get(&id).filter(|program| program.running())?;
                encode(&program.screen)
                    .map(|(bytes, how)| (Effect::TypeInto(id, bytes.clone()), bytes, how))
            }),
        };
        match sent {
            Some((effect, bytes, how)) => {
                if matches!(event.kind, MouseEventKind::Down(_)) {
                    self.proto.held = Some(tab);
                }
                self.proto.did(format!(
                    "to the {} panel ({how}) at {col},{row}: {}",
                    tab.name(),
                    bytes.escape_ascii()
                ));
                vec![effect]
            }
            None => {
                self.proto
                    .did("not forwarded: the program did not ask for this mouse event");
                Vec::new()
            }
        }
    }
}
