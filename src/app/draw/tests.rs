//! App: the application's screens, drawn on ratatui's `TestBackend` and compared with the
//! snapshots in `src/app/snapshots/` (#174). A snapshot is the screen's text, then the colour of
//! each cell's text, then of its background, one letter per theme token (see [`LETTERS`]).
//!
//! When a screen changes, the test writes what it drew beside the snapshot as `<name>.new.txt`
//! and fails; look at it, and rename it over `<name>.txt` to accept it.

use std::path::{Path, PathBuf};
use std::time::Instant;

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::layout::Rect;
use ratatui::style::Color;

use crate::adapters::fs;
use crate::adapters::holder::client::Heard;
use crate::app::draw::frame;
use crate::app::fixture;
use crate::app::paint::Mode;
use crate::app::state::App;
use crate::app::theme::{self, Tok};
use crate::domain::agent::HolderStatus;
use crate::domain::fleet::Worktree;
use crate::domain::pr::{Checks, Found, PrFacts, PrState, PullRequest};
use crate::domain::project::ProjectFleet;
use crate::domain::state::State;

/// The clock every snapshot is drawn at: 10 days after the epoch.
const NOW: u64 = 10 * 86_400_000;

/// Each token's letter in a snapshot's colour grids; any other colour is `?`.
const LETTERS: [(Tok, char); 15] = [
    (theme::CANVAS, 'c'),
    (theme::BG, 'b'),
    (theme::PANEL, 'p'),
    (theme::RAISED, 'r'),
    (theme::SELECTION, 's'),
    (theme::FOCUS_SEL, 'f'),
    (theme::FOCUS_BG, 'g'),
    (theme::BORDER, 'o'),
    (theme::TEXT, 'T'),
    (theme::TEXT_2, 't'),
    (theme::ACCENT, 'A'),
    (theme::WAITING, 'W'),
    (theme::WORKING, 'K'),
    (theme::DONE, 'D'),
    (theme::FAILED, 'F'),
];

fn minutes_ago(minutes: u64) -> u64 {
    NOW - minutes * 60_000
}

fn with_pr(mut worktree: Worktree, number: u64, checks: Checks) -> Worktree {
    worktree.pr = Some(PrFacts {
        fetched_at: NOW,
        found: Found::Pr(PullRequest {
            number,
            title: "Fix the login".to_owned(),
            state: PrState::Open,
            url: String::new(),
            checks,
            head: None,
        }),
    });
    worktree
}

/// A worktree whose claude-code agent last said `message`, `minutes` ago.
fn busy(path: &str, state: State, message: &str, minutes: u64) -> Worktree {
    let mut worktree = fixture::worktree(path, state);
    worktree.agent = Some("claude-code".to_owned());
    worktree.activity = Some(message.to_owned());
    worktree.last_activity = Some(minutes_ago(minutes));
    worktree
}

/// The `app` project: `fix-login`, its agent running and its screen open, and three more; and
/// the `api` project with one worktree.
fn fleets() -> Vec<ProjectFleet> {
    let mut fix = busy(
        "/code/app.wt/fix-login",
        State::Waiting,
        "Allow edits to src/auth/session.ts?",
        2,
    );
    fix.holder = HolderStatus::Alive;
    let fix = with_pr(fix, 131, Checks::Pending);
    let mut tests = busy(
        "/code/app.wt/run-tests",
        State::Working,
        "Running the suite",
        0,
    );
    tests.unseen = true;
    let mut docs = busy(
        "/code/app.wt/docs",
        State::Finished,
        "Updated the README",
        41,
    );
    docs.unseen = true;
    let docs = with_pr(docs, 128, Checks::Passing);
    let flaky = busy(
        "/code/app.wt/flaky",
        State::Failed,
        "API error: overloaded",
        180,
    );
    let api = busy("/code/api.wt/pagination", State::Idle, "Done", 3000);
    vec![
        fixture::project("/code/app", vec![fix, tests, docs, flaky]),
        fixture::project("/code/api", vec![api]),
    ]
}

