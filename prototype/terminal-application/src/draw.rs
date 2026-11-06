//! Where everything goes (`layout`) and how it is painted (`draw`): the design's "Quiet v2"
//! (`design/README.md`, section 4 of `design/orca-term TUI.dc.html`). The live loop and the
//! snapshot mode share both.
//!
//! 180 columns: CANVAS gutters at 0, 35, 144 and 179; the side menu card x1 w34, the centre card
//! x36 w108, the right card x145 w34; every card from row 0 to the row above the status bar.

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::{AGENT, App, Focus, NVIM, SHELL, TERMINALS};
use crate::fleet::{FileChange, Key};
use crate::mock;
use crate::paint::{
    self, Mode, Painter, Seg, bold, c, fg, italic, key, len, pill, seg, segs_len, trunc, trunc_left,
};
use crate::theme::{
    self, ACCENT, BG, BORDER, CANVAS, DONE, FAILED, FOCUS_BG, FOCUS_SEL, PANEL, RAISED, SELECTION,
    State, TEXT, TEXT_2, VEIL, WAITING, WORKING,
};

/// One terminal on screen: which of the worktree's three, its card and the PTY's area.
#[derive(Clone, Copy, Debug)]
pub struct Pane {
    pub which: usize,
    pub card: Rect,
    pub body: Rect,
}

/// Where each part of a frame went, for the mouse and the terminals' sizes.
#[derive(Clone, Debug, Default)]
pub struct Geometry {
    pub sidebar: Option<Rect>,
    pub right: Option<Rect>,
    pub panes: Vec<Pane>,
    pub status: Rect,
    /// Filled while drawing: the tab labels, the side menu's items (top row, row after, key)
    /// and the right card's file rows (row, index).
    pub tabs: Vec<(Rect, usize)>,
    pub items: Vec<(u16, u16, Key)>,
    pub files: Vec<(u16, usize)>,
    pub too_small: bool,
}

/// A pane's PTY area inside its card: from the third row (under the tabs and a blank row) to
/// the last row of the body; two columns of margin, fewer for nvim, whose sign column sits at
/// the card's edge as in the design (one column zoomed or alone, none in the split).
fn body(card: Rect, which: usize, split: bool) -> Rect {
    let inset = match (which, split) {
        (NVIM, true) => 0,
        (NVIM, false) => 1,
        _ => 2,
    };
    Rect {
        x: card.x + inset,
        y: card.y + 3,
        width: card.width.saturating_sub(2 * inset),
        height: card.height.saturating_sub(4),
    }
}

pub fn layout(app: &App, area: Rect) -> Geometry {
    let (w, h) = (area.width, area.height);
    let mut g = Geometry {
        status: Rect::new(0, h.saturating_sub(1), w, 1),
        ..Geometry::default()
    };
    if w < 60 || h < 16 {
        g.too_small = true;
        return g;
    }
    let ch = h - 1;
    let mut centre = Rect::new(1, 0, w - 2, ch);
    if !app.zoom {
        if w >= 150 {
            g.sidebar = Some(Rect::new(1, 0, 34, ch));
            centre = Rect::new(36, 0, w - 37, ch);
            if app.right_as_column() {
                g.right = Some(Rect::new(w - 35, 0, 34, ch));
                centre.width = w - 72;
            }
        } else if w >= 100 {
            g.sidebar = Some(Rect::new(1, 0, 28, ch));
            centre = Rect::new(30, 0, w - 31, ch);
        }
    }
    if app.right_overlay {
        g.right = Some(Rect::new(w - 35, 0, 34, ch));
    }
    let split = app.split && !app.zoom;
    let pane = |which: usize, card: Rect| Pane {
        which,
        card,
        body: body(card, which, split),
    };
    if split {
        let left = Rect {
            width: (centre.width - 1) / 2,
            ..centre
        };
        let right = Rect {
            x: left.right() + 1,
            width: centre.width - left.width - 1,
            ..centre
        };
        g.panes.push(pane(AGENT, left));
        g.panes.push(pane(app.side(), right));
    } else {
        g.panes.push(pane(app.current(), centre));
    }
    g
}

