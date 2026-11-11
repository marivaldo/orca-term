//! App: painting cell by cell, as the design describes its screens: text at a column, a card with
//! half-line edges, a pill, a keycap (`prototype/terminal-application/design/README.md`).
//!
//! Everything is painted with the theme's truecolor values; [`finish`] then turns the frame into
//! what the terminal can show: the same, the ANSI-256 fallbacks, or attributes only (`NO_COLOR`).

use ratatui::buffer::{Buffer, Cell};
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use unicode_width::{UnicodeWidthChar as _, UnicodeWidthStr as _};

use crate::app::theme::{self, Tok};

/// How the terminal shows colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Truecolor,
    Ansi256,
    /// `NO_COLOR`: no colour at all, only bold, dim, underline and reverse.
    Mono,
}

impl Mode {
    /// `NO_COLOR` set and not empty: attributes only; `COLORTERM` `truecolor` or `24bit`:
    /// truecolor; anything else: the 256 colours every terminal has.
    pub(crate) fn detect(no_color: bool, colorterm: Option<&str>) -> Self {
        if no_color {
            return Self::Mono;
        }
        match colorterm {
            Some("truecolor" | "24bit") => Self::Truecolor,
            _ => Self::Ansi256,
        }
    }
}

/// A style with the token as its text colour.
pub(crate) fn fg(tok: Tok) -> Style {
    Style::new().fg(tok.color())
}

pub(crate) fn bold(tok: Tok) -> Style {
    fg(tok).add_modifier(Modifier::BOLD)
}

pub(crate) fn italic(tok: Tok) -> Style {
    fg(tok).add_modifier(Modifier::ITALIC)
}

/// A piece of text in its style; a line is several, one after the other.
pub(crate) type Seg = (String, Style);

pub(crate) fn seg(text: impl Into<String>, style: Style) -> Seg {
    (text.into(), style)
}

/// A pill, `▐text▌`: the text bold on `bg`, its ends half blocks in `bg` over `on`.
pub(crate) fn pill(text: &str, bg: Tok, text_tok: Tok, on: Tok) -> Vec<Seg> {
    vec![
        seg("▐", fg(bg).bg(on.color())),
        seg(text, bold(text_tok).bg(bg.color())),
        seg("▌", fg(bg).bg(on.color())),
    ]
}

/// A keycap, ` k `: bold text on `RAISED`, or lit (accent on `FOCUS_SEL`) while the prefix waits.
pub(crate) fn key(name: &str, lit: bool) -> Seg {
    let style = if lit {
        bold(theme::ACCENT).bg(theme::FOCUS_SEL.color())
    } else {
        bold(theme::TEXT).bg(theme::RAISED.color())
    };
    seg(format!(" {name} "), style)
}

/// How many cells `text` takes on screen.
pub(crate) fn width(text: &str) -> u16 {
    u16::try_from(text.width()).unwrap_or(u16::MAX)
}

pub(crate) fn segs_width(segs: &[Seg]) -> u16 {
    segs.iter()
        .map(|(text, _)| width(text))
        .fold(0, u16::saturating_add)
}

/// `text` cut to at most `cells` cells, ending in `…` when it did not fit.
pub(crate) fn trunc(text: &str, cells: u16) -> String {
    if width(text) <= cells {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = u16::try_from(c.width().unwrap_or(0)).unwrap_or(u16::MAX);
        if used + w + 1 > cells {
            break;
        }
        out.push(c);
        used += w;
    }
    if cells > 0 {
        out.truncate(out.trim_end().len());
        out.push('…');
    }
    out
}

/// The words of `text` in lines of at most `cells` cells; a word longer than a line is cut.
pub(crate) fn wrap(text: &str, cells: u16) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split_whitespace() {
        if !line.is_empty() && width(&line) + 1 + width(word) > cells {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    if !line.is_empty() {
        lines.push(line);
    }
    lines.into_iter().map(|line| trunc(&line, cells)).collect()
}

/// The design's painter over a ratatui buffer. Nothing is painted outside the buffer.
#[derive(Debug)]
pub(crate) struct Painter<'a> {
    pub(crate) buf: &'a mut Buffer,
}

