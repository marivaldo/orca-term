//! PROTOTYPE (#203, throwaway): the navigation schemes the driver switches between live, how the
//! status bar spells the keys, the command palette's actions, the mouse's hit map and the key
//! probe's ring of recent events.

use std::collections::VecDeque;
use std::path::PathBuf;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;

use crate::app::view::Tab;

/// The navigation scheme in force. `F12` (or the palette) cycles them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Scheme {
    /// The rulings of #199-#202: bare keys on the application's regions, `Cmd` keys in a panel,
    /// `Alt+digit` and the `Ctrl-a` prefix as the fallback. The keyboard stays in its legacy
    /// encoding: no kitty keyboard protocol pushed.
    Cmd,
    /// The same, with the kitty keyboard protocol pushed (flag 1, disambiguate).
    CmdKkp,
    /// Today's scheme: only the `Ctrl-a` prefix, no mouse.
    CtrlA,
}

impl Scheme {
    pub(crate) fn from_env() -> Self {
        match std::env::var("ORCA_PROTO_SCHEME").as_deref() {
            Ok("cmd-kkp" | "kkp") => Self::CmdKkp,
            Ok("ctrl-a" | "today") => Self::CtrlA,
            _ => Self::Cmd,
        }
    }

    pub(crate) fn next(self) -> Self {
        match self {
            Self::Cmd => Self::CmdKkp,
            Self::CmdKkp => Self::CtrlA,
            Self::CtrlA => Self::Cmd,
        }
    }

    /// The pill in the status bar.
    pub(crate) fn badge(self) -> &'static str {
        match self {
            Self::Cmd => "⌘ legacy",
            Self::CmdKkp => "⌘ kkp",
            Self::CtrlA => "^a today",
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Cmd => "cmd (legacy keyboard)",
            Self::CmdKkp => "cmd + kitty keyboard protocol pushed",
            Self::CtrlA => "ctrl-a (today)",
        }
    }

    /// Whether the ruled keys (Cmd, Alt+digit, bare keys) are in force.
    pub(crate) fn cmd(self) -> bool {
        self != Self::CtrlA
    }

    /// Whether the kitty keyboard protocol is pushed.
    pub(crate) fn kkp(self) -> bool {
        self == Self::CmdKkp
    }
}

/// How the status bar decides to spell the panel's keys with `⌘` or with the fallback. `F9`
/// cycles it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Spelling {
    /// `⌘` when the startup probe says the terminal speaks the kitty protocol, or once a super key
    /// arrived.
    Auto,
    /// `⌘` only when the startup probe said yes.
    Probe,
    /// `⌘` only once a super key arrived.
    Seen,
    /// Always `⌘`.
    Cmd,
    /// Always the fallback.
    Fallback,
}

impl Spelling {
    pub(crate) fn next(self) -> Self {
        match self {
            Self::Auto => Self::Probe,
            Self::Probe => Self::Seen,
            Self::Seen => Self::Cmd,
            Self::Cmd => Self::Fallback,
            Self::Fallback => Self::Auto,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Probe => "probe",
            Self::Seen => "seen",
            Self::Cmd => "⌘",
            Self::Fallback => "fallback",
        }
    }
}

/// Something the application drew that a click acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Hit {
    Worktree(PathBuf),
    Tab(Tab),
    File(usize),
}

/// The prototype's state.
#[derive(Debug)]
pub(crate) struct Proto {
    pub(crate) scheme: Scheme,
    pub(crate) spelling: Spelling,
    /// What `supports_keyboard_enhancement` answered at startup (`None`: no answer).
    pub(crate) probe: Option<bool>,
    /// Whether any key with the super modifier arrived.
    pub(crate) super_seen: bool,
    /// Whether `Cmd+1/2/3` arrived (kitty takes them by default).
    pub(crate) super_digit_seen: bool,
    /// Whether `Cmd+k` arrived (kitty takes it by default).
    pub(crate) super_k_seen: bool,
    /// Whether the mouse is captured.
    pub(crate) mouse: bool,
    /// Whether the probe's overlay shows.
    pub(crate) show_probe: bool,
    /// The last events, for the overlay.
    pub(crate) events: VecDeque<String>,
    /// What the application did with the last key or click, for the log.
    pub(crate) last: Option<String>,
    /// The panel a forwarded press went to, so its drag and release go there too.
    pub(crate) held: Option<Tab>,
    /// Where the log file is.
    pub(crate) log_path: Option<PathBuf>,
}

impl Proto {
    pub(crate) fn new() -> Self {
        let scheme = Scheme::from_env();
        Self {
            scheme,
            spelling: Spelling::Auto,
            probe: None,
            super_seen: false,
            super_digit_seen: false,
            super_k_seen: false,
            mouse: scheme.cmd(),
            show_probe: std::env::var_os("ORCA_PROTO_PROBE").is_some(),
            events: VecDeque::new(),
            last: None,
            held: None,
            log_path: None,
        }
    }

    /// Whether the status bar spells the panel's keys with `⌘`.
    pub(crate) fn spell_super(&self) -> bool {
        let probed = self.probe == Some(true);
        match self.spelling {
            Spelling::Auto => probed || self.super_seen,
            Spelling::Probe => probed,
            Spelling::Seen => self.super_seen,
            Spelling::Cmd => true,
            Spelling::Fallback => false,
        }
    }

