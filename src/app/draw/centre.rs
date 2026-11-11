//! App: the centre's card (#174): the `1 agent` tab, where the open worktree is, and the agent's
//! panel, or how to start an agent where none runs.

use ratatui::layout::Rect;

use crate::app::menu;
use crate::app::paint::{Painter, Seg, fg, italic, key, pill, seg, segs_width, width};
use crate::app::panel::Status;
use crate::app::state::{App, Focus};
use crate::app::theme::{ACCENT, BG, BORDER, CANVAS, FOCUS_SEL, SELECTION, TEXT, TEXT_2};

/// The tab of the agent's panel.
const TAB: &str = "1 agent";

pub(crate) fn draw(p: &mut Painter<'_>, app: &App) {
    let card = app.layout.centre;
    let focused = app.focus == Focus::Centre;
    let stroke = if focused { width(TAB) + 2 } else { 0 };
    p.card(card, BG, CANVAS, stroke);
    let (pill_bg, pill_text) = if focused {
        (FOCUS_SEL, ACCENT)
    } else {
        (SELECTION, TEXT)
    };
    let after_tab = p.line(card.x + 2, card.y + 1, &pill(TAB, pill_bg, pill_text, BG));
    let place = location(app);
    let end = card.right() - 2;
    if after_tab + 2 + segs_width(&place) <= end {
        p.rsegs(end, card.y + 1, &place);
    }
    body(p, app, app.layout.body);
}

/// `project/worktree  ·  claude`, the open worktree and its agent, on the right of the tab row.
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

/// The agent's name to start: the one the open worktree recorded, else claude.
fn agent_to_start(app: &App) -> String {
    app.open
        .as_deref()
        .and_then(|open| menu::find(&app.fleets, open))
        .and_then(|(_, worktree)| worktree.agent.as_deref())
        .map_or("claude", menu::agent_label)
        .to_owned()
}

/// What the panel's area shows: the agent's screen, or what to do where none runs.
fn body(p: &mut Painter<'_>, app: &App, body: Rect) {
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
            panel.paint(p.buf, body);
            return;
        }
        Status::NoAgent => vec![
            seg("no agent running  ·  ", fg(TEXT_2)),
            key("enter", false),
            seg(format!(" starts {agent}"), fg(TEXT_2)),
        ],
        Status::Starting => vec![seg(format!("starting {agent}…"), fg(TEXT_2))],
        Status::Ended(how) => {
            panel.paint(p.buf, body);
            let last = Rect::new(body.x, body.bottom().saturating_sub(1), body.width, 1);
            p.fill(last, BG);
            p.segs(
                body.x + 1,
                last.y,
                &[
                    seg(format!("agent ended ({how})  ·  "), fg(TEXT_2)),
                    key("enter", false),
                    seg(" starts it again", fg(TEXT_2)),
                ],
                body.width.saturating_sub(1),
            );
            return;
        }
    };
    p.segs(body.x + 1, body.y, &line, body.width.saturating_sub(1));
}

/// Where the terminal's cursor goes: the agent's own, when the keys go to it.
pub(crate) fn cursor(app: &App) -> Option<(u16, u16)> {
    if app.focus != Focus::Centre || app.prefix.is_some() || app.layout.too_small {
        return None;
    }
    let panel = app.panel.as_ref()?;
    (panel.status == Status::Attached)
        .then(|| panel.cursor(app.layout.body))
        .flatten()
}
