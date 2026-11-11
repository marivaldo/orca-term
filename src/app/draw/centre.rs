//! App: the centre's cards (#174, #176), one per panel shown. Each has a tab row: alone, the
//! three tabs `1 agent  2 nvim  3 shell` with the open worktree and its agent on the right, or,
//! where the right column is not a column, its summary `4 files +55 −21` and `^a f` (#177); in the
//! split, `1 agent` on the left with the agent's name, and `2 nvim  3 shell` on the right; zoomed,
//! the panel's own tab with `zoom` and `^a z restore` (and, from the split, which half it shows).
//! The active tab is a pill, in the accent with the focus, under the accent's stroke. Under the
//! tab row, the panel's terminal, or what to do where nothing runs.

use ratatui::layout::Rect;

use crate::app::layout::Card;
use crate::app::menu;
use crate::app::paint::{Painter, Seg, bold, fg, italic, key, pill, seg, segs_width, width};
use crate::app::panel::Status;
use crate::app::program::{self, Kind};
use crate::app::state::{App, Focus};
use crate::app::theme::{
    ACCENT, BG, BORDER, CANVAS, DONE, FAILED, FOCUS_SEL, SELECTION, TEXT, TEXT_2,
};
use crate::app::view::Tab;

pub(crate) fn draw(p: &mut Painter<'_>, app: &App) {
    for card in &app.layout.cards {
        draw_card(p, app, card);
    }
}

fn draw_card(p: &mut Painter<'_>, app: &App, card: &Card) {
    let area = card.area;
    let focused = app.focus == Focus::Centre && app.view.tab == card.tab;
    p.card(area, BG, CANVAS, 0);
    let (tabs, right) = header(app, card.tab);
    let after_tabs = tab_row(p, area, &tabs, card.tab, focused);
    let end = area.right() - 2;
    if after_tabs + 2 + segs_width(&right) <= end {
        p.rsegs(end, area.y + 1, &right);
    }
    match Kind::of(card.tab) {
        None => agent_body(p, app, card.body),
        Some(kind) => program_body(p, app, kind, card.body),
    }
}

/// The tabs a card shows and what goes on the right of its tab row.
fn header(app: &App, tab: Tab) -> (Vec<Tab>, Vec<Seg>) {
    let view = app.view;
    if view.zoom {
        return (vec![tab], zoom_note(app));
    }
    match (view.split, tab) {
        (true, Tab::Agent) => (vec![Tab::Agent], agent_name(app)),
        (true, _) => (vec![Tab::Nvim, Tab::Shell], Vec::new()),
        (false, _) if app.layout.right.is_some_and(|right| !right.overlay) => {
            (Tab::ALL.to_vec(), location(app))
        }
        (false, _) => (Tab::ALL.to_vec(), summary(app)),
    }
}

/// `4 files +55 −21  ^a f`: the open worktree's changes, where the right column does not show
/// beside the centre, and the key that shows it.
fn summary(app: &App) -> Vec<Seg> {
    let mut out = Vec::new();
    if let Some(changes) = app.open_changes() {
        let lines = changes.totals();
        let count = changes.files.len();
        let files = if count == 1 { "file" } else { "files" };
        out.extend([
            seg(format!("{count} {files} "), fg(TEXT_2)),
            seg(format!("+{}", lines.added), fg(DONE)),
            seg(format!(" −{}  ", lines.deleted), fg(FAILED)),
        ]);
    }
    out.push(key("^a f", false));
    out
}

/// Draws `tabs` on the card's tab row, `active` as a pill, and the accent's stroke over it when
/// `focused`. Returns the column after the last tab.
fn tab_row(p: &mut Painter<'_>, area: Rect, tabs: &[Tab], active: Tab, focused: bool) -> u16 {
    let y = area.y + 1;
    let mut x = area.x + 2;
    for (at, tab) in tabs.iter().enumerate() {
        if at > 0 {
            x += 1;
        }
        let label = tab.label();
        if *tab != active {
            x = p.text(x, y, &format!(" {label} "), fg(TEXT_2));
            continue;
        }
        let (pill_bg, pill_text) = if focused {
            (FOCUS_SEL, ACCENT)
        } else {
            (SELECTION, TEXT)
        };
        let start = x;
        x = p.line(x, y, &pill(label, pill_bg, pill_text, BG));
        if focused {
            let stroke = "▁".repeat(usize::from(width(label) + 2));
            p.text(start, area.y, &stroke, fg(ACCENT).bg(CANVAS.color()));
        }
    }
    x
}

/// `project/worktree  ·  claude`, the open worktree and its agent.
fn location(app: &App) -> Vec<Seg> {
    let Some((project, worktree)) = app
        .open
        .as_deref()
        .and_then(|open| menu::find(&app.fleets, open))
    else {
        return Vec::new();
    };
    let name = format!("{}/{}", menu::project_name(&project.path), worktree.name);
    let mut place = vec![seg(name, fg(TEXT_2))];
    if let Some(agent) = &worktree.agent {
        place.push(seg("  ·  ", fg(BORDER)));
        place.push(seg(menu::agent_label(agent), italic(TEXT_2)));
    }
    place
}