impl Painter<'_> {
    /// Blank cells on `bg`.
    pub(crate) fn fill(&mut self, area: Rect, bg: Tok) {
        for position in area.intersection(self.buf.area).positions() {
            let cell = &mut self.buf[position];
            cell.reset();
            cell.set_fg(theme::TEXT.color()).set_bg(bg.color());
        }
    }

    /// Only the background of the cells.
    pub(crate) fn tint(&mut self, area: Rect, bg: Tok) {
        for position in area.intersection(self.buf.area).positions() {
            self.buf[position].set_bg(bg.color());
        }
    }

    /// `text` from column `x`, in at most `max` cells. The text colour is the style's, else TEXT;
    /// the background stays unless the style has one; the attributes are exactly the style's.
    /// Returns the column after the text.
    pub(crate) fn put(&mut self, x: u16, y: u16, text: &str, style: Style, max: u16) -> u16 {
        let area = self.buf.area;
        if y < area.top() || y >= area.bottom() || x < area.left() || x >= area.right() {
            return x.saturating_add(width(text).min(max));
        }
        let style = Style {
            fg: style.fg.or(Some(theme::TEXT.color())),
            sub_modifier: Modifier::all().difference(style.add_modifier),
            ..style
        };
        let room = usize::from(max.min(area.right() - x));
        let (end, _) = self.buf.set_stringn(x, y, text, room, style);
        end
    }

    pub(crate) fn text(&mut self, x: u16, y: u16, text: &str, style: Style) -> u16 {
        self.put(x, y, text, style, u16::MAX)
    }

    /// Several pieces one after the other, together in at most `max` cells.
    pub(crate) fn segs(&mut self, x: u16, y: u16, segs: &[Seg], max: u16) -> u16 {
        let end = x.saturating_add(max);
        let mut at = x;
        for (text, style) in segs {
            if at >= end {
                break;
            }
            at = self.put(at, y, text, *style, end - at);
        }
        at
    }

    pub(crate) fn line(&mut self, x: u16, y: u16, segs: &[Seg]) -> u16 {
        self.segs(x, y, segs, u16::MAX)
    }

    /// Text ending just before column `end`. Returns the column it starts at.
    pub(crate) fn rtext(&mut self, end: u16, y: u16, text: &str, style: Style) -> u16 {
        let x = end.saturating_sub(width(text));
        self.text(x, y, text, style);
        x
    }

    /// Pieces ending just before column `end`. Returns the column they start at.
    pub(crate) fn rsegs(&mut self, end: u16, y: u16, segs: &[Seg]) -> u16 {
        let x = end.saturating_sub(segs_width(segs));
        self.line(x, y, segs);
        x
    }

    /// A card: its body filled with `bg`, and its top and bottom rows half blocks in `bg` over
    /// `under` (`▗▄▄▖`, `▝▀▀▘`). `stroke` cells of the accent `▁` over its title, from the third
    /// column, say the keys go to it; 0 draws none.
    pub(crate) fn card(&mut self, area: Rect, bg: Tok, under: Tok, stroke: u16) {
        if area.width < 2 || area.height < 2 {
            return;
        }
        let inner = usize::from(area.width - 2);
        let edge = fg(bg).bg(under.color());
        let bottom = area.bottom() - 1;
        self.fill(
            Rect::new(area.x, area.y + 1, area.width, area.height - 2),
            bg,
        );
        self.text(area.x, area.y, &format!("▗{}▖", "▄".repeat(inner)), edge);
        self.text(area.x, bottom, &format!("▝{}▘", "▀".repeat(inner)), edge);
        if stroke > 0 {
            let line = "▁".repeat(usize::from(stroke));
            self.text(
                area.x + 2,
                area.y,
                &line,
                fg(theme::ACCENT).bg(under.color()),
            );
        }
    }
}

/// The frame as a terminal in `mode` gets it.
pub(crate) fn finish(buf: &mut Buffer, mode: Mode) {
    match mode {
        Mode::Truecolor => {}
        Mode::Ansi256 => {
            for cell in &mut buf.content {
                cell.fg = fallback(cell.fg);
                cell.bg = fallback(cell.bg);
            }
        }
        Mode::Mono => buf.content.iter_mut().for_each(mono),
    }
}

/// The ANSI-256 fallback of a token's colour; any other colour (an agent's own) unchanged.
fn fallback(colour: Color) -> Color {
    theme::TOKENS
        .iter()
        .find(|tok| tok.color() == colour)
        .map_or(colour, |tok| Color::Indexed(tok.ansi))
}

