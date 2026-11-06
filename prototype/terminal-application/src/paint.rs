//! Painting cell by cell, the way the design's `Buf` does (`design/orca-term TUI.dc.html`): `put`,
//! `segs`, `rsegs`, `fill`, `tint`, plus its `card`, `pill`, `key`, `rule` and `label`.
//!
//! Everything is painted with the theme's truecolor values. `finish` then turns the frame into
//! what the terminal gets: the same, the ANSI-256 fallbacks, or attributes only (NO_COLOR).

use ratatui::buffer::Buffer;
use ratatui::style::{Color, Modifier, Style};
use tui_term::vt100;

use crate::theme::{self, Tok};

/// How colours reach the terminal.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Truecolor,
    Ansi256,
    Mono,
}

impl Mode {
    /// `NO_COLOR` set: attributes only; `COLORTERM` truecolor or 24bit: truecolor; else 256.
    pub fn detect() -> Mode {
        if std::env::var_os("NO_COLOR").is_some_and(|value| !value.is_empty()) {
            return Mode::Mono;
        }
        match std::env::var("COLORTERM").as_deref() {
            Ok("truecolor" | "24bit") => Mode::Truecolor,
            _ => Mode::Ansi256,
        }
    }
}

/// A token as the colour painted (always its truecolor value; `finish` maps it).
pub fn c(tok: Tok) -> Color {
    tok.color(true)
}

/// A style with a foreground.
pub fn fg(tok: Tok) -> Style {
    Style::new().fg(c(tok))
}

pub fn bold(tok: Tok) -> Style {
    fg(tok).add_modifier(Modifier::BOLD)
}

pub fn italic(tok: Tok) -> Style {
    fg(tok).add_modifier(Modifier::ITALIC)
}

/// A piece of text with its style; a line is a list of them.
pub type Seg = (String, Style);

pub fn seg(text: impl Into<String>, style: Style) -> Seg {
    (text.into(), style)
}

pub fn len(text: &str) -> u16 {
    text.chars().count() as u16
}

pub fn segs_len(segs: &[Seg]) -> u16 {
    segs.iter().map(|(text, _)| len(text)).sum()
}

/// `text` cut to `n` characters, ending in `…` when it did not fit.
pub fn trunc(text: &str, n: usize) -> String {
    if text.chars().count() <= n {
        return text.to_owned();
    }
    let mut out: String = text.chars().take(n.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// The same, cut on the left: `…` then the end of `text`.
pub fn trunc_left(text: &str, n: usize) -> String {
    let count = text.chars().count();
    if count <= n {
        return text.to_owned();
    }
    let mut out = String::from("…");
    out.extend(text.chars().skip(count + 1 - n.max(1)));
    out
}

/// Words of `text` in lines of at most `width` characters.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        if line.is_empty() {
            line = word.to_owned();
        } else if line.chars().count() + 1 + word.chars().count() <= width {
            line.push(' ');
            line.push_str(word);
        } else {
            out.push(std::mem::take(&mut line));
            line = word.to_owned();
        }
    }
    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// The design's `Buf` over a ratatui buffer.
pub struct Painter<'a> {
    pub buf: &'a mut Buffer,
}