pub fn draw(frame: &mut Frame, app: &App, g: &mut Geometry, mode: Mode) {
    let area = frame.area();
    {
        let mut p = Painter {
            buf: frame.buffer_mut(),
        };
        p.fill(0, 0, area.width, area.height, CANVAS);
        if g.too_small {
            p.text(
                1,
                0,
                "the prototype needs at least 60 columns by 16 rows",
                fg(TEXT),
            );
        } else {
            if let Some(card) = g.sidebar {
                sidebar(&mut p, app, card, g);
            }
            for pane in g.panes.clone() {
                draw_pane(&mut p, app, g, &pane);
            }
            if let Some(card) = g.right {
                right(&mut p, app, card, g);
            }
            status(&mut p, app, g.status);
            if app.picker.is_some() {
                picker(&mut p, app, area);
            }
            if app.help {
                help(&mut p, app, area);
            }
        }
        paint::finish(p.buf, mode);
    }
    // The child's cursor is the real cursor, on the terminal with the focus.
    if let Some(pane) = g.panes.iter().find(|pane| pane.which == app.current())
        && app.focus == Focus::Centre
        && !app.help
        && app.picker.is_none()
        && let Some(term) = app.term(pane.which)
        && !term.exited()
    {
        let parser = term.parser();
        let screen = parser.screen();
        let (row, col) = screen.cursor_position();
        if !screen.hide_cursor() && screen.scrollback() == 0 && row < pane.body.height {
            frame.set_cursor_position((
                pane.body.x + col.min(pane.body.width - 1),
                pane.body.y + row,
            ));
        }
    }
}

// ---------- side menu ----------

/// The design's `sidebar2`: "Projects", each project's avatar and name, its worktrees in three
/// lines (two when the card is under 40 rows), the open one in a half-line card.
fn sidebar(p: &mut Painter, app: &App, card: Rect, g: &mut Geometry) {
    let focused = app.focus == Focus::Sidebar && app.picker.is_none();
    let (x, y, w, hh) = (card.x, card.y, card.width, card.height);
    p.card(x, y, w, hh, PANEL, if focused { 8 } else { 0 });
    let title = if focused { ACCENT } else { TEXT_2 };
    p.text(x + 2, y + 1, "Projects", bold(title));
    let compact = hh < 40;
    // Nothing reaches the card's bottom edge, nor the footer's row when it has the focus.
    let limit = y + hh - if focused { 2 } else { 1 };
    let mut r = y + 3;
    for (pi, project) in app.fleet.iter().enumerate() {
        if r >= limit {
            break;
        }
        let initial = project.name.chars().next().unwrap_or(' ').to_string();
        p.line(x + 1, r, &pill(&initial, RAISED, TEXT, PANEL));
        p.text(
            x + 5,
            r,
            &trunc(project.name, usize::from(w - 7)),
            bold(TEXT),
        );
        r += 2;
        if project.worktrees.is_empty() {
            if r < limit {
                p.text(x + 6, r, "no worktrees yet", italic(TEXT_2));
            }
            r += 2;
            continue;
        }
        for (wi, t) in project.worktrees.iter().enumerate() {
            let k = (pi, wi);
            let nl: u16 = if compact { 2 } else { 3 };
            if r + nl > limit {
                r += nl + 1;
                continue;
            }
            g.items.push((r, r + nl, k));
            let cursor = focused && k == app.cursor;
            let open = k == app.open;
            let unseen = t.unseen && !open;
            if cursor || open {
                let pb = if cursor { FOCUS_SEL } else { SELECTION };
                p.tint(x + 1, r, w - 2, nl, pb);
                let bar = |m: &str| m.repeat(usize::from(w - 2));
                p.text(x + 1, r - 1, &bar("▄"), fg(pb).bg(c(PANEL)));
                p.text(x + 1, r + nl, &bar("▀"), fg(pb).bg(c(PANEL)));
                if cursor {
                    for k in 0..nl {
                        p.text(x + 1, r + k, "▎", fg(ACCENT).bg(c(pb)));
                    }
                }
            }
            let state = theme::state_tok(t.state);
            p.text(
                x + 4,
                r,
                &theme::glyph(t.state, app.tick).to_string(),
                bold(state),
            );
            let tag = if t.primary { " primary " } else { "" };
            let room = usize::from(w)
                .saturating_sub(
                    9 + if tag.is_empty() {
                        0
                    } else {
                        len(tag) as usize + 1
                    },
                )
                .saturating_sub(if unseen { 2 } else { 0 });
            let name = if cursor { ACCENT } else { TEXT };
            let mut nx = p.text(x + 6, r, &trunc(t.name, room), bold(name));
            if unseen {
                nx = p.text(nx + 1, r, &theme::UNSEEN.to_string(), bold(state));
            }
            if !tag.is_empty() {
                p.text(nx + 1, r, tag, fg(TEXT_2).bg(c(RAISED)));
            }
            if compact {
                let room = usize::from(w).saturating_sub(11 + t.age.len());
                p.text(x + 6, r + 1, &trunc(t.last_message, room), fg(TEXT_2));
                p.rtext(x + w - 3, r + 1, t.age, fg(TEXT_2));
                r += 3;
                continue;
            }
            let pr = t.pr.as_ref().map(|pr| format!("#{}", pr.number));
            let pr_room = pr.as_ref().map_or(0, |n| n.len() + 1);
            let room = usize::from(w).saturating_sub(11 + pr_room);
            p.text(x + 6, r + 1, &trunc(t.branch, room), fg(TEXT_2));
            if let Some(pr) = &pr {
                p.rtext(x + w - 3, r + 1, pr, bold(DONE));
            }
            let mut lx = x + 6;
            if let Some(pr) = &t.pr {
                let glyph = format!("{} ", theme::glyph(pr.check, app.tick));
                lx = p.text(lx, r + 2, &glyph, bold(theme::state_tok(pr.check)));
            }
            if let Some(agent) = t.agent.filter(|_| !t.primary) {
                lx = p.text(lx, r + 2, &format!("{agent} · "), italic(TEXT_2));
            }
            let room = (x + w).saturating_sub(4 + len(t.age)).saturating_sub(lx);
            p.text(
                lx,
                r + 2,
                &trunc(t.last_message, usize::from(room)),
                fg(TEXT_2),
            );
            p.rtext(x + w - 3, r + 2, t.age, fg(TEXT_2));
            r += 4;
        }
        r += 1;
    }
    if focused {
        let mut segs = vec![
            key("enter", false),
            seg(" open  ", fg(TEXT_2)),
            key("n", false),
            seg(" new", fg(TEXT_2)),
        ];
        if w >= 32 {
            segs.extend([
                seg("  ", fg(TEXT)),
                key("/", false),
                seg(" filter", fg(TEXT_2)),
            ]);
        }
        p.segs(x + 2, y + hh - 2, &segs, w - 4);
    }
}

