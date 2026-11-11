//! App: the terminal emulator behind every panel (#174, #176): a `vt100` screen fed with a
//! program's output, read cell by cell and painted with the 16 ANSI colours on the theme. It also
//! answers the few questions a program asks its terminal and waits for (where the cursor is, what
//! terminal this is), which Neovim and fish ask as they start; the answers go back to the program
//! as if typed.
//!
//! `vt100` rather than `alacritty_terminal`, which the research also measured
//! (`docs/research/terminal-application.md` on the branch `research/terminal-application`):
//! both drew claude's and opencode's screens the same, and `vt100` is six crates instead of 56,
//! with a screen read cell by cell and no event loop to drive.

use std::num::NonZeroU16;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

use crate::app::theme::{self, Tok};

/// One terminal's screen.
pub(crate) struct Emulator {
    parser: vt100::Parser<Answers>,
}

impl std::fmt::Debug for Emulator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Emulator")
            .field("size", &self.size())
            .finish_non_exhaustive()
    }
}

/// The answers to the program's questions, waiting to be sent back.
#[derive(Debug, Default)]
struct Answers {
    bytes: Vec<u8>,
}

impl vt100::Callbacks for Answers {
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        first: Option<u8>,
        second: Option<u8>,
        params: &[&[u16]],
        last: char,
    ) {
        let param = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        match (first, second, last) {
            // Where is the cursor? (DSR 6)
            (None, None, 'n') if param == 6 => {
                let (row, col) = screen.cursor_position();
                let answer = format!("\x1b[{};{}R", row + 1, col + 1);
                self.bytes.extend(answer.bytes());
            }
            // Are you there? (DSR 5)
            (None, None, 'n') if param == 5 => self.bytes.extend(b"\x1b[0n"),
            // What are you? (DA1). Programs ask it last, to learn that the questions before it
            // got no answer, so they wait for this one.
            (None, None, 'c') if param == 0 => self.bytes.extend(b"\x1b[?62;22c"),
            // Which version are you? (DA2)
            (Some(b'>'), None, 'c') => self.bytes.extend(b"\x1b[>0;0;0c"),
            _ => {}
        }
    }
}

impl Emulator {
    /// An empty screen `size` cells large.
    pub(crate) fn new(size: Rect) -> Self {
        let (rows, cols) = dimensions(size);
        Self {
            parser: vt100::Parser::new_with_callbacks(rows, cols, 0, Answers::default()),
        }
    }

    /// An empty screen `size` cells large that keeps up to `lines` rows scrolled off its top, to
    /// read back: the agent's log (#178).
    pub(crate) fn with_scrollback(size: Rect, lines: usize) -> Self {
        let (rows, cols) = dimensions(size);
        Self {
            parser: vt100::Parser::new_with_callbacks(rows, cols, lines, Answers::default()),
        }
    }

    /// Shows the screen `up` rows back into what scrolled off its top, at most as far as it
    /// kept; 0 is the screen as it is. Returns how far back it shows.
    pub(crate) fn scroll(&mut self, up: usize) -> usize {
        let screen = self.parser.screen_mut();
        screen.set_scrollback(up);
        screen.scrollback()
    }

    /// Draws the program's `bytes`, and returns the answers to the questions they asked.
    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.parser.process(bytes);
        std::mem::take(&mut self.parser.callbacks_mut().bytes)
    }

    /// Makes the screen `size` large. Returns whether it changed, so the program must be told.
    pub(crate) fn resize(&mut self, size: Rect) -> bool {
        let (rows, cols) = dimensions(size);
        let (now_rows, now_cols) = self.parser.screen().size();
        if (rows, cols) == (now_rows, now_cols) {
            return false;
        }
        self.parser.set_size(rows, cols);
        true
    }

    /// The size the screen is, in rows and columns.
    pub(crate) fn size(&self) -> (u16, u16) {
        let (rows, cols) = self.parser.screen().size();
        (rows.get(), cols.get())
    }

    /// Whether the program asked for the cursor keys' application mode.
    pub(crate) fn application_cursor(&self) -> bool {
        self.parser.screen().application_cursor()
    }

    /// Whether the program asked for pastes inside the bracketed-paste marks.
    pub(crate) fn bracketed_paste(&self) -> bool {
        self.parser.screen().bracketed_paste()
    }

    /// Where the program's cursor is in `area`, when it shows one.
    pub(crate) fn cursor(&self, area: Rect) -> Option<(u16, u16)> {
        let screen = self.parser.screen();
        let (row, col) = screen.cursor_position();
        (!screen.hide_cursor() && row < area.height && col < area.width)
            .then_some((area.x + col, area.y + row))
    }

    /// Paints the screen into `area` of `buf`, its 16 ANSI colours on the theme's tokens (the
    /// default background BG, the default text TEXT), any other colour as the program gave it.
    pub(crate) fn paint(&self, buf: &mut Buffer, area: Rect) {
        self.paint_from(buf, area, 0);
    }

    /// Paints the end of the output into `area`: the rows up to the last one with something on
    /// it, as many as `area` holds, so a card under them leaves the program's last words in view.
    pub(crate) fn paint_end(&self, buf: &mut Buffer, area: Rect) {
        let screen = self.parser.screen();
        let (rows, cols) = self.size();
        let written = |row: u16| {
            (0..cols).any(|col| screen.cell(row, col).is_some_and(vt100::Cell::has_contents))
        };
        let last = (0..rows).rev().find(|row| written(*row));
        let first = last.map_or(0, |last| (last + 1).saturating_sub(area.height));
        self.paint_from(buf, area, first);
    }

    /// Paints the screen from its row `first` into `area`.
    fn paint_from(&self, buf: &mut Buffer, area: Rect, first: u16) {
        let screen = self.parser.screen();
        for position in area.intersection(buf.area).positions() {
            let (row, col) = (position.y - area.y + first, position.x - area.x);
            // The second cell of a wide character is drawn by the first.
            if let Some(cell) = screen.cell(row, col)
                && !cell.is_wide_continuation()
            {
                paint_cell(&mut buf[position], cell);
            }
        }
    }
}

