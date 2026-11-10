//! App: the agent's panel, the open worktree's agent terminal in the centre (#174). Its screen is
//! a `vt100` terminal fed with what the agent's holder sends: the replay first, then the agent's
//! output as it comes. The connection itself lives in the run loop (`app.rs`); this is what is
//! drawn and what the keys need to know (the cursor keys mode, bracketed paste).
//!
//! `vt100` rather than `alacritty_terminal`, which the research also measured
//! (`docs/research/terminal-application.md` on the branch `research/terminal-application`):
//! both drew claude's and opencode's screens the same, and `vt100` is six crates instead of 56,
//! with a screen read cell by cell and no event loop to drive. What `alacritty_terminal` adds
//! (answering the agent's questions, the kitty keyboard flags) this slice does not use.

use std::num::NonZeroU16;
use std::path::PathBuf;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

use crate::adapters::holder::client::Heard;
use crate::app::theme::{self, Tok};

/// Whether the panel shows a running agent, and how it got there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Status {
    /// No agent runs in the worktree: `enter` starts one.
    NoAgent,
    /// The core is starting the agent.
    Starting,
    /// Attached to the agent's holder.
    Attached,
    /// The agent ended, as the text says (`exit 0`): `enter` starts it again.
    Ended(String),
}

/// The open worktree's agent terminal.
pub(crate) struct Panel {
    /// The worktree whose agent this is.
    pub(crate) worktree: PathBuf,
    pub(crate) status: Status,
    screen: vt100::Parser,
}

impl std::fmt::Debug for Panel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Panel")
            .field("worktree", &self.worktree)
            .field("status", &self.status)
            .finish_non_exhaustive()
    }
}

impl Panel {
    /// The panel of the worktree at `worktree`, empty, `size` cells large.
    pub(crate) fn new(worktree: PathBuf, status: Status, size: Rect) -> Self {
        let (rows, cols) = dimensions(size);
        Self {
            worktree,
            status,
            screen: vt100::Parser::new(rows, cols, 0),
        }
    }

    /// What the holder said: output for the screen, or the agent's end.
    pub(crate) fn heard(&mut self, heard: Heard) {
        match heard {
            Heard::Output(bytes) => self.screen.process(&bytes),
            Heard::Ended(text) => self.status = Status::Ended(text),
            Heard::Closed => {
                if self.status == Status::Attached {
                    self.status = Status::NoAgent;
                }
            }
        }
    }

    /// Makes the screen `size` large. Returns whether it changed, so the holder must be told.
    pub(crate) fn resize(&mut self, size: Rect) -> bool {
        let (rows, cols) = dimensions(size);
        let (now_rows, now_cols) = self.screen.screen().size();
        if (rows, cols) == (now_rows, now_cols) {
            return false;
        }
        self.screen.set_size(rows, cols);
        true
    }

    /// The size the screen is, in rows and columns.
    pub(crate) fn size(&self) -> (u16, u16) {
        let (rows, cols) = self.screen.screen().size();
        (rows.get(), cols.get())
    }

    /// Whether the agent asked for the cursor keys' application mode.
    pub(crate) fn application_cursor(&self) -> bool {
        self.screen.screen().application_cursor()
    }

    /// Whether the agent asked for pastes inside the bracketed-paste marks.
    pub(crate) fn bracketed_paste(&self) -> bool {
        self.screen.screen().bracketed_paste()
    }

    /// Where the agent's cursor is in `area`, when it shows one.
    pub(crate) fn cursor(&self, area: Rect) -> Option<(u16, u16)> {
        let screen = self.screen.screen();
        let (row, col) = screen.cursor_position();
        (!screen.hide_cursor() && row < area.height && col < area.width)
            .then_some((area.x + col, area.y + row))
    }

    /// Paints the screen into `area` of `buf`, its 16 ANSI colours on the theme's tokens (the
    /// default background BG, the default text TEXT), any other colour as the agent gave it.
    pub(crate) fn paint(&self, buf: &mut Buffer, area: Rect) {
        let screen = self.screen.screen();
        for position in area.intersection(buf.area).positions() {
            let (row, col) = (position.y - area.y, position.x - area.x);
            // The second cell of a wide character is drawn by the first.
            if let Some(cell) = screen.cell(row, col)
                && !cell.is_wide_continuation()
            {
                paint_cell(&mut buf[position], cell);
            }
        }
    }
}