impl Painter<'_> {
    fn cell(&mut self, x: u16, y: u16) -> Option<&mut ratatui::buffer::Cell> {
        let area = self.buf.area;
        (x >= area.x && y >= area.y && x < area.right() && y < area.bottom())
            .then(|| &mut self.buf[(x, y)])
    }

    /// Blank cells on `bg`.
    pub fn fill(&mut self, x: u16, y: u16, w: u16, h: u16, bg: Tok) {
        for j in 0..h {
            for i in 0..w {
                if let Some(cell) = self.cell(x + i, y + j) {
                    cell.set_symbol(" ");
                    cell.bg = c(bg);
                    cell.fg = c(theme::TEXT);
                    cell.modifier = Modifier::empty();
                }
            }
        }
    }

    /// Only the background of the cells.
    pub fn tint(&mut self, x: u16, y: u16, w: u16, h: u16, bg: Tok) {
        for j in 0..h {
            for i in 0..w {
                if let Some(cell) = self.cell(x + i, y + j) {
                    cell.bg = c(bg);
                }
            }
        }
    }

    /// `text` from `x`, at most `max` characters. The foreground defaults to TEXT, the
    /// background stays unless the style has one, the attributes are the style's. Returns the
    /// column after the text.
    pub fn put(&mut self, x: u16, y: u16, text: &str, style: Style, max: u16) -> u16 {
        let mut i = 0;
        for ch in text.chars() {
            if i >= max {
                break;
            }
            if let Some(cell) = self.cell(x + i, y) {
                cell.set_char(ch);
                cell.fg = style.fg.unwrap_or(c(theme::TEXT));
                if let Some(bg) = style.bg {
                    cell.bg = bg;
                }
                cell.modifier = style.add_modifier;
            }
            i += 1;
        }
        x + i
    }

    pub fn text(&mut self, x: u16, y: u16, text: &str, style: Style) -> u16 {
        self.put(x, y, text, style, u16::MAX)
    }

    /// Several pieces one after the other, together at most `max` characters.
    pub fn segs(&mut self, x: u16, y: u16, segs: &[Seg], max: u16) -> u16 {
        let end = x.saturating_add(max);
        let mut cx = x;
        for (text, style) in segs {
            if cx >= end {
                break;
            }
            cx = self.put(cx, y, text, *style, end - cx);
        }
        cx
    }

    pub fn line(&mut self, x: u16, y: u16, segs: &[Seg]) -> u16 {
        self.segs(x, y, segs, u16::MAX)
    }

    /// Text ending just before column `end`.
    pub fn rtext(&mut self, end: u16, y: u16, text: &str, style: Style) -> u16 {
        let x = end.saturating_sub(len(text));
        self.text(x, y, text, style);
        x
    }

    /// Pieces ending just before column `end`. Returns where they start.
    pub fn rsegs(&mut self, end: u16, y: u16, segs: &[Seg]) -> u16 {
        let x = end.saturating_sub(segs_len(segs));
        self.line(x, y, segs);
        x
    }

    /// A card: half-line top and bottom (`▗▄▖` / `▝▀▘` in the card's colour over `under`), the
    /// body filled. `focus` is the length of the accent stroke `▁` over its title, 0 for none.
    pub fn card_on(&mut self, x: u16, y: u16, w: u16, h: u16, bg: Tok, under: Tok, focus: u16) {
        if w < 2 || h < 2 {
            return;
        }
        self.fill(x, y + 1, w, h - 2, bg);
        let edge = |l: char, m: char, r: char| {
            let mut s = String::new();
            s.push(l);
            s.extend(std::iter::repeat_n(m, usize::from(w - 2)));
            s.push(r);
            s
        };
        let style = fg(bg).bg(c(under));
        self.text(x, y, &edge('▗', '▄', '▖'), style);
        if focus > 0 {
            let stroke = "▁".repeat(usize::from(focus));
            self.text(x + 2, y, &stroke, fg(theme::ACCENT).bg(c(under)));
        }
        self.text(x, y + h - 1, &edge('▝', '▀', '▘'), style);
    }

    pub fn card(&mut self, x: u16, y: u16, w: u16, h: u16, bg: Tok, focus: u16) {
        self.card_on(x, y, w, h, bg, theme::CANVAS, focus);
    }

    /// A rule `─` in BORDER from `x` to just before `end`.
    pub fn rule(&mut self, x: u16, end: u16, y: u16) {
        if end > x {
            self.text(x, y, &"─".repeat(usize::from(end - x)), fg(theme::BORDER));
        }
    }

    /// A section label: bold text, an optional dimmed count, then a rule to `end`.
    pub fn label(&mut self, x: u16, end: u16, y: u16, text: &str, colour: Tok) {
        let after = self.text(x, y, text, bold(colour));
        self.rule(after + 1, end, y);
    }
}

/// A pill: `▐text▌`, the half blocks in the pill's colour over `on`.
pub fn pill(text: &str, bg: Tok, fg_tok: Tok, on: Tok) -> Vec<Seg> {
    vec![
        seg("▐", fg(bg).bg(c(on))),
        seg(text, bold(fg_tok).bg(c(bg))),
        seg("▌", fg(bg).bg(c(on))),
    ]
}

/// A keycap: ` k ` bold TEXT on RAISED, or lit (ACCENT on FOCUS_SEL).
pub fn key(k: &str, hot: bool) -> Seg {
    if hot {
        seg(
            format!(" {k} "),
            bold(theme::ACCENT).bg(c(theme::FOCUS_SEL)),
        )
    } else {
        seg(format!(" {k} "), bold(theme::TEXT).bg(c(theme::RAISED)))
    }
}

/// The 16 ANSI colours of an embedded terminal, as the theme's tokens.
fn ansi(index: u8) -> Tok {
    match index % 8 {
        0 if index == 8 => theme::TEXT_2,
        0 => theme::BG,
        1 => theme::FAILED,
        2 => theme::DONE,
        3 => theme::WAITING,
        4 | 6 => theme::WORKING,
        5 => theme::ACCENT,
        _ => theme::TEXT,
    }
}

