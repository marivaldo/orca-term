//! App: help (#178), `^a ?`, every key in a card floating over the screen, veiled: the prefix's
//! keys first, then what each part takes without it. Any key closes it. The design drew no help;
//! this one follows the prototype's.

use ratatui::layout::Rect;

use crate::app::paint::{Painter, bold, fg, italic, key, trunc, width};
use crate::app::theme::{ACCENT, CANVAS, PANEL, TEXT_2};

/// The card's title.
const TITLE: &str = "keys";

/// The widest the card gets.
const WIDTH: u16 = 76;

/// The column the words start at, after the keycaps.
const WORDS_AT: u16 = 18;

/// Each key and what it does; a row with no key goes on with the row above, and an empty row
/// separates the prefix's keys from the rest.
const ROWS: &[(&str, &str)] = &[
    (
        "^a h l",
        "focus left / right: side menu, centre, right column",
    ),
    ("", "in the split agent and nvim; zoomed, stays zoomed"),
    ("^a j k", "next / previous worktree, opened at once"),
    ("^a 1 2 3", "the agent, nvim or shell tab"),
    ("^a z", "zoom the panel with the focus, and back"),
    ("^a |", "agent | nvim side by side, and back"),
    ("^a f", "show / hide the changed files (the right column)"),
    ("^a w", "the floating worktree picker"),
    ("^a P", "open a draft pull request"),
    ("^a a", "send Ctrl-a itself to the panel"),
    ("^a ?", "this help"),
    ("^a q", "quit (the agents keep running)"),
    ("esc", "cancel the prefix"),
    ("", ""),
    ("side menu", "j k or ↑ ↓ move, enter opens, n new, / filter"),
    (
        "right column",
        "j k move, enter opens the diff in nvim, esc leaves",
    ),
    (
        "agent failed",
        "r resume, R restart clean, l log, x dismiss",
    ),
    ("panels", "take every other key"),
];

pub(crate) fn draw(p: &mut Painter<'_>, screen: Rect) {
    let rows = u16::try_from(ROWS.len()).unwrap_or(u16::MAX) + 6;
    let card = p.floating(screen, WIDTH, rows);
    p.card(card, PANEL, CANVAS, width(TITLE));
    p.text(card.x + 2, card.y + 1, TITLE, bold(ACCENT));
    let last = card.bottom().saturating_sub(2);
    let room = card.width.saturating_sub(WORDS_AT + 2);
    for (row, (keys, what)) in (card.y + 3..last).zip(ROWS) {
        if !keys.is_empty() {
            p.line(card.x + 2, row, &[key(keys, false)]);
        }
        p.put(card.x + WORDS_AT, row, &trunc(what, room), fg(TEXT_2), room);
    }
    p.text(card.x + 2, last, "any key closes", italic(TEXT_2));
}