// ---------- centre ----------

/// A tab row: the active tab a pill, the others quiet; `right` aligned on the right. Returns
/// the active pill's column and width, for the accent stroke.
fn tabs(
    p: &mut Painter,
    g: &mut Geometry,
    card: Rect,
    list: &[usize],
    active: usize,
    focused: bool,
    right: &[Seg],
) -> (u16, u16) {
    let y = card.y + 1;
    let mut cx = card.x + 2;
    let mut stroke = (cx, 0);
    for &which in list {
        let label = format!("{} {}", which + 1, TERMINALS[which]);
        let start = cx;
        if which == active {
            let (bg, text) = if focused {
                (FOCUS_SEL, ACCENT)
            } else {
                (SELECTION, TEXT)
            };
            cx = p.line(cx, y, &pill(&label, bg, text, BG));
            stroke = (start, cx - start);
        } else {
            cx = p.text(cx, y, &format!(" {label} "), fg(TEXT_2));
        }
        g.tabs.push((Rect::new(start, y, cx - start, 1), which));
        cx += 1;
    }
    if !right.is_empty() {
        let end = card.x + card.width - 1;
        let room = end.saturating_sub(cx + 1);
        if segs_len(right) <= room {
            p.rsegs(end, y, right);
        }
    }
    stroke
}

/// `4 files +55 −21`, from the open worktree's changed files.
fn summary(files: &[FileChange]) -> Vec<Seg> {
    let (add, del) = totals(files);
    vec![
        seg(format!("{} files ", files.len()), fg(TEXT_2)),
        seg(format!("+{add}"), fg(DONE)),
        seg(format!(" −{del}  "), fg(FAILED)),
    ]
}

fn totals(files: &[FileChange]) -> (u32, u32) {
    files
        .iter()
        .fold((0, 0), |(a, d), f| (a + f.add, d + f.del))
}

