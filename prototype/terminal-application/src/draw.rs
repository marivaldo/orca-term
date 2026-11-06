//! Where everything goes (`layout`) and how it is painted (`draw`), for the three variants. The
//! live loop and the snapshot mode share both.

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Paragraph};
use tui_term::widget::{Cursor, PseudoTerminal};

use crate::app::{AGENT, App, Focus, NVIM, SHELL, TERMINALS, Variant};
use crate::fleet::{CHECKS, Stop};
use crate::menu::{
    ACCENT, BAR, FAINT, GREEN, MUTED, MenuLines, RED, YELLOW, fit, menu_lines, state_colour,
};

const MENU_WIDTH: u16 = 36;
const RIGHT_WIDTH: u16 = 30;
/// The keys of the side menu and of the right column, which need no prefix.
const MENU_KEYS: &str = "j/k move  ⏎ terminal  q quit";
const RIGHT_KEYS: &str = "j/k move  ⏎ open  h back";
/// The same, for either of them drawn on top of the centre.
const ON_TOP_KEYS: &str = "j/k move  ⏎ open  esc close";
/// How long a message stays in the status bar.
const MESSAGE_FOR: Duration = Duration::from_secs(8);

/// One terminal on screen: which of the worktree's three, its title row and its body.
#[derive(Clone, Copy, Debug)]
pub struct Pane {
    pub which: usize,
    pub title: Option<Rect>,
    pub body: Rect,
}

/// Where each part of a frame goes.
#[derive(Clone, Debug, Default)]
pub struct Geometry {
    /// The side menu's and the right column's content.
    pub menu: Option<Rect>,
    pub right: Option<Rect>,
    /// Their boxes, when they are drawn on top of the centre.
    pub menu_frame: Option<Rect>,
    pub right_frame: Option<Rect>,
    /// Variant C's one-line strip of worktrees.
    pub strip: Option<Rect>,
    pub panes: Vec<Pane>,
    /// The one title row lists all three terminals, as tabs do (variant A, not split).
    pub title_lists_all: bool,
    /// The centre, when no worktree is selected and it only says so.
    pub note: Option<Rect>,
    /// The vertical lines between columns.
    pub rules: Vec<Rect>,
    pub status: Rect,
    pub too_small: bool,
}

/// A terminal's name in a title row: ` 1 agent `.
pub fn terminal_label(which: usize, name: &str) -> String {
    format!(" {} {name} ", which + 1)
}

fn titled(which: usize, area: Rect) -> Pane {
    Pane {
        which,
        title: Some(Rect { height: 1, ..area }),
        body: Rect {
            y: area.y + 1,
            height: area.height.saturating_sub(1),
            ..area
        },
    }
}

/// `area` cut in two side by side, with the rule between them.
fn side_by_side(area: Rect, left_width: u16) -> (Rect, Rect, Rect) {
    let left = Rect {
        width: left_width,
        ..area
    };
    let rule = Rect {
        x: area.x + left_width,
        width: 1,
        ..area
    };
    let right = Rect {
        x: area.x + left_width + 1,
        width: area.width - left_width - 1,
        ..area
    };
    (left, rule, right)
}