/// The open worktree's agent, `claude`, as the split's agent card says it.
fn agent_name(app: &App) -> Vec<Seg> {
    app.open
        .as_deref()
        .and_then(|open| menu::find(&app.fleets, open))
        .and_then(|(_, worktree)| worktree.agent.as_deref())
        .map(|agent| vec![seg(menu::agent_label(agent), italic(TEXT_2))])
        .unwrap_or_default()
}

/// `split  agent | nvim    ▐zoom▌  ^a z restore`: the zoom, the split it came from with the
/// panel shown in the accent, and the key that restores it.
fn zoom_note(app: &App) -> Vec<Seg> {
    let view = app.view;
    let mut note = Vec::new();
    if view.split {
        let lit = |tab: Tab| {
            if view.tab == tab {
                bold(ACCENT)
            } else {
                fg(TEXT_2)
            }
        };
        note.extend([
            seg("split  ", fg(TEXT_2)),
            seg(Tab::Agent.name(), lit(Tab::Agent)),
            seg(" | ", fg(BORDER)),
            seg(view.side.name(), lit(view.side)),
            seg("    ", fg(TEXT)),
        ]);
    }
    note.extend(pill("zoom", FOCUS_SEL, ACCENT, BG));
    note.extend([
        seg("  ", fg(TEXT)),
        key("^a z", false),
        seg(" restore", fg(TEXT_2)),
    ]);
    note
}

/// The agent's name to start: the one the open worktree recorded, else claude.
fn agent_to_start(app: &App) -> String {
    app.open
        .as_deref()
        .and_then(|open| menu::find(&app.fleets, open))
        .and_then(|(_, worktree)| worktree.agent.as_deref())
        .map_or("claude", menu::agent_label)
        .to_owned()
}

/// What the agent's panel shows: the agent's screen, or what to do where none runs.
fn agent_body(p: &mut Painter<'_>, app: &App, body: Rect) {
    let Some(panel) = &app.panel else {
        let hint = "no worktree to show: add a project with `orca-term project add`";
        p.put(
            body.x + 1,
            body.y,
            hint,
            fg(TEXT_2),
            body.width.saturating_sub(1),
        );
        return;
    };
    let agent = agent_to_start(app);
    let line = match &panel.status {
        Status::Attached => {
            panel.screen.paint(p.buf, body);
            return;
        }
        Status::NoAgent => vec![
            seg("no agent running  ·  ", fg(TEXT_2)),
            key("enter", false),
            seg(format!(" starts {agent}"), fg(TEXT_2)),
        ],
        Status::Starting => vec![seg(format!("starting {agent}…"), fg(TEXT_2))],
        Status::Ended(how) => {
            panel.screen.paint(p.buf, body);
            last_line(p, body, &format!("agent ended ({how})"), "starts it again");
            return;
        }
    };
    p.segs(body.x + 1, body.y, &line, body.width.saturating_sub(1));
}

/// What a Neovim's or a shell's panel shows: its screen, how it ended, or why it did not start.
fn program_body(p: &mut Painter<'_>, app: &App, kind: Kind, body: Rect) {
    let Some(program) = app.open.as_ref().and_then(|worktree| {
        app.programs.get(&program::ProgramId {
            worktree: worktree.clone(),
            kind,
        })
    }) else {
        return;
    };
    let name = kind.name();
    match &program.status {
        program::Status::Running => program.screen.paint(p.buf, body),
        program::Status::Ended(how) => {
            program.screen.paint(p.buf, body);
            last_line(p, body, &format!("{name} ended ({how})"), "starts it again");
        }
        program::Status::Failed(why) => {
            let line = [
                seg(format!("could not start {name}: {why}  ·  "), fg(TEXT_2)),
                key("enter", false),
                seg(" tries again", fg(TEXT_2)),
            ];
            p.segs(body.x + 1, body.y, &line, body.width.saturating_sub(1));
        }
    }
}

/// Over the last row of `body`: `what  ·  enter <again>`.
fn last_line(p: &mut Painter<'_>, body: Rect, what: &str, again: &str) {
    let last = Rect::new(body.x, body.bottom().saturating_sub(1), body.width, 1);
    p.fill(last, BG);
    p.segs(
        body.x + 1,
        last.y,
        &[
            seg(format!("{what}  ·  "), fg(TEXT_2)),
            key("enter", false),
            seg(format!(" {again}"), fg(TEXT_2)),
        ],
        body.width.saturating_sub(1),
    );
}

/// Where the terminal's cursor goes: the focused panel's own, when the keys go to it.
pub(crate) fn cursor(app: &App) -> Option<(u16, u16)> {
    if app.focus != Focus::Centre || app.prefix.is_some() || app.layout.too_small {
        return None;
    }
    let body = app.layout.card(app.view.tab)?.body;
    match Kind::of(app.view.tab) {
        None => {
            let panel = app.panel.as_ref()?;
            (panel.status == Status::Attached)
                .then(|| panel.screen.cursor(body))
                .flatten()
        }
        Some(kind) => {
            let id = program::ProgramId {
                worktree: app.open.clone()?,
                kind,
            };
            let program = app.programs.get(&id)?;
            program
                .running()
                .then(|| program.screen.cursor(body))
                .flatten()
        }
    }
}