fn draw_pane(p: &mut Painter, app: &App, g: &mut Geometry, pane: &Pane) {
    let card = pane.card;
    let wt = app.open_wt();
    let focused = app.focus == Focus::Centre && app.current() == pane.which;
    p.card(card.x, card.y, card.width, card.height, BG, 0);

    let agent_name = || -> Vec<Seg> {
        wt.agent
            .map(|a| vec![seg(a, italic(TEXT_2))])
            .unwrap_or_default()
    };
    let (list, right): (Vec<usize>, Vec<Seg>) = if app.zoom {
        let mut right = Vec::new();
        if app.split {
            let side = TERMINALS[app.side()];
            let lit = |name: &str| {
                if TERMINALS[app.current()] == name {
                    bold(ACCENT)
                } else {
                    fg(TEXT_2)
                }
            };
            right.extend([
                seg("split  ", fg(TEXT_2)),
                seg("agent", lit("agent")),
                seg(" | ", fg(BORDER)),
                seg(side, lit(side)),
                seg("    ", fg(TEXT)),
            ]);
        }
        right.extend(pill("zoom", FOCUS_SEL, ACCENT, BG));
        right.extend([
            seg("  ", fg(TEXT)),
            key("^a z", false),
            seg(" restore", fg(TEXT_2)),
        ]);
        (vec![pane.which], right)
    } else if app.split && pane.which == AGENT {
        (vec![AGENT], agent_name())
    } else if app.split {
        let file = app
            .opened
            .get(&app.open)
            .filter(|_| pane.which == NVIM)
            .map(|path| path.rsplit('/').next().unwrap_or(path).to_owned());
        let right = file.map(|f| vec![seg(f, fg(TEXT_2))]).unwrap_or_default();
        (vec![NVIM, SHELL], right)
    } else if !app.right_as_column() {
        let mut right = summary(&app.open_files());
        right.push(key("^a f", false));
        (vec![AGENT, NVIM, SHELL], right)
    } else {
        let mut right = vec![seg(app.location(app.open), fg(TEXT_2))];
        if let Some(agent) = wt.agent {
            right.push(seg("  ·  ", fg(BORDER)));
            right.push(seg(agent, italic(TEXT_2)));
        }
        (vec![AGENT, NVIM, SHELL], right)
    };
    let (sx, sw) = tabs(p, g, card, &list, pane.which, focused, &right);
    if focused {
        p.text(
            sx,
            card.y,
            &"▁".repeat(usize::from(sw)),
            fg(ACCENT).bg(c(CANVAS)),
        );
    }

    let b = pane.body;
    if pane.which == AGENT && wt.failure {
        failure(p, app, b);
        return;
    }
    if pane.which == AGENT && wt.agent.is_none()
        || pane.which == AGENT && !app.mock() && app.term(AGENT).is_none()
    {
        p.line(
            b.x + 1,
            b.y,
            &[
                seg("no agent running  ·  ", fg(TEXT_2)),
                key("enter", false),
                seg(" starts claude", fg(TEXT_2)),
            ],
        );
        return;
    }
    if app.mock() {
        placeholder(p, pane.which, b, focused);
        return;
    }
    let Some(term) = app.term(pane.which) else {
        return;
    };
    let parser = term.parser();
    let screen = parser.screen();
    paint::screen(p.buf, b, screen);
    if term.exited() {
        let y = b.y + b.height - 1;
        p.fill(b.x, y, b.width, 1, BG);
        p.line(
            b.x + 1,
            y,
            &[
                seg("process exited  ·  ", fg(TEXT_2)),
                key("enter", false),
                seg(" starts it again", fg(TEXT_2)),
            ],
        );
    } else if screen.scrollback() > 0 {
        let text = format!(" {} lines up ", screen.scrollback());
        p.rtext(b.x + b.width, b.y, &text, fg(TEXT_2).bg(c(RAISED)));
    }
}

/// What the snapshots show where a terminal would be.
fn placeholder(p: &mut Painter, which: usize, b: Rect, focused: bool) {
    match which {
        AGENT => {
            // claude-code keeps a column of margin of its own when it has room.
            let indent = u16::from(b.width > 60);
            let lines = mock::claude_lines(usize::from(b.width - indent - 1));
            mock::draw_lines(p, b.x + indent, b.y, b.width - indent, b.height, &lines);
        }
        NVIM => mock::nvim(p, b.x, b.y, b.width, b.height, focused),
        _ => {
            p.line(
                b.x,
                b.y,
                &[seg("~/code/app.worktrees/fix-login $ ", fg(TEXT_2))],
            );
        }
    }
}