/// The application on a screen `width` by `height`, its fleets read and the open worktree's agent
/// showing a prompt.
fn app(width: u16, height: u16, fleets: Vec<ProjectFleet>) -> App {
    let mut app = App::new(Rect::new(0, 0, width, height), None);
    app.set_fleets(fleets);
    if let Some(panel) = app.panel.as_mut() {
        panel.heard(Heard::Output(
            b"\x1b[1mfake claude\x1b[0m ready\r\n\r\n\x1b[33m? Allow edits to src/auth/session.ts?\x1b[0m\r\n  1. Yes\r\n  2. No\r\n\r\n> "
                .to_vec(),
        ));
    }
    app
}

fn letter(colour: Color) -> char {
    LETTERS
        .iter()
        .find(|(tok, _)| tok.color() == colour)
        .map_or('?', |(_, letter)| *letter)
}

/// The screen as a snapshot: its text, its text colours and its background colours.
fn snapshot(buf: &Buffer) -> String {
    let area = buf.area;
    let mut grids = [String::new(), String::new(), String::new()];
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buf[(x, y)];
            grids[0].push_str(cell.symbol());
            grids[1].push(letter(cell.fg));
            grids[2].push(letter(cell.bg));
        }
        for grid in &mut grids {
            grid.push('\n');
        }
    }
    grids.join("\n")
}

fn draw(app: &App) -> String {
    let area = app.layout.status;
    let height = area.y + 1;
    let mut terminal = Terminal::new(TestBackend::new(area.width, height)).unwrap();
    terminal
        .draw(|f| frame(f, app, NOW, Mode::Truecolor))
        .unwrap();
    snapshot(terminal.backend().buffer())
}

fn snapshot_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/app/snapshots")
        .join(name)
}

/// Draws `app` and compares the screen with the snapshot `name`.
fn check(name: &str, app: &App) {
    let drawn = draw(app);
    let path = snapshot_path(&format!("{name}.txt"));
    let kept = fs::read_optional(&path).unwrap().map(String::from_utf8);
    if kept
        .as_ref()
        .is_some_and(|kept| kept.as_deref() == Ok(drawn.as_str()))
    {
        return;
    }
    let new = snapshot_path(&format!("{name}.new.txt"));
    fs::write_atomically(&new, drawn.as_bytes()).unwrap();
    panic!(
        "the screen `{name}` differs from {}; look at {} and rename it over the snapshot to \
         accept it",
        path.display(),
        new.display()
    );
}

fn press(app: &mut App, code: KeyCode, modifiers: KeyModifiers) {
    app.key(KeyEvent::new(code, modifiers), Instant::now());
}

#[test]
fn the_main_screen() {
    check("main", &app(150, 44, fleets()));
}

#[test]
fn the_prefix_armed_with_the_side_menu_focused() {
    let mut app = app(120, 30, fleets());
    press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
    press(&mut app, KeyCode::Char('h'), KeyModifiers::NONE);
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
    press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
    check("prefix", &app);
}

#[test]
fn a_project_with_no_worktrees() {
    let mut fleets = fleets();
    fleets.insert(1, fixture::project("/code/notes", Vec::new()));
    check("no-worktrees", &app(120, 30, fleets));
}

#[test]
fn a_gone_project() {
    let mut fleets = fleets();
    fleets.insert(0, fixture::gone("/code/old"));
    check("gone", &app(120, 30, fleets));
}

#[test]
fn a_long_list_scrolls_to_the_open_worktree_and_to_its_end_from_the_last() {
    let mut fleets = fleets();
    fleets.push(fixture::gone("/code/old"));
    let mut app = app(120, 20, fleets);
    let text = |app: &App| {
        draw(app)
            .split("\n\n")
            .next()
            .unwrap_or_default()
            .to_owned()
    };
    assert!(text(&app).contains("fix-login"));
    assert!(!text(&app).contains("pagination"), "below the screen");
    press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
    press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
    let shown = text(&app);
    assert!(shown.contains("pagination"), "{shown}");
    assert!(shown.contains("the project's"), "{shown}");
    assert!(
        !shown.contains("fix-login  "),
        "scrolled past the top: {shown}"
    );
}

#[test]
fn under_100_columns_only_the_centre() {
    check("narrow", &app(90, 24, fleets()));
}

#[test]
fn a_worktree_with_no_agent_says_how_to_start_one() {
    let mut app = app(120, 30, fleets());
    press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
    press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
    check("no-agent", &app);
}
