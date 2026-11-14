//! PROTOTYPE (#203, throwaway): the command palette and the key probe's overlay.

use ratatui::layout::Rect;

use crate::app::paint::{Painter, bold, fg, italic, key, seg, trunc, width};
use crate::app::proto::Palette;
use crate::app::state::App;
use crate::app::theme::{ACCENT, CANVAS, FOCUS_SEL, PANEL, TEXT, TEXT_2};

const TITLE: &str = "command palette";

pub(crate) fn palette(p: &mut Painter<'_>, palette: &Palette, screen: Rect) {
    let items = palette.matching();
    let rows = u16::try_from(items.len()).unwrap_or(20) + 8;
    let card = p.floating(screen, 64, rows);
    let (x, y, w) = (card.x, card.y, card.width);
    p.card(card, PANEL, CANVAS, width(TITLE));
    p.text(x + 2, y + 1, TITLE, bold(ACCENT));
    p.rtext(x + w - 2, y + 1, "⌘p", fg(TEXT_2));
    let after = p.text(x + 2, y + 3, "›", bold(ACCENT));
    let after = p.put(
        after + 1,
        y + 3,
        &palette.query,
        fg(TEXT),
        w.saturating_sub(8),
    );
    p.text(after, y + 3, "▏", fg(ACCENT));
    let last = card.bottom().saturating_sub(2);
    for (row, (at, (_, label))) in (y + 5..last).zip(items.iter().enumerate()) {
        let cursor = at == palette.cursor;
        if cursor {
            p.tint(Rect::new(x + 1, row, w - 2, 1), FOCUS_SEL);
        }
        let tok = if cursor { ACCENT } else { TEXT };
        p.put(x + 3, row, label, fg(tok), w.saturating_sub(6));
    }
    if items.is_empty() {
        p.text(x + 3, y + 5, "nothing matches", italic(TEXT_2));
    }
    p.segs(
        x + 2,
        last,
        &[
            key("↑↓", false),
            seg(" move  ", fg(TEXT_2)),
            key("enter", false),
            seg(" run  ", fg(TEXT_2)),
            key("esc", false),
            seg(" close", fg(TEXT_2)),
        ],
        w.saturating_sub(4),
    );
}

/// The last events crossterm read, and what the application did with each, in a card at the
/// bottom right. It takes no key.
pub(crate) fn probe(p: &mut Painter<'_>, app: &App, screen: Rect) {
    let events = &app.proto.events;
    let w = screen.width.saturating_sub(4).min(110);
    let h = u16::try_from(events.len()).unwrap_or(14) + 6;
    if screen.height < h + 2 || w < 30 {
        return;
    }
    let card = Rect::new(
        screen.width - w - 2,
        screen.height.saturating_sub(h + 2),
        w,
        h,
    );
    let (x, y) = (card.x, card.y);
    p.card(card, PANEL, CANVAS, 0);
    p.text(x + 2, y + 1, "key probe", bold(ACCENT));
    let head = format!(
        "kitty protocol probe: {}   super seen: {}   ⌘digit: {}   ⌘k: {}   spelling: {} → {}   mouse: {}",
        match app.proto.probe {
            Some(true) => "yes",
            Some(false) => "no",
            None => "no answer",
        },
        app.proto.super_seen,
        app.proto.super_digit_seen,
        app.proto.super_k_seen,
        app.proto.spelling.name(),
        if app.proto.spell_super() {
            "⌘"
        } else {
            "fallback"
        },
        if app.proto.mouse { "on" } else { "off" },
    );
    let room = w.saturating_sub(4);
    p.text(
        x + 14,
        y + 1,
        &trunc(&head, room.saturating_sub(12)),
        fg(TEXT_2),
    );
    for (row, line) in (y + 3..).zip(events.iter()) {
        p.put(x + 2, row, line, fg(TEXT), room);
    }
    if let Some(path) = &app.proto.log_path {
        let text = format!("log: {}   (F10 hides)", path.display());
        p.put(x + 2, card.bottom() - 2, &text, italic(TEXT_2), room);
    }
}