/// The stopped agent: its last output, and under it the orca-term card that says what
/// happened and offers the actions, the primary one lit (4e).
fn failure(p: &mut Painter, app: &App, b: Rect) {
    let wt = app.open_wt();
    let bottom = b.y + b.height - 3;
    let top = bottom.saturating_sub(5);
    let lines = mock::fail_lines(usize::from(b.width - 2));
    mock::draw_lines(p, b.x + 1, b.y, b.width - 1, top - b.y, &lines);
    p.card_on(b.x, top, b.width, 6, SELECTION, BG, 0);
    let x = b.x;
    p.line(
        x + 3,
        top + 1,
        &[
            seg("✗", bold(FAILED)),
            seg("  agent stopped", bold(TEXT)),
            seg(
                format!("  ·  exit 1 after 41m  ·  {} ago", wt.age),
                fg(TEXT_2),
            ),
        ],
    );
    let files = app.open_files();
    let (add, del) = totals(&files);
    let n = files.len();
    let safe = format!(
        "Your changes are safe in the worktree ({n} file{}, +{add} −{del}). The session can \
         resume from the last turn.",
        if n == 1 { "" } else { "s" }
    );
    p.text(
        x + 6,
        top + 2,
        &trunc(&safe, usize::from(b.width - 7)),
        fg(TEXT_2),
    );
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
        b.width - 7,
    );
}

// ---------- right card ----------

/// The design's `right2`: changes with their diffstat bar, the pull request, the branch.
fn right(p: &mut Painter, app: &App, card: Rect, g: &mut Geometry) {
    let focused = app.focus == Focus::Right && app.picker.is_none();
    let (x, y, w, hh) = (card.x, card.y, card.width, card.height);
    p.card(x, y, w, hh, PANEL, if focused { 7 } else { 0 });
    let wt = app.open_wt();
    let files = app.open_files();
    let (add, del) = totals(&files);
    let title = if focused { ACCENT } else { TEXT };
    p.line(
        x + 2,
        y + 1,
        &[
            seg("changes", bold(title)),
            seg(format!("  {}", files.len()), fg(TEXT_2)),
        ],
    );
    p.rsegs(
        x + w - 2,
        y + 1,
        &[
            seg(format!("+{add}"), fg(DONE)),
            seg(format!(" −{del}"), fg(FAILED)),
        ],
    );

    // What comes after the files keeps its rows; the files scroll in what is left.
    let pr_rows = wt.pr.as_ref().map_or(3, |pr| 3 + pr.checks.len() as u16);
    let after = 13 + pr_rows;
    let room = usize::from(hh.saturating_sub(2 + 3 + after)).max(1);
    let offset = (app.file + 1).saturating_sub(room);
    let mut r = y + 3;
    for (index, f) in files.iter().enumerate().skip(offset).take(room) {
        let cursor = focused && index == app.file;
        if cursor {
            p.tint(x + 1, r, w - 2, 1, FOCUS_SEL);
            p.text(x + 1, r, "▎", fg(ACCENT).bg(c(FOCUS_SEL)));
        }
        g.files.push((r, index));
        let colour = match f.kind {
            'A' => DONE,
            'D' => FAILED,
            _ => WORKING,
        };
        p.text(x + 2, r, &f.kind.to_string(), bold(colour));
        let mut count = Vec::new();
        if f.add > 0 {
            count.push(seg(format!("+{}", f.add), fg(DONE)));
        }
        if f.del > 0 {
            let lead = if f.add > 0 { " " } else { "" };
            count.push(seg(format!("{lead}−{}", f.del), fg(FAILED)));
        }
        let cw = segs_len(&count);
        let (dir, base) = match f.path.rfind('/') {
            Some(i) => (&f.path[..=i], &f.path[i + 1..]),
            None => ("", f.path.as_str()),
        };
        let name_style = match (cursor, f.kind) {
            (true, _) => bold(ACCENT),
            (_, 'D') => fg(TEXT_2),
            _ => fg(TEXT),
        };
        let base_room = usize::from((x + w).saturating_sub(5 + cw + x + 4));
        let ex = p.text(x + 4, r, &trunc(base, base_room), name_style);
        if !dir.is_empty() {
            let dir_room = (x + w).saturating_sub(4 + cw + ex + 1);
            p.text(
                ex + 1,
                r,
                &trunc_left(dir, usize::from(dir_room)),
                fg(TEXT_2),
            );
        }
        p.rsegs(x + w - 2, r, &count);
        r += 1;
    }
    r += 1;
    let bw = w - 4;
    if add + del > 0 {
        let ga = ((f64::from(bw) * f64::from(add) / f64::from(add + del)).round()) as u16;
        p.text(x + 2, r, &"━".repeat(usize::from(ga)), fg(DONE));
        p.text(x + 2 + ga, r, &"━".repeat(usize::from(bw - ga)), fg(FAILED));
    }
    r += 3;
    p.label(x + 2, x + w - 2, r, "pull request", TEXT_2);
    r += 2;
    match &wt.pr {
        Some(pr) => {
            p.segs(
                x + 2,
                r,
                &[
                    seg(format!("#{}", pr.number), bold(TEXT)),
                    seg(format!(" {}", pr.title), fg(TEXT)),
                ],
                w - 4,
            );
            p.text(x + 2, r + 1, pr.summary, fg(TEXT_2));
            r += 3;
            for check in &pr.checks {
                let tok = theme::state_tok(check.state);
                p.text(
                    x + 2,
                    r,
                    &theme::glyph(check.state, 0).to_string(),
                    bold(tok),
                );
                p.text(x + 4, r, check.name, fg(TEXT));
                let detail = if check.state == State::Failed {
                    FAILED
                } else {
                    TEXT_2
                };
                p.rtext(x + w - 2, r, check.detail, fg(detail));
                r += 1;
            }
        }
        None => {
            p.line(
                x + 2,
                r,
                &[seg("○", fg(TEXT_2)), seg("  none yet", fg(TEXT_2))],
            );
            r += 2;
            p.line(
                x + 2,
                r,
                &[key("^a P", false), seg("  open a draft", fg(TEXT_2))],
            );
            r += 1;
        }
    }
    r += 2;
    p.label(x + 2, x + w - 2, r, "branch", TEXT_2);
    r += 2;
    p.segs(
        x + 2,
        r,
        &[
            seg(wt.name, fg(TEXT)),
            seg(" → ", fg(TEXT_2)),
            seg("main", fg(TEXT)),
        ],
        w - 4,
    );
    p.line(
        x + 2,
        r + 1,
        &[
            seg(format!("↑{}", wt.ahead), bold(TEXT)),
            seg(" ahead  ", fg(TEXT_2)),
            seg("↓0", bold(TEXT)),
            seg(" behind", fg(TEXT_2)),
        ],
    );
    let n = files.len();
    let uncommitted = format!("{n} file{} uncommitted", if n == 1 { "" } else { "s" });
    p.text(x + 2, r + 2, &uncommitted, fg(TEXT_2));
}