/// One cell of the agent's screen, as the theme shows it.
fn paint_cell(out: &mut ratatui::buffer::Cell, cell: &vt100::Cell) {
    let symbol = if cell.has_contents() {
        cell.contents()
    } else {
        " "
    };
    out.set_symbol(symbol);
    out.fg = colour(cell.fgcolor(), theme::TEXT);
    out.bg = colour(cell.bgcolor(), theme::BG);
    out.modifier = modifiers(cell);
}

/// A size as `vt100` takes it: at least one row and one column.
fn dimensions(size: Rect) -> (NonZeroU16, NonZeroU16) {
    (
        NonZeroU16::new(size.height).unwrap_or(NonZeroU16::MIN),
        NonZeroU16::new(size.width).unwrap_or(NonZeroU16::MIN),
    )
}

/// The 16 ANSI colours of the agent's terminal, as the design maps them onto the theme.
fn ansi(index: u8) -> Tok {
    match index {
        0 => theme::BG,
        8 => theme::TEXT_2,
        1 | 9 => theme::FAILED,
        2 | 10 => theme::DONE,
        3 | 11 => theme::WAITING,
        4 | 6 | 12 | 14 => theme::WORKING,
        5 | 13 => theme::ACCENT,
        _ => theme::TEXT,
    }
}

fn colour(colour: vt100::Color, default: Tok) -> Color {
    match colour {
        vt100::Color::Default => default.color(),
        vt100::Color::Idx(index) if index < 16 => ansi(index).color(),
        vt100::Color::Idx(index) => Color::Indexed(index),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn modifiers(cell: &vt100::Cell) -> Modifier {
    [
        (cell.bold(), Modifier::BOLD),
        (cell.dim(), Modifier::DIM),
        (cell.italic(), Modifier::ITALIC),
        (cell.underline(), Modifier::UNDERLINED),
        (cell.inverse(), Modifier::REVERSED),
    ]
    .into_iter()
    .filter(|(on, _)| *on)
    .fold(Modifier::empty(), |all, (_, modifier)| all | modifier)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn panel() -> Panel {
        Panel::new(
            PathBuf::from("/w"),
            Status::Attached,
            Rect::new(0, 0, 20, 4),
        )
    }

    #[test]
    fn the_holders_output_draws_with_the_themes_colours() {
        let mut panel = panel();
        panel.heard(Heard::Output(b"\x1b[31mred\x1b[0m ok".to_vec()));
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 4));
        panel.paint(&mut buf, Rect::new(0, 0, 20, 4));
        assert_eq!(buf[(0, 0)].symbol(), "r");
        assert_eq!(buf[(0, 0)].fg, theme::FAILED.color());
        assert_eq!(buf[(4, 0)].fg, theme::TEXT.color());
        assert_eq!(buf[(4, 0)].bg, theme::BG.color());
        assert_eq!(panel.cursor(Rect::new(5, 5, 20, 4)), Some((11, 5)));
    }

    #[test]
    fn the_agents_end_and_a_lost_holder_are_told_apart() {
        let mut panel = panel();
        panel.heard(Heard::Ended("exit 3".to_owned()));
        panel.heard(Heard::Closed);
        assert_eq!(panel.status, Status::Ended("exit 3".to_owned()));
        let mut panel = self::panel();
        panel.heard(Heard::Closed);
        assert_eq!(
            panel.status,
            Status::NoAgent,
            "the holder went without a word"
        );
    }

    #[test]
    fn modes_and_size_are_read_from_the_screen() {
        let mut panel = panel();
        panel.heard(Heard::Output(b"\x1b[?1h\x1b[?2004h\x1b[?25l".to_vec()));
        assert!(panel.application_cursor());
        assert!(panel.bracketed_paste());
        assert_eq!(panel.cursor(Rect::new(0, 0, 20, 4)), None, "hidden");
        assert!(!panel.resize(Rect::new(9, 9, 20, 4)), "the same size");
        assert!(panel.resize(Rect::new(0, 0, 30, 5)));
        assert_eq!(panel.size(), (5, 30));
    }
}