pub fn layout(app: &App, area: Rect) -> Geometry {
    let mut g = Geometry {
        status: Rect {
            y: area.bottom().saturating_sub(1),
            height: 1,
            ..area
        },
        ..Geometry::default()
    };
    if area.width < 70 || area.height < 14 {
        g.too_small = true;
        return g;
    }
    let body = Rect {
        height: area.height - 1,
        ..area
    };
    let has_worktree = app.worktree().is_some();

    if app.zoom {
        // The panel with the focus takes everything but the status bar.
        match app.focus {
            Focus::Menu => g.menu = Some(body),
            Focus::Right => g.right = Some(body),
            Focus::Centre if has_worktree => g.panes.push(Pane {
                which: app.current(),
                title: None,
                body,
            }),
            Focus::Centre => g.note = Some(body),
        }
        return g;
    }

    // Narrower columns on a narrow screen, so the centre keeps most of it.
    let menu_width = MENU_WIDTH.min(area.width * 3 / 10);
    let right_width = RIGHT_WIDTH.min(area.width / 4);
    let mut centre = body;
    if app.variant == Variant::C {
        g.strip = Some(Rect { height: 1, ..body });
        centre.y += 1;
        centre.height -= 1;
    } else {
        if app.show_menu {
            let (menu, rule, rest) = side_by_side(centre, menu_width);
            g.menu = Some(menu);
            g.rules.push(rule);
            centre = rest;
        }
        if app.variant == Variant::A && app.show_right {
            let (rest, rule, right) = side_by_side(centre, centre.width - right_width - 1);
            g.right = Some(right);
            g.rules.push(rule);
            centre = rest;
        }
    }

    // The side menu or the right column on top of the centre, in a box.
    let inner = |frame: Rect| Rect {
        x: frame.x + 1,
        y: frame.y + 1,
        width: frame.width - 2,
        height: frame.height - 2,
    };
    match app.overlay {
        Some(Focus::Menu) => {
            let frame = Rect {
                width: menu_width + 2,
                ..centre
            };
            g.menu_frame = Some(frame);
            g.menu = Some(inner(frame));
        }
        Some(Focus::Right) => {
            let frame = Rect {
                x: centre.right() - (right_width + 2),
                width: right_width + 2,
                ..centre
            };
            g.right_frame = Some(frame);
            g.right = Some(inner(frame));
        }
        _ => {}
    }

    if !has_worktree {
        g.note = Some(centre);
        return g;
    }
    let titles = app.variant != Variant::C;
    let pane = |which: usize, area: Rect| {
        if titles {
            titled(which, area)
        } else {
            Pane {
                which,
                title: None,
                body: area,
            }
        }
    };
    match app.variant {
        Variant::B => {
            // The agent large on the left with the shell as a strip under it, nvim on the right.
            let (left, rule, right) = side_by_side(centre, centre.width * 55 / 100);
            let strip = (centre.height / 4).clamp(5, 10);
            let agent = Rect {
                height: left.height - strip - 1,
                ..left
            };
            let shell = Rect {
                y: agent.bottom(),
                height: strip + 1,
                ..left
            };
            g.panes.push(pane(AGENT, agent));
            g.panes.push(pane(SHELL, shell));
            g.panes.push(pane(NVIM, right));
            g.rules.push(rule);
        }
        _ if app.split => {
            let (left, rule, right) = side_by_side(centre, centre.width / 2);
            g.panes.push(pane(AGENT, left));
            g.panes.push(pane(NVIM, right));
            g.rules.push(rule);
        }
        _ => {
            g.panes.push(pane(app.current(), centre));
            g.title_lists_all = titles;
        }
    }
    g
}

/// The side menu ready to paint in `area`: its lines, how many of them are scrolled away so
/// that the selection stays on screen, the screen row of the first one shown and how many rows
/// there are for them.
pub struct MenuView {
    pub menu: MenuLines,
    pub offset: usize,
    pub first_row: u16,
    pub rows: u16,
}

pub fn menu_view(app: &App, area: Rect, titled: bool) -> MenuView {
    let focused = app.focus == Focus::Menu;
    let menu = menu_lines(app, area.width, focused);
    // As a column it has a title row above and a row for its keys below.
    let first_row = area.y + u16::from(titled);
    let rows = area.height.saturating_sub(2 * u16::from(titled));
    let last_selected = menu
        .stops
        .iter()
        .rposition(|stop| *stop == Some(app.selected))
        .unwrap_or(0);
    let offset = (last_selected + 1).saturating_sub(usize::from(rows));
    MenuView {
        menu,
        offset,
        first_row,
        rows,
    }
}

/// The style of a name in a title row: lit when it has the focus, marked when it is only the
/// one shown, quiet otherwise.
fn label_style(shown: bool, focused: bool) -> Style {
    if shown && focused {
        Style::new()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD)
    } else if shown {
        Style::new()
            .fg(Color::White)
            .bg(FAINT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(MUTED).bg(BAR)
    }
}

