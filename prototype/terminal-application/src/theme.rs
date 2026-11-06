//! orca-term theme tokens (Quiet v2). Truecolor first, ANSI-256 fallback.
use ratatui::style::{Color, Modifier, Style};

#[derive(Clone, Copy)]
pub struct Tok { pub rgb: (u8, u8, u8), pub ansi: u8 }
impl Tok {
    pub fn color(self, truecolor: bool) -> Color {
        if truecolor { Color::Rgb(self.rgb.0, self.rgb.1, self.rgb.2) } else { Color::Indexed(self.ansi) }
    }
}

// Surfaces
pub const CANVAS:    Tok = Tok { rgb: (0x06, 0x07, 0x08), ansi: 232 }; // screen bg between cards, status bar
pub const BG:        Tok = Tok { rgb: (0x0e, 0x10, 0x13), ansi: 233 }; // terminal panes (agent / nvim / shell)
pub const PANEL:     Tok = Tok { rgb: (0x17, 0x1a, 0x1f), ansi: 234 }; // sidebar + right cards
pub const RAISED:    Tok = Tok { rgb: (0x1f, 0x23, 0x29), ansi: 235 }; // keycaps, "primary" tag, project avatar
pub const SELECTION: Tok = Tok { rgb: (0x2a, 0x2f, 0x37), ansi: 236 }; // open worktree / active tab, pane NOT focused
pub const FOCUS_SEL: Tok = Tok { rgb: (0x2f, 0x28, 0x50), ansi: 236 }; // cursor / active tab inside FOCUSED pane
pub const FOCUS_BG:  Tok = Tok { rgb: (0x2a, 0x23, 0x40), ansi: 236 }; // status bar while prefix pending
// Non-text decoration
pub const BORDER:    Tok = Tok { rgb: (0x34, 0x3a, 0x43), ansi: 237 }; // section rules, ~ after EOF
pub const VEIL:      Tok = Tok { rgb: (0x50, 0x56, 0x5f), ansi: 240 }; // text veiled behind overlays
// Text (all >= 4.5:1 on every surface above, truecolor and 256)
pub const TEXT:      Tok = Tok { rgb: (0xdd, 0xe1, 0xe6), ansi: 254 };
pub const TEXT_2:    Tok = Tok { rgb: (0x9a, 0xa2, 0xac), ansi: 247 };
pub const ACCENT:    Tok = Tok { rgb: (0xb9, 0xa2, 0xff), ansi: 147 }; // ONLY where keys go
pub const WAITING:   Tok = Tok { rgb: (0xe8, 0xb8, 0x5a), ansi: 179 };
pub const WORKING:   Tok = Tok { rgb: (0x67, 0xb7, 0xf0), ansi: 75 };
pub const DONE:      Tok = Tok { rgb: (0x7f, 0xcf, 0x98), ansi: 114 };
pub const FAILED:    Tok = Tok { rgb: (0xf0, 0x7f, 0x7f), ansi: 210 };

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum State { Waiting, Working, Done, Idle, Failed }

pub const SPINNER: [char; 4] = ['◐', '◓', '◑', '◒']; // 120 ms per frame
pub fn glyph(s: State, frame: usize) -> char {
    match s { State::Waiting => '?', State::Working => SPINNER[frame % 4], State::Done => '●', State::Idle => '○', State::Failed => '✗' }
}
pub fn state_tok(s: State) -> Tok {
    match s { State::Waiting => WAITING, State::Working => WORKING, State::Done => DONE, State::Idle => TEXT_2, State::Failed => FAILED }
}
pub const UNSEEN: char = '•'; // drawn in the state colour

/// NO_COLOR: every style collapses to attributes only.
pub fn mono(focused_cursor: bool, accent: bool, secondary: bool) -> Style {
    let mut s = Style::default();
    if focused_cursor { s = s.add_modifier(Modifier::REVERSED | Modifier::BOLD); }
    if accent { s = s.add_modifier(Modifier::BOLD | Modifier::UNDERLINED); }
    if secondary { s = s.add_modifier(Modifier::DIM); }
    s
}
