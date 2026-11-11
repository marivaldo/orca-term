//! App: the side menu's card (#174): `Projects`, then every project on the list with its initial
//! and name, and its worktrees under it, three lines each (two under 40 rows): the state glyph, the
//! name and the unseen mark; the branch and the pull request; the agent's last message and its age.
//! The open worktree sits in a half-line card; with the focus, the cursor's worktree takes the
//! accent. A project with no worktrees says so, and one that cannot be read shows why, muted.

use ratatui::layout::Rect;

use crate::app::menu;
use crate::app::paint::{Painter, Seg, bold, fg, italic, key, pill, seg, trunc, width, wrap};
use crate::app::state::{App, Focus};
use crate::app::theme::{
    self, ACCENT, CANVAS, DONE, FOCUS_SEL, PANEL, RAISED, SELECTION, TEXT, TEXT_2,
};
use crate::domain::fleet::Worktree;
use crate::domain::project::ProjectFleet;

/// The card's title.
const TITLE: &str = "Projects";

/// The most lines a project's error takes.
const ERROR_LINES: usize = 3;

/// One thing of the list, and the row it starts at, counted from the list's top.
#[derive(Debug)]
struct Placed<'a> {
    row: u16,
    part: Part<'a>,
}

#[derive(Debug)]
enum Part<'a> {
    /// A project's initial and name.
    Header(&'a ProjectFleet),
    /// A worktree, in its lines.
    Item(&'a Worktree),
    /// A muted line: `no worktrees yet`, or a line of a project's error.
    Note(String),
}

/// What the side menu draws, given to every part.
struct Menu<'a> {
    app: &'a App,
    card: Rect,
    focused: bool,
    /// Lines per worktree: 3, or 2 on a short screen.
    lines: u16,
    now_ms: u64,
}

pub(crate) fn draw(p: &mut Painter<'_>, app: &App, card: Rect, now_ms: u64) {
    let focused = app.focus == Focus::SideMenu;
    p.card(card, PANEL, CANVAS, if focused { width(TITLE) } else { 0 });
    let title = if focused { ACCENT } else { TEXT_2 };
    p.text(card.x + 2, card.y + 1, TITLE, bold(title));
    let menu = Menu {
        app,
        card,
        focused,
        lines: if card.height < 40 { 2 } else { 3 },
        now_ms,
    };
    let top = card.y + 3;
    // Nothing reaches the bottom edge, nor the footer's rows with the focus.
    let bottom = card.bottom().saturating_sub(if focused { 3 } else { 1 });
    let room = bottom.saturating_sub(top);
    let placed = place(app, card.width.saturating_sub(8), menu.lines);
    let offset = scroll(&placed, &menu, room);
    for Placed { row, part } in &placed {
        let Some(row) = row.checked_sub(offset) else {
            continue;
        };
        if row + height(part, menu.lines) > room {
            break;
        }
        draw_part(p, &menu, part, top + row);
    }
    if focused {
        let hints = [
            key("j k", false),
            seg(" move  ", fg(TEXT_2)),
            key("enter", false),
            seg(" open", fg(TEXT_2)),
        ];
        p.segs(
            card.x + 2,
            card.bottom() - 2,
            &hints,
            card.width.saturating_sub(4),
        );
    }
}

/// Every part of the list and its row: a project's header, a blank row, its worktrees with a
/// blank row after each (or its note), and one more blank row between projects. `room` is how
/// many cells a note's line may take.
fn place(app: &App, room: u16, lines: u16) -> Vec<Placed<'_>> {
    let mut placed = Vec::new();
    let mut row = 0;
    for project in &app.fleets {
        placed.push(Placed {
            row,
            part: Part::Header(project),
        });
        row += 2;
        let notes = match &project.fleet {
            Err(err) => wrap(&format!("{err:#}"), room),
            Ok(fleet) if fleet.worktrees.is_empty() => vec!["no worktrees yet".to_owned()],
            Ok(fleet) => {
                for worktree in &fleet.worktrees {
                    placed.push(Placed {
                        row,
                        part: Part::Item(worktree),
                    });
                    row += lines + 1;
                }
                Vec::new()
            }
        };
        let notes = at_most(notes, ERROR_LINES, room);
        let shown = notes.len();
        for note in notes {
            placed.push(Placed {
                row,
                part: Part::Note(note),
            });
            row += 1;
        }
        row += u16::from(shown > 0) + 1;
    }
    placed
}

/// The first `most` of `lines`, the last of them ending in `…` when some were left out, in `room`
/// cells.
fn at_most(mut lines: Vec<String>, most: usize, room: u16) -> Vec<String> {
    if lines.len() > most {
        lines.truncate(most);
        if let Some(last) = lines.last_mut() {
            let more = format!("{last}…");
            *last = if width(&more) <= room {
                more
            } else {
                trunc(&more, room)
            };
        }
    }
    lines
}

fn height(part: &Part<'_>, lines: u16) -> u16 {
    match part {
        Part::Item(_) => lines,
        Part::Header(_) | Part::Note(_) => 1,
    }
}