/// `NO_COLOR`, the design's `mono`: no colour; where the keys go (the cursor, a focused pill, a lit
/// keycap, the whole status bar while the prefix waits) reversed and bold; accent text bold and
/// underlined; secondary text dim; half blocks blank, except the `▄▀` around the open worktree,
/// which become rules.
fn mono(cell: &mut Cell) {
    let (fg_was, bg_was) = (cell.fg, cell.bg);
    let is = |colour: Color, tok: Tok| colour == tok.color();
    cell.fg = Color::Reset;
    cell.bg = Color::Reset;
    let focus = [theme::FOCUS_SEL, theme::ACCENT, theme::FOCUS_BG];
    if focus.iter().any(|tok| is(bg_was, *tok)) {
        cell.modifier |= Modifier::REVERSED | Modifier::BOLD;
        return;
    }
    let symbol = cell.symbol().to_owned();
    if ["▄", "▀", "▗", "▖", "▝", "▘", "▐", "▌"].contains(&symbol.as_str()) {
        let rule = is(fg_was, theme::SELECTION) && (symbol == "▄" || symbol == "▀");
        cell.set_symbol(if rule { "─" } else { " " });
        cell.modifier = Modifier::DIM;
        return;
    }
    if is(fg_was, theme::ACCENT) {
        let stroke = symbol == "▁" || symbol == "▎";
        cell.modifier |= if stroke {
            Modifier::BOLD
        } else {
            Modifier::BOLD | Modifier::UNDERLINED
        };
    } else if is(fg_was, theme::TEXT_2) || is(fg_was, theme::BORDER) {
        cell.modifier |= Modifier::DIM;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_colour_mode_follows_no_color_then_colorterm() {
        assert_eq!(Mode::detect(true, Some("truecolor")), Mode::Mono);
        assert_eq!(Mode::detect(false, Some("truecolor")), Mode::Truecolor);
        assert_eq!(Mode::detect(false, Some("24bit")), Mode::Truecolor);
        assert_eq!(Mode::detect(false, None), Mode::Ansi256);
        assert_eq!(Mode::detect(false, Some("yes")), Mode::Ansi256);
    }

    #[test]
    fn text_is_cut_to_its_cells_with_an_ellipsis() {
        assert_eq!(trunc("fix-login", 20), "fix-login");
        assert_eq!(trunc("fix-login", 5), "fix-…");
        assert_eq!(
            trunc("Running the suite", 13),
            "Running the…",
            "no space before it"
        );
        assert_eq!(
            trunc("日本語です", 5),
            "日本…",
            "wide characters take two cells"
        );
        assert_eq!(trunc("abc", 0), "");
    }

    #[test]
    fn text_wraps_by_words() {
        assert_eq!(
            wrap("the project's directory is gone", 14),
            ["the project's", "directory is", "gone"]
        );
        assert_eq!(wrap("averyveryverylongword", 6), ["avery…"]);
    }

    #[test]
    fn a_token_falls_back_to_its_256_colour_and_other_colours_stay() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 2, 1));
        buf[(0, 0)].set_fg(theme::ACCENT.color());
        buf[(1, 0)].set_fg(Color::Rgb(1, 2, 3));
        finish(&mut buf, Mode::Ansi256);
        assert_eq!(buf[(0, 0)].fg, Color::Indexed(147));
        assert_eq!(buf[(1, 0)].fg, Color::Rgb(1, 2, 3));
    }

    #[test]
    fn no_color_keeps_only_attributes() {
        let mut buf = Buffer::empty(Rect::new(0, 0, 3, 1));
        buf[(0, 0)].set_bg(theme::FOCUS_SEL.color());
        buf[(1, 0)].set_symbol("▀").set_fg(theme::SELECTION.color());
        buf[(2, 0)].set_fg(theme::TEXT_2.color());
        finish(&mut buf, Mode::Mono);
        assert!(buf[(0, 0)].modifier.contains(Modifier::REVERSED));
        assert_eq!(buf[(1, 0)].symbol(), "─");
        assert!(buf[(2, 0)].modifier.contains(Modifier::DIM));
        assert!(buf.content.iter().all(|cell| cell.fg == Color::Reset));
    }
}