fn bar(frame: &mut Frame, area: Rect, line: Line<'_>) {
    frame.render_widget(Paragraph::new(line).style(Style::new().bg(BAR)), area);
}

pub fn draw(frame: &mut Frame, app: &App, g: &Geometry) {
    if g.too_small {
        frame.render_widget(
            Paragraph::new("the prototype needs at least 70 columns by 14 rows"),
            frame.area(),
        );
        return;
    }
    for rule in &g.rules {
        let lines: Vec<Line> = (0..rule.height).map(|_| Line::raw("│")).collect();
        frame.render_widget(Paragraph::new(lines).style(Style::new().fg(FAINT)), *rule);
    }
    if let Some(area) = g.strip {
        draw_strip(frame, app, area);
    }
    if let Some(area) = g.note {
        draw_note(frame, app, area);
    }
    for pane in &g.panes {
        draw_pane(frame, app, g, pane);
    }
    if let Some(area) = g.menu {
        match g.menu_frame {
            Some(outer) => draw_box(frame, outer, " worktrees ", ON_TOP_KEYS),
            None => draw_keys(frame, app, area, Focus::Menu, MENU_KEYS),
        }
        draw_menu(frame, app, area, g.menu_frame.is_none());
    }
    if let Some(area) = g.right {
        match g.right_frame {
            Some(outer) => draw_box(frame, outer, " changes and checks ", ON_TOP_KEYS),
            None => draw_keys(frame, app, area, Focus::Right, RIGHT_KEYS),
        }
        draw_right(frame, app, area, g.right_frame.is_none());
    }
    draw_status(frame, app, g.status);
    if app.help {
        draw_help(frame, app);
    }
}

/// The box around the side menu or the right column when it is on top of the centre, with its
/// keys on the bottom edge.
fn draw_box(frame: &mut Frame, area: Rect, title: &str, keys: &str) {
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(title.to_owned())
        .title_bottom(format!(" {keys} "));
    frame.render_widget(block, area);
}

/// A column's own keys on its last row, while it has the focus.
fn draw_keys(frame: &mut Frame, app: &App, area: Rect, column: Focus, keys: &str) {
    if app.focus != column {
        return;
    }
    let last = Rect {
        y: area.bottom() - 1,
        height: 1,
        ..area
    };
    frame.render_widget(
        Paragraph::new(format!(" {keys}")).style(Style::new().fg(MUTED)),
        last,
    );
}

fn draw_menu(frame: &mut Frame, app: &App, area: Rect, titled: bool) {
    let view = menu_view(app, area, titled);
    if titled {
        let label = Span::styled(" orca-term ", label_style(true, app.focus == Focus::Menu));
        bar(frame, Rect { height: 1, ..area }, Line::from(label));
    }
    let content = Rect {
        y: view.first_row,
        height: view.rows,
        ..area
    };
    let lines: Vec<Line> = view.menu.lines.into_iter().skip(view.offset).collect();
    frame.render_widget(Paragraph::new(lines), content);
}