/// One cell of the program's screen, as the theme shows it.
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

/// The 16 ANSI colours of a panel's terminal, as the design maps them onto the theme.
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

    fn emulator() -> Emulator {
        Emulator::new(Rect::new(0, 0, 20, 4))
    }

    #[test]
    fn output_draws_with_the_themes_colours() {
        let mut emulator = emulator();
        emulator.feed(b"\x1b[31mred\x1b[0m ok");
        let mut buf = Buffer::empty(Rect::new(0, 0, 20, 4));
        emulator.paint(&mut buf, Rect::new(0, 0, 20, 4));
        assert_eq!(buf[(0, 0)].symbol(), "r");
        assert_eq!(buf[(0, 0)].fg, theme::FAILED.color());
        assert_eq!(buf[(4, 0)].fg, theme::TEXT.color());
        assert_eq!(buf[(4, 0)].bg, theme::BG.color());
        assert_eq!(emulator.cursor(Rect::new(5, 5, 20, 4)), Some((11, 5)));
    }

    #[test]
    fn modes_and_size_are_read_from_the_screen() {
        let mut emulator = emulator();
        emulator.feed(b"\x1b[?1h\x1b[?2004h\x1b[?25l");
        assert!(emulator.application_cursor());
        assert!(emulator.bracketed_paste());
        assert_eq!(emulator.cursor(Rect::new(0, 0, 20, 4)), None, "hidden");
        assert!(!emulator.resize(Rect::new(9, 9, 20, 4)), "the same size");
        assert!(emulator.resize(Rect::new(0, 0, 30, 5)));
        assert_eq!(emulator.size(), (5, 30));
    }

    #[test]
    fn a_screen_with_scrollback_shows_the_rows_that_went_off_its_top() {
        let mut log = Emulator::with_scrollback(Rect::new(0, 0, 10, 2), 100);
        log.feed(b"one\r\ntwo\r\nthree\r\nfour");
        let first_row = |log: &Emulator| {
            let mut buf = Buffer::empty(Rect::new(0, 0, 10, 2));
            log.paint(&mut buf, Rect::new(0, 0, 10, 2));
            (0..5)
                .map(|x| buf[(x, 0)].symbol().to_owned())
                .collect::<String>()
        };
        assert_eq!(first_row(&log), "three");
        assert_eq!(log.scroll(1), 1);
        assert_eq!(first_row(&log), "two  ");
        assert_eq!(log.scroll(50), 2, "as far as it kept");
        assert_eq!(first_row(&log), "one  ");
        assert_eq!(log.scroll(0), 0);
    }

    #[test]
    fn the_end_of_the_output_paints_its_last_written_rows() {
        let mut emulator = Emulator::new(Rect::new(0, 0, 10, 4));
        emulator.feed(b"one\r\ntwo\r\nthree");
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 2));
        emulator.paint_end(&mut buf, Rect::new(0, 0, 10, 2));
        assert_eq!(buf[(0, 0)].symbol(), "t");
        assert_eq!(buf[(1, 0)].symbol(), "w", "two");
        assert_eq!(buf[(1, 1)].symbol(), "h", "three");
    }

    #[test]
    fn the_questions_a_program_waits_for_are_answered() {
        let mut emulator = emulator();
        assert_eq!(emulator.feed(b"ab\x1b[6n"), b"\x1b[1;3R");
        assert_eq!(emulator.feed(b"\x1b[5n"), b"\x1b[0n");
        assert_eq!(emulator.feed(b"\x1b[c"), b"\x1b[?62;22c");
        assert_eq!(emulator.feed(b"\x1b[>c"), b"\x1b[>0;0;0c");
        assert_eq!(emulator.feed(b"plain text"), b"", "nothing asked");
    }
}