    pub(crate) fn did(&mut self, what: impl Into<String>) {
        self.last = Some(what.into());
    }

    pub(crate) fn remember(&mut self, line: String) {
        self.events.push_back(line);
        while self.events.len() > 14 {
            self.events.pop_front();
        }
    }

    /// Next scheme: changes the mouse with it (today's had none).
    pub(crate) fn next_scheme(&mut self) {
        let was = self.scheme;
        self.scheme = self.scheme.next();
        if was.cmd() != self.scheme.cmd() {
            self.mouse = self.scheme.cmd();
        }
    }

    /// The panel's keys as the status bar shows them: (key, what).
    pub(crate) fn panel_keys(&self) -> Vec<(String, &'static str)> {
        if self.spell_super() {
            let tabs = if self.super_digit_seen {
                "⌘1-3"
            } else {
                "⌘[ ⌘]"
            };
            let worktrees = if self.super_k_seen {
                "⌘j ⌘k"
            } else {
                "⌘u ⌘d"
            };
            vec![
                ("⌘e".to_owned(), "menu"),
                (tabs.to_owned(), "tab"),
                (worktrees.to_owned(), "worktree"),
                ("⌘p".to_owned(), "palette"),
            ]
        } else {
            vec![
                ("^a e".to_owned(), "menu"),
                ("⌥1-3".to_owned(), "tab"),
                ("^a j k".to_owned(), "worktree"),
                ("^a p".to_owned(), "palette"),
            ]
        }
    }
}

/// The actions the palette offers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    DraftPr,
    NewWorktree,
    Picker,
    Split,
    Zoom,
    Files,
    StartAgent,
    SendCtrlA,
    Help,
    NextScheme,
    NextSpelling,
    ToggleProbe,
    ToggleMouse,
    Quit,
}

pub(crate) const ACTIONS: &[(Action, &str)] = &[
    (Action::DraftPr, "draft pull request"),
    (Action::NewWorktree, "new worktree"),
    (Action::Picker, "switch worktree (picker)"),
    (Action::Split, "split agent | nvim"),
    (Action::Zoom, "zoom the focused panel"),
    (Action::Files, "show / hide the changed files"),
    (Action::StartAgent, "start the agent"),
    (Action::SendCtrlA, "send Ctrl-a to the panel"),
    (Action::Help, "help: every key"),
    (
        Action::NextScheme,
        "prototype: next navigation scheme (F12)",
    ),
    (Action::NextSpelling, "prototype: next key spelling (F9)"),
    (
        Action::ToggleProbe,
        "prototype: show / hide the key probe (F10)",
    ),
    (Action::ToggleMouse, "prototype: mouse capture on / off"),
    (Action::Quit, "quit (the agents keep running)"),
];

/// The palette's filter and cursor.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Palette {
    pub(crate) query: String,
    pub(crate) cursor: usize,
}

impl Palette {
    pub(crate) fn matching(&self) -> Vec<(Action, &'static str)> {
        let query = self.query.to_lowercase();
        ACTIONS
            .iter()
            .filter(|(_, label)| {
                query
                    .split_whitespace()
                    .all(|word| label.to_lowercase().contains(word))
            })
            .copied()
            .collect()
    }
}

/// A key as a person names it: `⌘⇧e`, `⌥1`, `^a`, `F12`.
pub(crate) fn describe(key: KeyEvent) -> String {
    let m = key.modifiers;
    let mut out = String::new();
    for (bit, name) in [
        (KeyModifiers::CONTROL, "ctrl+"),
        (KeyModifiers::ALT, "alt+"),
        (KeyModifiers::SHIFT, "shift+"),
        (KeyModifiers::SUPER, "super+"),
        (KeyModifiers::HYPER, "hyper+"),
        (KeyModifiers::META, "meta+"),
    ] {
        if m.contains(bit) {
            out.push_str(name);
        }
    }
    match key.code {
        KeyCode::Char(c) => out.push_str(&format!("{c:?}")),
        code => out.push_str(&format!("{code:?}")),
    }
    out
}

/// `Alt+1/2/3`, or what Option+1/2/3 compose on a U.S. layout when Option is not Alt.
pub(crate) fn alt_digit(key: KeyEvent) -> Option<Tab> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    match key.code {
        KeyCode::Char('1') if alt => Some(Tab::Agent),
        KeyCode::Char('2') if alt => Some(Tab::Nvim),
        KeyCode::Char('3') if alt => Some(Tab::Shell),
        KeyCode::Char('¡') => Some(Tab::Agent),
        KeyCode::Char('™') => Some(Tab::Nvim),
        KeyCode::Char('£') => Some(Tab::Shell),
        _ => None,
    }
}

/// The cell under a point, as a hit.
pub(crate) fn hit_at(hits: &[(Rect, Hit)], x: u16, y: u16) -> Option<Hit> {
    hits.iter()
        .rev()
        .find(|(area, _)| area.contains(ratatui::layout::Position { x, y }))
        .map(|(_, hit)| hit.clone())
}
