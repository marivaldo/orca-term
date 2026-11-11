//! App: a failed agent in the centre (#178, the design's 4e): the end of its last output, and under
//! it the application's own card, `✗  agent stopped  ·  exit 1 after 41m  ·  12m ago`, a line
//! saying the changes are kept, and the keys, `r` lit as the one to press; or, after `l`, its log
//! filling the panel with how to scroll it.

use ratatui::layout::Rect;

use crate::app::emulator::Emulator;
use crate::app::menu;
use crate::app::paint::{Painter, bold, fg, key, seg, trunc};
use crate::app::state::{App, Log};
use crate::app::theme::{BG, FAILED, SELECTION, TEXT, TEXT_2};
use crate::domain::agent::AgentEnd;

/// The card's height, edges included, and the rows left under it.
const CARD_ROWS: u16 = 6;
const UNDER_CARD: u16 = 2;

/// The failed agent's panel: the end of its output in `body`, and the card under it.
pub(crate) fn draw(p: &mut Painter<'_>, app: &App, end: &AgentEnd, body: Rect, now_ms: u64) {
    p.fill(body, BG);
    let top = body
        .bottom()
        .saturating_sub(CARD_ROWS + UNDER_CARD)
        .max(body.y);
    let output = Rect::new(body.x, body.y, body.width, top - body.y);
    if let Some(screen) = last_output(app) {
        screen.paint_end(p.buf, output);
    }
    let card = Rect::new(body.x, top, body.width, CARD_ROWS.min(body.bottom() - top));
    p.card(card, SELECTION, BG, 0);
    let x = body.x;
    let room = body.width.saturating_sub(7);
    let ran = menu::span(end.ended_at.saturating_sub(end.started_at));
    let ago = match menu::age(end.ended_at, now_ms).as_str() {
        "now" => "just now".to_owned(),
        age => format!("{age} ago"),
    };
    p.segs(
        x + 3,
        top + 1,
        &[
            seg("✗", bold(FAILED)),
            seg("  agent stopped", bold(TEXT)),
            seg(
                format!("  ·  {} after {ran}  ·  {ago}", end.exit),
                fg(TEXT_2),
            ),
        ],
        body.width.saturating_sub(4),
    );
    p.put(x + 6, top + 2, &trunc(&kept(app), room), fg(TEXT_2), room);
    p.segs(
        x + 6,
        top + 4,
        &[
            key("r", true),
            seg(" resume session   ", fg(TEXT_2)),
            key("R", false),
            seg(" restart clean   ", fg(TEXT_2)),
            key("l", false),
            seg(" show log   ", fg(TEXT_2)),
            key("x", false),
            seg(" dismiss", fg(TEXT_2)),
        ],
        room,
    );
}

/// The screen holding the agent's last output: its log once read, else what the panel showed.
fn last_output(app: &App) -> Option<&Emulator> {
    let log = app
        .log
        .as_ref()
        .filter(|log| app.open.as_ref() == Some(&log.worktree))
        .and_then(|log| log.screen.as_ref());
    log.or_else(|| app.panel.as_ref().map(|panel| &panel.screen))
}

/// That the changes are kept, with how many files and lines changed, as the right column read
/// them (#177), else how many files the core counted, when either is known and not none.
fn kept(app: &App) -> String {
    let plural = |n: usize| if n == 1 { "" } else { "s" };
    let files = match app.open_changes() {
        Some(changes) if changes.files.is_empty() => String::new(),
        Some(changes) => {
            let (n, lines) = (changes.files.len(), changes.totals());
            format!(
                " ({n} file{}, +{} −{})",
                plural(n),
                lines.added,
                lines.deleted
            )
        }
        None => app
            .open
            .as_deref()
            .and_then(|open| menu::find(&app.fleets, open))
            .and_then(|(_, worktree)| worktree.changed_files)
            .filter(|n| *n > 0)
            .map(|n| format!(" ({n} file{})", plural(n)))
            .unwrap_or_default(),
    };
    format!(
        "Your changes are safe in the worktree{files}. The session can resume from its last turn."
    )
}

/// The log filling `body`, and on its last row how far back it shows and its keys.
pub(crate) fn log(p: &mut Painter<'_>, log: &Log, body: Rect) {
    p.fill(body, BG);
    let last = body.bottom().saturating_sub(1);
    let Some(screen) = &log.screen else {
        p.put(
            body.x + 1,
            body.y,
            "reading the log…",
            fg(TEXT_2),
            body.width,
        );
        return;
    };
    screen.paint(p.buf, Rect::new(body.x, body.y, body.width, last - body.y));
    p.fill(Rect::new(body.x, last, body.width, 1), BG);
    let place = if log.up == 0 {
        "the end".to_owned()
    } else {
        format!("{} lines up", log.up)
    };
    p.segs(
        body.x + 1,
        last,
        &[
            seg("agent log", bold(TEXT)),
            seg(format!("  ·  {place}  ·  "), fg(TEXT_2)),
            key("j k", false),
            seg(" scroll  ", fg(TEXT_2)),
            key("g G", false),
            seg(" top end  ", fg(TEXT_2)),
            key("esc", false),
            seg(" close", fg(TEXT_2)),
        ],
        body.width.saturating_sub(1),
    );
}