// ---------- status bar ----------

/// The design's `status2`: the pill, where the keys go, the counts, the hints.
fn status(p: &mut Painter, app: &App, area: Rect) {
    let (y, w) = (area.y, area.width);
    let prefix = app.pending.is_some();
    let sb = if prefix { FOCUS_BG } else { CANVAS };
    p.fill(0, y, w, 1, sb);
    let narrow = w < 150;
    let pfx = format!("^{}", app.prefix);
    let mut x = if prefix {
        let label = format!("^{}", app.prefix.to_ascii_uppercase());
        p.line(1, y, &pill(&label, ACCENT, BG, sb))
    } else {
        p.line(1, y, &pill("orca", SELECTION, TEXT, sb))
    } + 2;

    let wt = app.open_wt();
    let pane = if app.picker.is_some() {
        "picker"
    } else {
        match app.focus {
            Focus::Sidebar => "worktrees",
            Focus::Right => "changes",
            Focus::Centre => TERMINALS[app.current()],
        }
    };
    x = p.line(
        x,
        y,
        &[
            seg(
                theme::glyph(wt.state, app.tick).to_string(),
                bold(theme::state_tok(wt.state)),
            ),
            seg(format!(" {}", app.location(app.open)), bold(TEXT)),
            seg(format!("  {pane}"), fg(TEXT_2)),
        ],
    );
    if app.zoom {
        x = p.line(x + 2, y, &pill("zoom", FOCUS_SEL, ACCENT, sb));
    }
    x += 4;

    let hints: Vec<(String, &str)> = if prefix {
        vec![
            ("h l".into(), "focus"),
            ("j k".into(), "worktree"),
            ("z".into(), "zoom"),
            ("|".into(), "split"),
            ("w".into(), "picker"),
            ("f".into(), "files"),
            ("a".into(), "send ^A"),
            ("esc".into(), "cancel"),
        ]
    } else if narrow {
        vec![
            (pfx.clone(), "prefix"),
            (format!("{pfx} h l"), "focus"),
            (format!("{pfx} j k"), "worktree"),
            (format!("{pfx} ?"), "help"),
        ]
    } else {
        vec![
            (pfx.clone(), "prefix"),
            (format!("{pfx} h l"), "focus"),
            (format!("{pfx} j k"), "worktree"),
            (format!("{pfx} z"), "zoom"),
            (format!("{pfx} |"), "split"),
            (format!("{pfx} ?"), "help"),
        ]
    };
    let hint_segs = |hints: &[(String, &str)]| -> Vec<Seg> {
        let mut out = Vec::new();
        for (i, (k, l)) in hints.iter().enumerate() {
            out.push(key(k, prefix));
            let gap = if i + 1 < hints.len() { "  " } else { " " };
            out.push(seg(format!(" {l}{gap}"), fg(TEXT_2)));
        }
        out
    };
    let counts = crate::fleet::counts(&app.fleet);
    let count_segs = |labels: bool| -> Vec<Seg> {
        let mut out = Vec::new();
        let marks = [
            ('?', WAITING, "needs you"),
            ('✗', FAILED, "failed"),
            ('◐', WORKING, "running"),
            ('○', TEXT_2, "quiet"),
        ];
        for (i, (g, tok, label)) in marks.iter().enumerate() {
            out.push(seg(g.to_string(), bold(*tok)));
            out.push(seg(format!(" {}", counts[i]), bold(TEXT)));
            if labels {
                out.push(seg(format!(" {label}"), fg(TEXT_2)));
            }
            if i + 1 < marks.len() {
                out.push(seg("   ", fg(TEXT)));
            }
        }
        out
    };

    // The right-hand side: the hints, or a message for a while. Neither may run into the
    // counts: the counts lose their labels first, then the first hints go.
    let message = app
        .message
        .as_ref()
        .filter(|_| !prefix)
        .map(|(text, _)| text.clone());
    let end = w - 1;
    let mut right = match &message {
        Some(text) => vec![seg(format!("{text} "), fg(TEXT))],
        None => hint_segs(&hints),
    };
    // At least two cells between the counts and what is on the right.
    let mut labels = !narrow;
    if labels && x + segs_len(&count_segs(true)) + 2 > end.saturating_sub(segs_len(&right)) {
        labels = false;
    }
    let counts_end = x + segs_len(&count_segs(labels)) + 2;
    let mut first = 0;
    while message.is_none()
        && first + 1 < hints.len()
        && counts_end > end.saturating_sub(segs_len(&right))
    {
        first += 1;
        right = hint_segs(&hints[first..]);
    }
    if let Some(text) = &message {
        let room = usize::from(end.saturating_sub(counts_end));
        right = vec![seg(
            format!("{} ", trunc(text, room.saturating_sub(1))),
            fg(TEXT),
        )];
    }
    p.line(x, y, &count_segs(labels));
    p.rsegs(end, y, &right);
}

