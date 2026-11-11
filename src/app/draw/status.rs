//! App: the status bar (#174): the `orca` pill (`^A`, lit, while the prefix waits), the open
//! worktree and where the keys go, the counts of every project's worktrees (needs you, failed,
//! running, quiet), and on the right the keys, or a message for a while. While the prefix waits the
//! whole bar is lit and lists the keys it arms.

use crate::app::menu::{self, Counts};
use crate::app::paint::{Painter, Seg, bold, fg, key, pill, seg, segs_width, trunc, width};
use crate::app::state::{App, Focus};
use crate::app::theme::{
    self, ACCENT, BG, CANVAS, FAILED, FOCUS_BG, SELECTION, TEXT, TEXT_2, WAITING, WORKING,
};

/// The keys shown without the prefix, and with it.
const KEYS: &[(&str, &str)] = &[
    ("^a", "prefix"),
    ("^a h l", "focus"),
    ("^a j k", "worktree"),
    ("^a q", "quit"),
];
const ARMED_KEYS: &[(&str, &str)] = &[
    ("h l", "focus"),
    ("j k", "worktree"),
    ("a", "send ^A"),
    ("q", "quit"),
    ("esc", "cancel"),
];

/// From this width the counts carry their words.
const WORDS_FROM: u16 = 150;

pub(crate) fn draw(p: &mut Painter<'_>, app: &App) {
    let area = app.layout.status;
    let (y, end) = (area.y, area.right().saturating_sub(1));
    let armed = app.prefix.is_some();
    let bar = if armed { FOCUS_BG } else { CANVAS };
    p.fill(area, bar);
    let badge = if armed {
        pill("^A", ACCENT, BG, bar)
    } else {
        pill("orca", SELECTION, TEXT, bar)
    };
    let mut x = p.line(1, y, &badge) + 2;
    x = p.line(x, y, &where_keys_go(app)) + 4;

    let message = app
        .message
        .as_ref()
        .filter(|_| !armed)
        .map(|(text, _)| format!("{text} "));
    let keys = if armed { ARMED_KEYS } else { KEYS };
    let right_width = message
        .as_deref()
        .map_or_else(|| segs_width(&hints(keys, armed)), width);
    // The counts lose their words first, then the first keys go, so nothing runs into the rest.
    let counts = menu::counts(&app.fleets);
    let words = area.width >= WORDS_FROM
        && x + segs_width(&count_segs(counts, true)) + 2 + right_width <= end;
    let counts_end = p.line(x, y, &count_segs(counts, words)) + 2;
    let room = end.saturating_sub(counts_end);
    if let Some(text) = message {
        p.rtext(end, y, &trunc(&text, room), fg(TEXT));
        return;
    }
    let fitting = (0..keys.len())
        .map(|first| hints(&keys[first..], armed))
        .find(|right| segs_width(right) <= room);
    if let Some(right) = fitting {
        p.rsegs(end, y, &right);
    }
}

/// The open worktree's glyph and `project/worktree`, then where the keys go.
fn where_keys_go(app: &App) -> Vec<Seg> {
    let found = app
        .open
        .as_deref()
        .and_then(|open| menu::find(&app.fleets, open));
    let Some((project, worktree)) = found else {
        return vec![seg("no worktree", fg(TEXT_2))];
    };
    let (glyph, tok) = theme::look(&worktree.state, app.frame);
    let focus = match app.focus {
        Focus::SideMenu => "worktrees",
        Focus::Centre => "agent",
    };
    vec![
        seg(glyph.to_string(), bold(tok)),
        seg(
            format!(" {}/{}", menu::project_name(&project.path), worktree.name),
            bold(TEXT),
        ),
        seg(format!("  {focus}"), fg(TEXT_2)),
    ]
}

/// `? 1 needs you   ✗ 0 failed   ◐ 2 running   ○ 3 quiet`, or without the words.
fn count_segs(counts: Counts, words: bool) -> Vec<Seg> {
    let marks = [
        ('?', WAITING, counts.needs_you, "needs you"),
        ('✗', FAILED, counts.failed, "failed"),
        ('◐', WORKING, counts.running, "running"),
        ('○', TEXT_2, counts.quiet, "quiet"),
    ];
    let mut out = Vec::new();
    for (at, (glyph, tok, count, word)) in marks.into_iter().enumerate() {
        if at > 0 {
            out.push(seg("   ", fg(TEXT)));
        }
        out.push(seg(glyph.to_string(), bold(tok)));
        out.push(seg(format!(" {count}"), bold(TEXT)));
        if words {
            out.push(seg(format!(" {word}"), fg(TEXT_2)));
        }
    }
    out
}

/// The keys as keycaps, each with what it does; lit while the prefix waits.
fn hints(keys: &[(&str, &str)], lit: bool) -> Vec<Seg> {
    let mut out = Vec::new();
    for (at, (name, what)) in keys.iter().enumerate() {
        out.push(key(name, lit));
        let gap = if at + 1 < keys.len() { "  " } else { " " };
        out.push(seg(format!(" {what}{gap}"), fg(TEXT_2)));
    }
    out
}