fn draw_right(frame: &mut Frame, app: &App, area: Rect, titled: bool) {
    let focused = app.focus == Focus::Right;
    let muted = Style::new().fg(MUTED);
    let width = usize::from(area.width);
    let mut content = area;
    if titled {
        let name = match app.stops[app.selected] {
            Stop::Worktree(p, w) => app.fleet[p].worktrees[w].name,
            Stop::Header(p) => app.fleet[p].name,
        };
        let label = Span::styled(format!(" {name} "), label_style(true, focused));
        bar(frame, Rect { height: 1, ..area }, Line::from(label));
        // The title row above, the row for its keys below.
        content.y += 1;
        content.height = content.height.saturating_sub(2);
    }
    let Some((p, w)) = app.worktree() else {
        frame.render_widget(
            Paragraph::new(Line::styled(" no worktree selected", muted)),
            content,
        );
        return;
    };

    let mut lines = vec![Line::styled(format!(" CHANGES {}", app.files.len()), muted)];
    // The checks keep their rows at the bottom; the files scroll in what is left.
    let checks_rows = if app.fleet[p].worktrees[w].pull_request {
        CHECKS.len() + 2
    } else {
        3
    };
    let room = usize::from(content.height)
        .saturating_sub(checks_rows + 1)
        .max(1);
    let offset = (app.file + 1).saturating_sub(room);
    for (index, path) in app.files.iter().enumerate().skip(offset).take(room) {
        // A long path keeps its end, where the file's name is.
        let shown = if path.chars().count() > width - 4 {
            let tail: String = path
                .chars()
                .skip(path.chars().count() - (width - 5))
                .collect();
            format!("…{tail}")
        } else {
            path.clone()
        };
        let selected = index == app.file;
        let style = match (selected, focused) {
            (true, true) => Style::new().fg(Color::Black).bg(ACCENT),
            (true, false) => Style::new().bg(FAINT),
            _ => Style::new(),
        };
        let mark = if selected { " ▸ " } else { "   " };
        lines.push(Line::styled(
            format!("{mark}{shown:<w$}", w = width - 3),
            style,
        ));
    }
    lines.push(Line::default());
    if app.fleet[p].worktrees[w].pull_request {
        lines.push(Line::styled(" CHECKS  PR #12", muted));
        for (mark, name) in CHECKS {
            let colour = match mark {
                "✓" => GREEN,
                "✗" => RED,
                _ => YELLOW,
            };
            lines.push(Line::from(vec![
                Span::styled(format!(" {mark} "), Style::new().fg(colour)),
                Span::raw(fit(name, width - 4)),
            ]));
        }
    } else {
        lines.push(Line::styled(" CHECKS", muted));
        lines.push(Line::styled("   no pull request", muted));
    }
    frame.render_widget(Paragraph::new(lines), content);
}

/// Variant C's strip: every worktree with its glyph, the selected one lit, and the three
/// terminals' names at the right end.
fn draw_strip(frame: &mut Frame, app: &App, area: Rect) {
    let mut spans = Vec::new();
    for (p, project) in app.fleet.iter().enumerate() {
        let header = app.stops[app.selected] == Stop::Header(p);
        let style = if header {
            label_style(true, true)
        } else {
            Style::new().fg(MUTED).bg(BAR)
        };
        spans.push(Span::styled(format!(" {} ", project.name), style));
        for (w, worktree) in project.worktrees.iter().enumerate() {
            let selected = app.stops[app.selected] == Stop::Worktree(p, w);
            let unseen = if worktree.unseen { "•" } else { " " };
            let glyph = worktree.state.glyph(app.tick);
            if selected {
                let text = format!("{unseen}{glyph} {} ", worktree.name);
                spans.push(Span::styled(text, label_style(true, true)));
            } else {
                let colour = Style::new().fg(state_colour(worktree.state)).bg(BAR);
                spans.push(Span::styled(format!("{unseen}{glyph}"), colour));
                spans.push(Span::raw(format!(" {} ", worktree.name)));
            }
        }
        spans.push(Span::styled("│", Style::new().fg(FAINT)));
    }
    bar(frame, area, Line::from(spans));

    let mut names = Vec::new();
    for (which, name) in TERMINALS.iter().enumerate() {
        let shown = app.worktree().is_some() && which == app.current();
        names.push(Span::styled(
            terminal_label(which, name),
            label_style(shown, app.focus == Focus::Centre),
        ));
    }
    let names = Line::from(names);
    let width = (names.width() as u16).min(area.width);
    let at = Rect {
        x: area.right() - width,
        width,
        ..area
    };
    frame.render_widget(Paragraph::new(names), at);
}

/// The centre when the selection is a project's header, which has no terminals.
fn draw_note(frame: &mut Frame, app: &App, area: Rect) {
    let Stop::Header(p) = app.stops[app.selected] else {
        return;
    };
    let project = &app.fleet[p];
    let text = if project.worktrees.is_empty() {
        format!("project {}: no worktrees yet", project.name)
    } else {
        format!(
            "project {}: select one of its worktrees to see its terminals",
            project.name
        )
    };
    let middle = Rect {
        y: area.y + area.height / 2,
        height: 1,
        ..area
    };
    frame.render_widget(
        Paragraph::new(Line::styled(text, Style::new().fg(MUTED))).alignment(Alignment::Center),
        middle,
    );
}