/// How many rows the list scrolls by, so the worktree with the cursor (with the focus) or the open
/// one shows whole, with its card's bottom edge, in `room` rows. On the last worktree the list
/// scrolls to its end, so what follows it (a project with none, or one that cannot be read) can
/// be seen too.
fn scroll(placed: &[Placed<'_>], menu: &Menu<'_>, room: u16) -> u16 {
    let target = if menu.focused {
        menu.app.cursor.as_deref()
    } else {
        menu.app.open.as_deref()
    };
    let is_target = |placed: &&Placed<'_>| matches!(placed.part, Part::Item(worktree) if Some(worktree.path.as_path()) == target);
    let Some(at) = placed.iter().position(|placed| is_target(&placed)) else {
        return 0;
    };
    let last_item = !placed[at + 1..]
        .iter()
        .any(|placed| matches!(placed.part, Part::Item(_)));
    let end = if last_item {
        placed
            .last()
            .map_or(0, |last| last.row + height(&last.part, menu.lines))
    } else {
        placed[at].row + menu.lines + 1
    };
    end.saturating_sub(room)
}

fn draw_part(p: &mut Painter<'_>, menu: &Menu<'_>, part: &Part<'_>, row: u16) {
    let x = menu.card.x;
    match part {
        Part::Header(project) => {
            let name = menu::project_name(&project.path);
            let initial: String = name.chars().take(1).collect();
            p.line(x + 1, row, &pill(&initial, RAISED, TEXT, PANEL));
            let colour = if project.fleet.is_ok() { TEXT } else { TEXT_2 };
            let room = menu.card.width.saturating_sub(7);
            p.text(x + 5, row, &trunc(&name, room), bold(colour));
        }
        Part::Note(text) => {
            p.text(x + 6, row, text, italic(TEXT_2));
        }
        Part::Item(worktree) => item(p, menu, worktree, row),
    }
}

/// One worktree's lines.
fn item(p: &mut Painter<'_>, menu: &Menu<'_>, worktree: &Worktree, row: u16) {
    let (x, w) = (menu.card.x, menu.card.width);
    let open = menu.app.open.as_ref() == Some(&worktree.path);
    let cursor = menu.focused && menu.app.cursor.as_ref() == Some(&worktree.path);
    if cursor || open {
        highlight(p, menu, row, cursor);
    }
    let (glyph, state) = theme::look(&worktree.state, menu.app.frame);
    p.text(x + 4, row, &glyph.to_string(), bold(state));
    let unseen = worktree.unseen && !open;
    let room = w.saturating_sub(8 + if unseen { 2 } else { 0 });
    let name_colour = if cursor { ACCENT } else { TEXT };
    let after = p.text(x + 6, row, &trunc(&worktree.name, room), bold(name_colour));
    if unseen {
        p.text(after + 1, row, &theme::UNSEEN.to_string(), bold(state));
    }
    let end = x + w - 2;
    if menu.lines == 2 {
        last_line(p, menu, worktree, row + 1, &[]);
        return;
    }
    let pr = menu::pr_number(worktree).map(|number| format!("#{number}"));
    let pr_width = pr.as_deref().map_or(0, |pr| width(pr) + 1);
    let branch = worktree
        .branch
        .as_ref()
        .map_or("detached", |branch| branch.as_str());
    let room = w.saturating_sub(8 + pr_width);
    p.text(x + 6, row + 1, &trunc(branch, room), fg(TEXT_2));
    if let Some(pr) = &pr {
        p.rtext(end, row + 1, pr, bold(DONE));
    }
    let mut lead = Vec::new();
    if let Some((glyph, tok)) = menu::checks(worktree).and_then(theme::check_look) {
        lead.push(seg(format!("{glyph} "), bold(tok)));
    }
    if let Some(agent) = &worktree.agent {
        lead.push(seg(
            format!("{} · ", menu::agent_label(agent)),
            italic(TEXT_2),
        ));
    }
    last_line(p, menu, worktree, row + 2, &lead);
}

/// A worktree's last line: `lead`, then its agent's last message, cut to fit, and its age on the
/// right.
fn last_line(p: &mut Painter<'_>, menu: &Menu<'_>, worktree: &Worktree, row: u16, lead: &[Seg]) {
    let (x, end) = (menu.card.x, menu.card.x + menu.card.width - 2);
    let age = worktree
        .last_activity
        .map(|then| menu::age(then, menu.now_ms))
        .unwrap_or_default();
    let after = p.line(x + 6, row, lead);
    let room = end.saturating_sub(after + width(&age) + u16::from(!age.is_empty()));
    p.text(
        after,
        row,
        &trunc(&menu::message(worktree), room),
        fg(TEXT_2),
    );
    p.rtext(end, row, &age, fg(TEXT_2));
}

/// The half-line card around the open worktree (SELECTION), or around the cursor's with the focus
/// (`FOCUS_SEL`, an accent bar on its left).
fn highlight(p: &mut Painter<'_>, menu: &Menu<'_>, row: u16, cursor: bool) {
    let (x, inner) = (menu.card.x + 1, menu.card.width.saturating_sub(2));
    let bg = if cursor { FOCUS_SEL } else { SELECTION };
    p.tint(Rect::new(x, row, inner, menu.lines), bg);
    let edge = fg(bg).bg(PANEL.color());
    p.text(x, row - 1, &"▄".repeat(usize::from(inner)), edge);
    p.text(x, row + menu.lines, &"▀".repeat(usize::from(inner)), edge);
    if cursor {
        for line in 0..menu.lines {
            p.text(x, row + line, "▎", fg(ACCENT).bg(bg.color()));
        }
    }
}