// ---------- overlays ----------

/// Dims what is under an overlay: text turns VEIL; the cards keep their shapes.
fn veil(p: &mut Painter, area: Rect) {
    for y in 0..area.height.saturating_sub(1) {
        for x in 0..area.width {
            let cell = &mut p.buf[(x, y)];
            let symbol = cell.symbol();
            if !"▄▀▗▖▝▘▐▌▁▎".contains(symbol) || symbol.is_empty() {
                cell.fg = c(VEIL);
                cell.modifier = ratatui::style::Modifier::empty();
            }
        }
    }
}

/// `^a w`: every worktree in a floating card, the cursor in FOCUS_SEL with its `▎`.
fn picker(p: &mut Painter, app: &App, area: Rect) {
    let Some(at) = app.picker else {
        return;
    };
    veil(p, area);
    let rows = app.fleet.len() * 2 + app.order.len() + 6;
    let w = 64.min(area.width - 4);
    let h = (rows as u16).min(area.height - 3);
    let x = (area.width - w) / 2;
    let y = (area.height - 1 - h) / 2;
    p.card(x, y, w, h, PANEL, 15);
    p.text(x + 2, y + 1, "switch worktree", bold(ACCENT));
    p.rtext(x + w - 2, y + 1, "^a w", fg(TEXT_2));
    let mut r = y + 3;
    let limit = y + h - 3;
    for (pi, project) in app.fleet.iter().enumerate() {
        if r >= limit {
            break;
        }
        let initial = project.name.chars().next().unwrap_or(' ').to_string();
        p.line(x + 1, r, &pill(&initial, RAISED, TEXT, PANEL));
        p.text(x + 5, r, project.name, bold(TEXT));
        r += 1;
        if project.worktrees.is_empty() {
            p.text(x + 6, r, "no worktrees yet", italic(TEXT_2));
            r += 1;
        }
        for (wi, t) in project.worktrees.iter().enumerate() {
            if r >= limit {
                break;
            }
            let k = (pi, wi);
            let cursor = app.order[at] == k;
            if cursor {
                p.tint(x + 1, r, w - 2, 1, FOCUS_SEL);
                p.text(x + 1, r, "▎", fg(ACCENT).bg(c(FOCUS_SEL)));
            }
            let state = theme::state_tok(t.state);
            p.text(
                x + 4,
                r,
                &theme::glyph(t.state, app.tick).to_string(),
                bold(state),
            );
            let name = if cursor { ACCENT } else { TEXT };
            p.text(x + 6, r, &trunc(t.name, 14), bold(name));
            p.text(x + 21, r, &trunc(t.branch, 18), fg(TEXT_2));
            let room = usize::from(w).saturating_sub(48);
            p.text(x + 40, r, &trunc(t.last_message, room), fg(TEXT_2));
            p.rtext(x + w - 3, r, t.age, fg(TEXT_2));
            r += 1;
        }
        r += 1;
    }
    p.line(
        x + 2,
        y + h - 2,
        &[
            key("↑↓", false),
            seg(" move  ", fg(TEXT_2)),
            key("enter", false),
            seg(" open  ", fg(TEXT_2)),
            key("esc", false),
            seg(" close", fg(TEXT_2)),
        ],
    );
}