fn pty_colour(colour: vt100::Color, default: Tok) -> Color {
    match colour {
        vt100::Color::Default => c(default),
        vt100::Color::Idx(i) if i < 16 => c(ansi(i)),
        vt100::Color::Idx(i) => Color::Indexed(i),
        vt100::Color::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

/// A terminal's screen into `area`, with its 16 ANSI colours on the theme (default background
/// BG, default foreground TEXT). Indexed colours above 15 and RGB colours pass unchanged.
pub fn screen(buf: &mut Buffer, area: ratatui::layout::Rect, screen: &vt100::Screen) {
    for row in 0..area.height {
        for col in 0..area.width {
            let Some(cell) = screen.cell(row, col) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let out = &mut buf[(area.x + col, area.y + row)];
            out.set_symbol(if cell.has_contents() {
                cell.contents()
            } else {
                " "
            });
            out.fg = pty_colour(cell.fgcolor(), theme::TEXT);
            out.bg = pty_colour(cell.bgcolor(), theme::BG);
            let mut modifier = Modifier::empty();
            for (on, m) in [
                (cell.bold(), Modifier::BOLD),
                (cell.dim(), Modifier::DIM),
                (cell.italic(), Modifier::ITALIC),
                (cell.underline(), Modifier::UNDERLINED),
                (cell.inverse(), Modifier::REVERSED),
            ] {
                if on {
                    modifier |= m;
                }
            }
            out.modifier = modifier;
        }
    }
}

const TOKENS: [Tok; 16] = [
    theme::CANVAS,
    theme::BG,
    theme::PANEL,
    theme::RAISED,
    theme::SELECTION,
    theme::FOCUS_SEL,
    theme::FOCUS_BG,
    theme::BORDER,
    theme::VEIL,
    theme::TEXT,
    theme::TEXT_2,
    theme::ACCENT,
    theme::WAITING,
    theme::WORKING,
    theme::DONE,
    theme::FAILED,
];

/// The ANSI-256 fallback of a token's truecolor value; anything else unchanged.
fn fallback(colour: Color) -> Color {
    TOKENS
        .iter()
        .find(|tok| c(**tok) == colour)
        .map_or(colour, |tok| tok.color(false))
}

/// The frame as the terminal gets it in `mode`.
pub fn finish(buf: &mut Buffer, mode: Mode) {
    match mode {
        Mode::Truecolor => {}
        Mode::Ansi256 => {
            for cell in &mut buf.content {
                cell.fg = fallback(cell.fg);
                cell.bg = fallback(cell.bg);
            }
        }
        Mode::Mono => {
            for cell in &mut buf.content {
                mono_cell(cell);
            }
        }
    }
}

/// README, NO_COLOR: no fg or bg. Focus (the cursor, the focused tab's pill, the lit keycaps,
/// the whole status bar while the prefix waits) is REVERSED+BOLD; accent text BOLD+UNDERLINED;
/// secondary text DIM; half lines and quadrants become spaces, except the `▄▀` of an unfocused
/// selection, which become `─` rules above and below it. The design's `mono()` cell by cell.
fn mono_cell(cell: &mut ratatui::buffer::Cell) {
    let is = |colour: Color, tok: Tok| colour == c(tok);
    let reversed = [
        theme::FOCUS_SEL,
        theme::ACCENT,
        theme::WORKING,
        theme::FOCUS_BG,
    ]
    .iter()
    .any(|tok| is(cell.bg, *tok));
    let fg_was = cell.fg;
    let bg_was = cell.bg;
    cell.fg = Color::Reset;
    cell.bg = Color::Reset;
    if reversed {
        cell.modifier |= theme::mono(true, false, false).add_modifier;
        return;
    }
    if is(bg_was, theme::TEXT) {
        // A cell drawn as a block cursor.
        cell.modifier |= Modifier::REVERSED;
        return;
    }
    let symbol = cell.symbol();
    if "▄▀▗▖▝▘▐▌".contains(symbol) && symbol.chars().count() == 1 {
        let selection = is(fg_was, theme::SELECTION) && (symbol == "▄" || symbol == "▀");
        cell.set_symbol(if selection { "─" } else { " " });
        cell.modifier = Modifier::DIM;
        return;
    }
    let accent = is(fg_was, theme::ACCENT);
    let stroke = symbol == "▁" || symbol == "▎";
    let secondary = [theme::TEXT_2, theme::BORDER, theme::VEIL]
        .iter()
        .any(|tok| is(fg_was, *tok));
    cell.modifier |= theme::mono(false, accent && !stroke, secondary).add_modifier;
    if accent {
        cell.modifier |= Modifier::BOLD;
    }
}