fn draw_pane(frame: &mut Frame, app: &App, g: &Geometry, pane: &Pane) {
    let focused = app.focus == Focus::Centre && app.current() == pane.which;
    if let Some(title) = pane.title {
        let mut spans = Vec::new();
        for (which, name) in TERMINALS.iter().enumerate() {
            if g.title_lists_all || which == pane.which {
                spans.push(Span::styled(
                    terminal_label(which, name),
                    label_style(which == app.current(), app.focus == Focus::Centre),
                ));
            }
        }
        bar(frame, title, Line::from(spans));
    }

    let body = pane.body;
    let name = TERMINALS[pane.which];
    let Some(term) = app.term(pane.which) else {
        // Snapshot mode starts nothing: the panel only says what would be there.
        let command = app.commands[pane.which][0].rsplit('/').next().unwrap_or("");
        let middle = Rect {
            y: body.y + body.height / 2,
            height: 1,
            ..body
        };
        frame.render_widget(
            Paragraph::new(Line::styled(
                format!("{name} terminal ({command})"),
                Style::new().fg(MUTED),
            ))
            .alignment(Alignment::Center),
            middle,
        );
        return;
    };

    let parser = term.parser();
    let screen = parser.screen();
    // The child's cursor is the real cursor of this terminal, placed below; the widget's own
    // drawing of one stays off.
    let widget = PseudoTerminal::new(screen).cursor(Cursor::default().visibility(false));
    frame.render_widget(widget, body);

    if term.exited() {
        let last = Rect {
            y: body.bottom() - 1,
            height: 1,
            ..body
        };
        frame.render_widget(
            Paragraph::new(" [exited]  Enter starts it again ")
                .style(Style::new().fg(Color::Black).bg(YELLOW)),
            last,
        );
    } else if screen.scrollback() > 0 {
        let text = format!(" {} lines up ", screen.scrollback());
        let width = (text.len() as u16).min(body.width);
        let corner = Rect {
            x: body.right() - width,
            width,
            height: 1,
            ..body
        };
        frame.render_widget(
            Paragraph::new(text).style(Style::new().fg(Color::Black).bg(YELLOW)),
            corner,
        );
    } else if focused && !app.help && !screen.hide_cursor() {
        let (row, col) = screen.cursor_position();
        if row < body.height && col < body.width {
            frame.set_cursor_position((body.x + col, body.y + row));
        }
    }
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let prefix = app.prefix_label();
    let rule = Span::styled(" │ ", Style::new().fg(FAINT).bg(BAR));
    let mut spans = Vec::new();

    match app.stops[app.selected] {
        Stop::Worktree(p, w) => {
            let worktree = &app.fleet[p].worktrees[w];
            let colour = Style::new().fg(state_colour(worktree.state)).bg(BAR);
            spans.push(Span::styled(
                format!(" {} ", worktree.state.glyph(app.tick)),
                colour,
            ));
            spans.push(Span::styled(
                worktree.name,
                Style::new().add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(format!(" {}", worktree.state.word()), colour));
        }
        Stop::Header(p) => {
            spans.push(Span::raw(format!(" project {}", app.fleet[p].name)));
        }
    }
    spans.push(rule.clone());
    spans.push(Span::raw(format!("focus {}", app.focus.word())));
    if app.zoom {
        spans.push(Span::styled(" ZOOM", Style::new().fg(YELLOW)));
    }
    spans.push(rule.clone());
    spans.push(Span::raw(app.variant.label()));

    // Without a right column, the status bar carries its summary.
    if app.variant != Variant::A
        && let Some((p, w)) = app.worktree()
    {
        spans.push(rule.clone());
        spans.push(Span::raw(format!("{} files ", app.files.len())));
        if app.fleet[p].worktrees[w].pull_request {
            let count = |mark: &str| CHECKS.iter().filter(|(m, _)| *m == mark).count();
            spans.push(Span::styled(
                format!("✓{} ", count("✓")),
                Style::new().fg(GREEN),
            ));
            spans.push(Span::styled(
                format!("✗{} ", count("✗")),
                Style::new().fg(RED),
            ));
            spans.push(Span::styled(
                format!("·{}", count("·")),
                Style::new().fg(YELLOW),
            ));
        } else {
            spans.push(Span::styled("no PR", Style::new().fg(MUTED)));
        }
    }
    spans.push(rule);

    let message = app
        .message
        .as_ref()
        .filter(|(_, since)| since.elapsed() < MESSAGE_FOR);
    if app.pending {
        spans.push(Span::styled(
            format!("{prefix} … (press ? for the keys)"),
            Style::new().fg(YELLOW),
        ));
    } else if let Some((text, _)) = message {
        spans.push(Span::styled(text.clone(), Style::new().fg(YELLOW)));
    } else {
        spans.push(Span::styled(
            format!("{prefix} ? help  {prefix} v variant  {prefix} z zoom  {prefix} q quit"),
            Style::new().fg(MUTED),
        ));
    }

    // Not part of the design: the prototype's own switch between the variants.
    let switch = format!(" prototype  ◀ {} ▶  {prefix} v ", app.variant.label());
    let width = (Span::raw(switch.as_str()).width() as u16).min(area.width);
    bar(frame, area, Line::default());
    let left = Rect {
        width: area.width.saturating_sub(width + 1),
        ..area
    };
    bar(frame, left, Line::from(spans));
    let at = Rect {
        x: area.right() - width,
        width,
        ..area
    };
    frame.render_widget(
        Paragraph::new(switch).style(
            Style::new()
                .fg(Color::Black)
                .bg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        at,
    );
}

fn draw_help(frame: &mut Frame, app: &App) {
    let p = app.prefix_label();
    let rows: Vec<(String, &str)> = vec![
        (
            format!("{p} h / l"),
            "focus the column to the left / right (also ← →)",
        ),
        (
            format!("{p} j / k"),
            "select the next / previous worktree or project",
        ),
        (format!("{p} 1 2 3"), "the worktree's agent / nvim / shell"),
        (format!("{p} n / p"), "the next / previous of the three"),
        (format!("{p} z"), "zoom the panel with the focus, and back"),
        (
            format!("{p} |"),
            "agent and nvim side by side, and back (A, C)",
        ),
        (format!("{p} s"), "show / hide the side menu"),
        (format!("{p} r"), "show / hide the changes and checks"),
        (
            format!("{p} v"),
            "the next layout variant (A Orca, B Panes, C Focus)",
        ),
        (
            format!("{p} {p}"),
            "send the control key itself to the terminal",
        ),
        (format!("{p} ?"), "this help"),
        (format!("{p} q"), "quit"),
        (String::new(), ""),
        (
            "side menu".to_owned(),
            "j / k move, Enter or l to the terminal, q quit",
        ),
        (
            "right column".to_owned(),
            "j / k move, Enter opens in nvim, h back",
        ),
        (
            "on top (C)".to_owned(),
            "Esc closes the side menu or the right column",
        ),
        (
            "a terminal".to_owned(),
            "takes every key but the prefix; Enter restarts",
        ),
        (String::new(), "one that exited; the wheel scrolls it"),
        ("mouse".to_owned(), "a click focuses the panel under it"),
    ];
    let area = frame.area();
    let width = 74.min(area.width);
    let height = (rows.len() as u16 + 4).min(area.height);
    let outer = Rect {
        x: (area.width - width) / 2,
        y: (area.height - height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, outer);
    let lines: Vec<Line> = std::iter::once(Line::default())
        .chain(rows.into_iter().map(|(keys, what)| {
            Line::from(vec![
                Span::styled(format!("  {keys:<14}"), Style::new().fg(ACCENT)),
                Span::raw(what),
            ])
        }))
        .collect();
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(ACCENT))
        .title(" keys ")
        .title_bottom(" any key closes ");
    frame.render_widget(Paragraph::new(lines).block(block), outer);
}
