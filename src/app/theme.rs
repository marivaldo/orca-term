//! App: the colours and glyphs of the "Quiet v2" look (ADR 0012), copied from the design's
//! tokens (`prototype/terminal-application/design/theme.rs` on the branch
//! `prototype/terminal-application`). Every token has a truecolor value and an ANSI-256 fallback,
//! and every text token reads at 4.5:1 or more on every surface, in both.

use ratatui::style::Color;

use crate::domain::pr::Checks;
use crate::domain::state::State;

/// A colour of the theme: its truecolor value and its ANSI-256 fallback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Tok {
    pub(crate) rgb: (u8, u8, u8),
    pub(crate) ansi: u8,
}

impl Tok {
    /// The colour everything is painted with; [`crate::app::paint::finish`] turns it into the
    /// fallback when the terminal has no truecolor.
    pub(crate) const fn color(self) -> Color {
        Color::Rgb(self.rgb.0, self.rgb.1, self.rgb.2)
    }
}

// Surfaces.
/// Between the cards, and the status bar.
pub(crate) const CANVAS: Tok = tok(0x06, 0x07, 0x08, 232);
/// The terminals' background.
pub(crate) const BG: Tok = tok(0x0e, 0x10, 0x13, 233);
/// The side menu's card.
pub(crate) const PANEL: Tok = tok(0x17, 0x1a, 0x1f, 234);
/// Keycaps and a project's initial.
pub(crate) const RAISED: Tok = tok(0x1f, 0x23, 0x29, 235);
/// The open worktree and the active tab, where the keys do not go.
pub(crate) const SELECTION: Tok = tok(0x2a, 0x2f, 0x37, 236);
/// The cursor and the active tab where the keys go.
pub(crate) const FOCUS_SEL: Tok = tok(0x2f, 0x28, 0x50, 236);
/// The status bar while the prefix waits for its key.
pub(crate) const FOCUS_BG: Tok = tok(0x2a, 0x23, 0x40, 236);

// Decoration, never text.
pub(crate) const BORDER: Tok = tok(0x34, 0x3a, 0x43, 237);
/// The text behind a floating card (the picker, help), veiled.
pub(crate) const VEIL: Tok = tok(0x50, 0x56, 0x5f, 240);

// Text.
pub(crate) const TEXT: Tok = tok(0xdd, 0xe1, 0xe6, 254);
pub(crate) const TEXT_2: Tok = tok(0x9a, 0xa2, 0xac, 247);
/// Only where the keys go.
pub(crate) const ACCENT: Tok = tok(0xb9, 0xa2, 0xff, 147);
pub(crate) const WAITING: Tok = tok(0xe8, 0xb8, 0x5a, 179);
pub(crate) const WORKING: Tok = tok(0x67, 0xb7, 0xf0, 75);
pub(crate) const DONE: Tok = tok(0x7f, 0xcf, 0x98, 114);
pub(crate) const FAILED: Tok = tok(0xf0, 0x7f, 0x7f, 210);

/// Every token, for turning a painted colour back into its fallback.
pub(crate) const TOKENS: [Tok; 16] = [
    CANVAS, BG, PANEL, RAISED, SELECTION, FOCUS_SEL, FOCUS_BG, BORDER, VEIL, TEXT, TEXT_2, ACCENT,
    WAITING, WORKING, DONE, FAILED,
];

const fn tok(r: u8, g: u8, b: u8, ansi: u8) -> Tok {
    Tok {
        rgb: (r, g, b),
        ansi,
    }
}

/// The frames of the working glyph, 120 ms each.
pub(crate) const SPINNER: [char; 4] = ['◐', '◓', '◑', '◒'];

/// The mark of an unseen worktree, after its name, in its state's colour.
pub(crate) const UNSEEN: char = '•';

/// How a worktree's state is shown: its glyph at spinner frame `frame`, and its colour.
pub(crate) fn look(state: &State, frame: usize) -> (char, Tok) {
    match state {
        State::Working => (SPINNER[frame % SPINNER.len()], WORKING),
        State::Waiting => ('?', WAITING),
        State::Finished => ('●', DONE),
        State::Failed => ('✗', FAILED),
        State::Interrupted => ('●', TEXT_2),
        State::Idle => ('○', TEXT_2),
        State::NoAgent => ('·', TEXT_2),
        State::SettingUp => ('◌', WORKING),
        State::NotReady(_) => ('△', FAILED),
        State::Broken(_) => ('!', FAILED),
    }
}

/// How a pull request's checks are shown, when they say anything.
pub(crate) fn check_look(checks: Checks) -> Option<(char, Tok)> {
    match checks {
        Checks::Passing => Some(('✓', DONE)),
        Checks::Failing => Some(('✗', FAILED)),
        Checks::Pending => Some(('◐', WORKING)),
        Checks::None => None,
    }
}