/// `^a ?`: every key, in a card.
fn help(p: &mut Painter, app: &App, area: Rect) {
    let k = format!("^{}", app.prefix);
    let rows: Vec<(String, &str)> = vec![
        (
            format!("{k} h l"),
            "focus left / right: side menu, centre, right card",
        ),
        (
            String::new(),
            "in the split agent and nvim; zoomed, stays zoomed",
        ),
        (
            format!("{k} j k"),
            "next / previous worktree, opened at once",
        ),
        (format!("{k} 1 2 3"), "the agent, nvim or shell tab"),
        (
            format!("{k} z"),
            "zoom the terminal with the focus, and back",
        ),
        (format!("{k} |"), "agent | nvim side by side, and back"),
        (format!("{k} w"), "the floating worktree picker"),
        (format!("{k} f"), "show / hide the changed files"),
        (
            format!("{k} P"),
            "open a draft pull request (not in the prototype)",
        ),
        (format!("{k} a"), "send Ctrl-a itself to the terminal"),
        (format!("{k} ?"), "this help"),
        (format!("{k} q"), "quit"),
        ("esc".into(), "cancel the prefix"),
        (String::new(), ""),
        (
            "side menu".into(),
            "j k or ↑ ↓ move, enter opens, n new, / filter",
        ),
        ("changes".into(), "j k move, enter opens the file in nvim"),
        (
            "agent failed".into(),
            "r resume, R restart clean, l log, x dismiss",
        ),
        (
            "terminals".into(),
            "take every other key; the wheel scrolls",
        ),
    ];
    let w = 76.min(area.width - 4);
    let h = (rows.len() as u16 + 6).min(area.height - 3);
    let x = (area.width - w) / 2;
    let y = (area.height - 1 - h) / 2;
    veil(p, area);
    p.card(x, y, w, h, PANEL, 4);
    p.text(x + 2, y + 1, "keys", bold(ACCENT));
    for (i, (keys, what)) in rows.iter().enumerate() {
        let r = y + 3 + i as u16;
        if r >= y + h - 2 {
            break;
        }
        if !keys.is_empty() {
            p.line(x + 2, r, &[key(keys, false)]);
        }
        p.text(x + 18, r, &trunc(what, usize::from(w - 20)), fg(TEXT_2));
    }
    p.text(x + 2, y + h - 2, "any key closes", italic(TEXT_2));
}
