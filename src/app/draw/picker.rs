//! App: the picker (#178), `^a w`, a card floating in the middle of the screen over everything
//! else, veiled: `switch worktree`, what was typed, then every project with its matching worktrees
//! one line each (state, name, branch, last message, age), the cursor's in the accent, and its keys
//! at the bottom. The design drew no picker; this one follows the prototype's.

use ratatui::layout::Rect;

use crate::app::menu;
use crate::app::paint::{Painter, bold, fg, italic, key, pill, seg, trunc, width};
use crate::app::state::{App, Picker};
use crate::app::theme::{self, ACCENT, FOCUS_SEL, PANEL, RAISED, TEXT, TEXT_2};
use crate::domain::fleet::Worktree;
use crate::domain::project::ProjectFleet;

/// The card's title, and the key that opens it.
const TITLE: &str = "switch worktree";
const OPENED_BY: &str = "^a w";

/// The widest the card gets.
const WIDTH: u16 = 64;

/// The rows of the card that are not the list: its edges, the title, the query, the footer and
/// the blank rows between them.
const FRAME_ROWS: u16 = 8;

/// One row of the list.
enum Row<'a> {
    Project(&'a ProjectFleet),
    Item(&'a Worktree),
    Note(&'static str),
    Blank,
}

pub(crate) fn draw(p: &mut Painter<'_>, app: &App, picker: &Picker, screen: Rect, now_ms: u64) {
    let rows = rows(app, &picker.query);
    let wanted = u16::try_from(rows.len()).unwrap_or(u16::MAX) + FRAME_ROWS;
    let card = p.floating(screen, WIDTH, wanted);
    let (x, y, w) = (card.x, card.y, card.width);
    p.card(card, PANEL, theme::CANVAS, width(TITLE));
    p.text(x + 2, y + 1, TITLE, bold(ACCENT));
    p.rtext(x + w - 2, y + 1, OPENED_BY, fg(TEXT_2));
    query(p, &picker.query, x, y + 3, w);

    let room = card.height.saturating_sub(FRAME_ROWS);
    let at = rows.iter().position(|row| {
        matches!(row, Row::Item(worktree) if picker.cursor.as_deref() == Some(worktree.path.as_path()))
    });
    let first = at.map_or(0, |at| (at + 1).saturating_sub(usize::from(room)));
    let shown = rows.iter().skip(first).take(usize::from(room));
    for (row, line) in (y + 5..).zip(shown) {
        match line {
            Row::Project(project) => {
                let name = menu::project_name(&project.path);
                let initial: String = name.chars().take(1).collect();
                p.line(x + 1, row, &pill(&initial, RAISED, TEXT, PANEL));
                p.text(x + 5, row, &trunc(&name, w.saturating_sub(7)), bold(TEXT));
            }
            Row::Item(worktree) => {
                let cursor = picker.cursor.as_deref() == Some(worktree.path.as_path());
                item(p, app, worktree, Rect::new(x, row, w, 1), cursor, now_ms);
            }
            Row::Note(text) => {
                p.text(x + 6, row, text, italic(TEXT_2));
            }
            Row::Blank => {}
        }
    }
    p.segs(
        x + 2,
        card.bottom().saturating_sub(2),
        &[
            key("↑↓", false),
            seg(" move  ", fg(TEXT_2)),
            key("enter", false),
            seg(" open  ", fg(TEXT_2)),
            key("esc", false),
            seg(" close", fg(TEXT_2)),
        ],
        w.saturating_sub(4),
    );
}

/// What was typed, after the accent's `›`, with a caret; a hint while nothing is.
fn query(p: &mut Painter<'_>, query: &str, x: u16, y: u16, w: u16) {
    let room = w.saturating_sub(7);
    let after = p.text(x + 2, y, "›", bold(ACCENT));
    let after = p.put(after + 1, y, query, fg(TEXT), room);
    let after = p.text(after, y, "▏", fg(ACCENT));
    if query.is_empty() {
        p.text(after, y, "type to filter", italic(TEXT_2));
    }
}

/// The list: each project that can be read with its worktrees matching `query`; with nothing
/// typed, a project with none says so, and with something typed, a project with no match is left
/// out.
fn rows<'a>(app: &'a App, query: &str) -> Vec<Row<'a>> {
    let mut rows = Vec::new();
    for project in &app.fleets {
        let Ok(fleet) = &project.fleet else {
            continue;
        };
        let matching: Vec<&Worktree> = fleet
            .worktrees
            .iter()
            .filter(|worktree| menu::matches(&project.path, worktree, query))
            .collect();
        if matching.is_empty() && !query.trim().is_empty() {
            continue;
        }
        if !rows.is_empty() {
            rows.push(Row::Blank);
        }
        rows.push(Row::Project(project));
        if matching.is_empty() {
            rows.push(Row::Note("no worktrees yet"));
        }
        rows.extend(matching.into_iter().map(Row::Item));
    }
    if rows.is_empty() {
        rows.push(Row::Note("nothing matches"));
    }
    rows
}

/// One worktree on one line of `line`: its state, name, branch, last message and age, in the
/// accent's cursor when `cursor`.
fn item(
    p: &mut Painter<'_>,
    app: &App,
    worktree: &Worktree,
    line: Rect,
    cursor: bool,
    now_ms: u64,
) {
    let (x, y, w) = (line.x, line.y, line.width);
    if cursor {
        p.tint(Rect::new(x + 1, y, w.saturating_sub(2), 1), FOCUS_SEL);
        p.text(x + 1, y, "▎", fg(ACCENT).bg(FOCUS_SEL.color()));
    }
    let (glyph, state) = theme::look(menu::state(worktree), app.frame);
    p.text(x + 4, y, &glyph.to_string(), bold(state));
    let name = if cursor { ACCENT } else { TEXT };
    p.text(x + 6, y, &trunc(&worktree.name, 14), bold(name));
    let age = menu::last_active(worktree)
        .map(|then| menu::age(then, now_ms))
        .unwrap_or_default();
    let end = x + w - 3;
    p.rtext(end, y, &age, fg(TEXT_2));
    let branch = worktree.branch.as_ref().map_or("detached", |b| b.as_str());
    if w >= 44 {
        p.text(x + 21, y, &trunc(branch, 18), fg(TEXT_2));
    }
    if w >= 56 {
        let room = end.saturating_sub(x + 40 + width(&age) + 1);
        p.text(
            x + 40,
            y,
            &trunc(&menu::message(worktree), room),
            fg(TEXT_2),
        );
    }
}
