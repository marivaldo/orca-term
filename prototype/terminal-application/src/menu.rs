//! The side menu's lines, copied from the Neovim sidebar (`lua/orca-term/render.lua`,
//! `spec/golden/sidebar.txt`): a header for each project, the three attention sections, a
//! worktree in two lines, and the selected worktree inside a rounded card.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::App;
use crate::fleet::{self, State, Stop};

pub const ACCENT: Color = Color::Indexed(75);
pub const MUTED: Color = Color::Indexed(245);
pub const FAINT: Color = Color::Indexed(240);
pub const BAR: Color = Color::Indexed(236);
pub const GREEN: Color = Color::Indexed(114);
pub const RED: Color = Color::Indexed(203);
pub const YELLOW: Color = Color::Indexed(179);

pub fn state_colour(state: State) -> Color {
    match state {
        State::Waiting => YELLOW,
        State::Working => ACCENT,
        State::Finished => GREEN,
        State::Idle => MUTED,
    }
}

/// What the side menu shows: its lines and, for each line, the stop it belongs to.
pub struct MenuLines {
    pub lines: Vec<Line<'static>>,
    pub stops: Vec<Option<usize>>,
}

/// `text` cut to `max` screen cells, ending in `…` when it did not fit.
pub fn fit(text: &str, max: usize) -> String {
    if text.width() <= max {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > max {
            break;
        }
        out.push(c);
        used += w;
    }
    out.push('…');
    out
}

fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(Span::width).sum()
}

/// `left`, then spaces, then `right`, filling exactly `width` cells.
fn spread(
    mut left: Vec<Span<'static>>,
    right: Vec<Span<'static>>,
    width: usize,
) -> Vec<Span<'static>> {
    let gap = width.saturating_sub(spans_width(&left) + spans_width(&right));
    left.push(Span::raw(" ".repeat(gap)));
    left.extend(right);
    left
}

pub fn menu_lines(app: &App, width: u16, focused: bool) -> MenuLines {
    // Nothing reaches the last column, as in the sidebar.
    let width = usize::from(width).saturating_sub(1).max(20);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut stops: Vec<Option<usize>> = Vec::new();
    let muted = Style::new().fg(MUTED);
    let selected = app.stops[app.selected];

    for (p, project) in app.fleet.iter().enumerate() {
        let stop = app.stops.iter().position(|s| *s == Stop::Header(p));
        let mut header = vec![
            Span::styled(" ▾ ", muted),
            Span::styled(project.name, Style::new().add_modifier(Modifier::BOLD)),
            Span::styled(format!("  {}", project.branch), muted),
        ];
        let mut style = Style::new();
        if selected == Stop::Header(p) {
            // The header is a stop: the selected one is a full-width bar.
            header = spread(header, vec![], width);
            style = if focused {
                Style::new().bg(ACCENT).fg(Color::Black)
            } else {
                Style::new().bg(FAINT)
            };
            for span in &mut header {
                span.style = style.add_modifier(span.style.add_modifier);
            }
        }
        lines.push(Line::from(header).style(style));
        stops.push(stop);

        if project.worktrees.is_empty() {
            lines.push(Line::styled("   no worktrees yet", muted));
            stops.push(None);
        }
        for (title, members) in fleet::sections(project) {
            lines.push(Line::default());
            stops.push(None);
            lines.push(Line::styled(format!(" {title} {}", members.len()), muted));
            stops.push(None);
            for w in members {
                let stop = app.stops.iter().position(|s| *s == Stop::Worktree(p, w));
                let carded = selected == Stop::Worktree(p, w);
                for line in worktree_rows(app, p, w, width, carded, focused) {
                    lines.push(line);
                    stops.push(stop);
                }
            }
        }
        lines.push(Line::default());
        stops.push(None);
    }
    MenuLines { lines, stops }
}

/// A worktree's two lines, or four when it is the selected one and sits in its card.
fn worktree_rows(
    app: &App,
    p: usize,
    w: usize,
    width: usize,
    carded: bool,
    focused: bool,
) -> Vec<Line<'static>> {
    let worktree = &app.fleet[p].worktrees[w];
    let colour = state_colour(worktree.state);
    let muted = Style::new().fg(MUTED);
    let unseen = if worktree.unseen {
        Span::styled("•", Style::new().fg(ACCENT))
    } else {
        Span::raw(" ")
    };
    let name_style = if worktree.unseen || carded {
        Style::new().add_modifier(Modifier::BOLD)
    } else {
        Style::new()
    };
    // The card takes four cells of the row: `│ ` before and ` │` after.
    let inner = if carded { width - 5 } else { width - 1 };

    let right = vec![
        Span::styled(worktree.agent, muted),
        Span::styled(format!(" {}", worktree.age), muted),
    ];
    let name_room = inner.saturating_sub(spans_width(&right) + 3);
    let first = spread(
        vec![
            Span::styled(worktree.state.glyph(app.tick), Style::new().fg(colour)),
            Span::raw(" "),
            Span::styled(fit(worktree.name, name_room), name_style),
        ],
        right,
        inner,
    );

    let icons = if worktree.pull_request {
        vec![Span::styled("PR", Style::new().fg(GREEN))]
    } else {
        vec![]
    };
    let says_style = if worktree.state == State::Waiting {
        Style::new().fg(colour)
    } else {
        muted
    };
    let says_room = inner.saturating_sub(spans_width(&icons) + 3);
    let second = spread(
        vec![
            Span::raw("  "),
            Span::styled(fit(worktree.says, says_room), says_style),
        ],
        icons,
        inner,
    );

    if !carded {
        let mut one = vec![unseen];
        one.extend(first);
        let mut two = vec![Span::raw(" ")];
        two.extend(second);
        return vec![Line::from(one), Line::from(two)];
    }

    let border = Style::new().fg(if focused { ACCENT } else { MUTED });
    let edge = |left: &str, right: &str| {
        Line::styled(format!(" {left}{}{right}", "─".repeat(inner + 2)), border)
    };
    let boxed = |lead: Span<'static>, content: Vec<Span<'static>>| {
        let mut spans = vec![Span::styled(" │", border), lead];
        spans.extend(content);
        spans.push(Span::styled(" │", border));
        Line::from(spans)
    };
    vec![
        edge("╭", "╮"),
        boxed(unseen, first),
        boxed(Span::raw(" "), second),
        edge("╰", "╯"),
    ]
}
