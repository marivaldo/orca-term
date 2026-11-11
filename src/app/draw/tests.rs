//! App: the application's screens, drawn on ratatui's `TestBackend` and compared with the
//! snapshots in `src/app/snapshots/` (#174, #176, #177). A snapshot is the screen's text, then the colour of
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
use ratatui::style::{Color, Modifier};

use crate::adapters::fs;
use crate::adapters::holder::client::Heard;
use crate::adapters::pty::Printed;
use crate::app::draw::frame;
use crate::app::fixture;
use crate::app::paint::Mode;
use crate::app::program::{Kind, ProgramId};
use crate::app::state::App;
use crate::app::theme::{self, Tok};
use crate::domain::agent::{AgentEnd, HolderStatus};
use crate::domain::branch::Branch;
use crate::domain::changes::{Change, ChangedFile, Changes, Lines};
use crate::domain::detail::Drift;
use crate::domain::fleet::Worktree;
use crate::domain::pr::{CheckResult, CheckRun, Checks, Found, PrFacts, PrState, PullRequest};
use crate::domain::project::ProjectFleet;
use crate::domain::state::State;

/// The clock every snapshot is drawn at: 10 days after the epoch.
const NOW: u64 = 10 * 86_400_000;

/// Each token's letter in a snapshot's colour grids; any other colour is `?`.
const LETTERS: [(Tok, char); 16] = [
    (theme::VEIL, 'v'),
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
            runs: Vec::new(),
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

/// `fix-login`'s changes, as the design shows them: two files modified, one added, one deleted,
/// two commits ahead of `main`.
fn changes() -> Changes {
    let file = |path: &str, change, added, deleted| ChangedFile {
        path: path.to_owned(),
        change,
        lines: Some(Lines { added, deleted }),
    };
    Changes {
        files: vec![
            file("app/templates/password.html", Change::Deleted, 0, 10),
            file("src/auth/flag.rs", Change::Added, 31, 0),
            file("src/auth/login.rs", Change::Modified, 18, 9),
            file("src/auth/session.rs", Change::Modified, 6, 2),
        ],
        drift: Some(Drift {
            against: Branch::new("main"),
            ahead: 2,
            behind: 0,
        }),
    }
}

/// The application on a screen `width` by `height`, its fleets read, the open worktree's agent
/// showing a prompt and its changes read.
fn app(width: u16, height: u16, fleets: Vec<ProjectFleet>) -> App {
    let mut app = App::new(Rect::new(0, 0, width, height), None);
    app.set_fleets(fleets);
    if let Some(open) = app.open.clone() {
        app.changes_read(open, Ok(changes()));
    }
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

/// A cell's attributes as one character, for the `NO_COLOR` snapshots: the sum of bold 1, dim 2,
/// italic 4, underlined 8 and reversed 16, written in base 32 (`0`-`9`, then `a`-`v`).
fn attributes(modifier: Modifier) -> char {
    let bits = [
        Modifier::BOLD,
        Modifier::DIM,
        Modifier::ITALIC,
        Modifier::UNDERLINED,
        Modifier::REVERSED,
    ]
    .iter()
    .enumerate()
    .filter(|(_, bit)| modifier.contains(**bit))
    .fold(0, |sum, (at, _)| sum | (1 << at));
    char::from_digit(bits, 32).unwrap_or('?')
}

/// The screen as a snapshot: its text, its text colours and its background colours; in `NO_COLOR`
/// (`Mode::Mono`), its text and each cell's attributes instead.
fn snapshot(buf: &Buffer, mode: Mode) -> String {
    let area = buf.area;
    let mut grids = [String::new(), String::new(), String::new()];
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buf[(x, y)];
            grids[0].push_str(cell.symbol());
            if mode == Mode::Mono {
                grids[1].push(attributes(cell.modifier));
            } else {
                grids[1].push(letter(cell.fg));
                grids[2].push(letter(cell.bg));
            }
        }
        for grid in &mut grids {
            grid.push('\n');
        }
    }
    if mode == Mode::Mono {
        return grids[..2].join("\n");
    }
    grids.join("\n")
}

fn draw(app: &App) -> String {
    draw_in(app, Mode::Truecolor)
}

fn draw_in(app: &App, mode: Mode) -> String {
    let area = app.layout.status;
    let height = area.y + 1;
    let mut terminal = Terminal::new(TestBackend::new(area.width, height)).unwrap();
    terminal.draw(|f| frame(f, app, NOW, mode)).unwrap();
    snapshot(terminal.backend().buffer(), mode)
}

fn snapshot_path(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src/app/snapshots")
        .join(name)
}

/// Draws `app` and compares the screen with the snapshot `name`.
fn check(name: &str, app: &App) {
    check_in(name, app, Mode::Truecolor);
}

/// Draws `app` for a terminal in `mode` and compares the screen with the snapshot `name`.
fn check_in(name: &str, app: &App, mode: Mode) {
    let drawn = draw_in(app, mode);
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
fn the_primary_checkout_comes_first_in_its_project_with_its_tag() {
    let mut fleets = fleets();
    if let Ok(fleet) = fleets[0].fleet.as_mut() {
        // The first worktree, so the one the application opens.
        let mut primary = fixture::primary("/code/app", State::Idle);
        primary.last_activity = Some(minutes_ago(300));
        fleet.worktrees.insert(0, primary);
    }
    check("primary", &app(150, 44, fleets));
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

/// A Neovim's screen, as a stand-in draws it: numbered lines, the end of the buffer and a
/// statusline with the mode on the WORKING colour.
const NVIM_SCREEN: &[u8] = b"\x1b[2J\x1b[H\x1b[90m  1 \x1b[0m\x1b[34muse\x1b[0m std::io;\r\n\x1b[90m  2 \x1b[0m\r\n\x1b[90m  3 \x1b[0m\x1b[34mfn\x1b[0m \x1b[1mmain\x1b[0m() {}\r\n\x1b[90m~\r\n~\x1b[0m\r\n\x1b[44;30m NORMAL \x1b[0m src/main.rs";

/// Presses `^a` and then `code`, and sizes the panels to the new view, as the run loop does.
fn prefixed(app: &mut App, code: KeyCode) {
    press(app, KeyCode::Char('a'), KeyModifiers::CONTROL);
    press(app, code, KeyModifiers::NONE);
    let status = app.layout.status;
    app.fit(Rect::new(0, 0, status.width, status.y + 1));
}

/// What the open worktree's Neovim printed: [`NVIM_SCREEN`].
fn nvim_draws(app: &mut App) {
    let id = ProgramId {
        worktree: PathBuf::from("/code/app.wt/fix-login"),
        kind: Kind::Nvim,
    };
    app.printed(&id, Printed::Output(NVIM_SCREEN.to_vec()));
}

#[test]
fn the_nvim_tab() {
    let mut app = app(150, 44, fleets());
    prefixed(&mut app, KeyCode::Char('2'));
    nvim_draws(&mut app);
    check("nvim-tab", &app);
}

#[test]
fn the_split_with_the_agent_focused_and_the_prefix_armed() {
    let mut app = app(150, 44, fleets());
    prefixed(&mut app, KeyCode::Char('|'));
    nvim_draws(&mut app);
    press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
    check("split-agent", &app);
}

#[test]
fn the_split_with_neovim_focused() {
    let mut app = app(150, 44, fleets());
    prefixed(&mut app, KeyCode::Char('|'));
    prefixed(&mut app, KeyCode::Char('l'));
    nvim_draws(&mut app);
    check("split-nvim", &app);
}

#[test]
fn the_zoom_of_neovim_from_the_split() {
    let mut app = app(150, 44, fleets());
    prefixed(&mut app, KeyCode::Char('|'));
    prefixed(&mut app, KeyCode::Char('l'));
    prefixed(&mut app, KeyCode::Char('z'));
    nvim_draws(&mut app);
    check("zoom", &app);
}

#[test]
fn a_shell_that_ended_says_how_to_start_it_again() {
    let mut app = app(120, 30, fleets());
    prefixed(&mut app, KeyCode::Char('3'));
    let id = ProgramId {
        worktree: PathBuf::from("/code/app.wt/fix-login"),
        kind: Kind::Shell,
    };
    app.printed(&id, Printed::Output(b"$ exit\r\n".to_vec()));
    app.printed(&id, Printed::Ended("exit 0".to_owned()));
    check("shell-ended", &app);
}

#[test]
fn a_worktree_with_no_agent_says_how_to_start_one() {
    let mut app = app(120, 30, fleets());
    press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
    press(&mut app, KeyCode::Char('k'), KeyModifiers::NONE);
    check("no-agent", &app);
}

/// `fix-login`'s pull request as a draft with three checks: one passed, one failed, one running.
fn draft_with_checks() -> Vec<ProjectFleet> {
    let mut fleets = fleets();
    if let Ok(fleet) = fleets[0].fleet.as_mut()
        && let Some(PrFacts {
            found: Found::Pr(pr),
            ..
        }) = fleet.worktrees[0].pr.as_mut()
    {
        pr.state = PrState::Draft;
        let run = |name: &str, result| CheckRun {
            name: name.to_owned(),
            result,
        };
        pr.runs = vec![
            run("test", CheckResult::Pass),
            run("lint", CheckResult::Fail),
            run("build", CheckResult::Pending),
        ];
    }
    fleets
}

#[test]
fn the_right_column_with_a_pull_request() {
    check("right-column", &app(180, 48, draft_with_checks()));
}

#[test]
fn the_right_column_without_a_pull_request_focused() {
    let mut fleets = fleets();
    if let Ok(fleet) = fleets[0].fleet.as_mut() {
        fleet.worktrees[0].pr = Some(PrFacts {
            fetched_at: NOW,
            found: Found::NoPr,
        });
    }
    let mut app = app(180, 48, fleets);
    prefixed(&mut app, KeyCode::Char('l'));
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
    check("right-column-no-pr", &app);
}

#[test]
fn under_150_columns_the_summary_shows_in_the_centres_header() {
    check("summary", &app(120, 36, fleets()));
}

#[test]
fn under_150_columns_f_lays_the_right_column_over_the_centre() {
    let mut app = app(120, 36, draft_with_checks());
    prefixed(&mut app, KeyCode::Char('f'));
    press(&mut app, KeyCode::Char('j'), KeyModifiers::NONE);
    check("overlay", &app);
}

/// What the failed agent left in its log: its work, then the error it died of.
const FAILED_LOG: &[u8] = b"\x1b[1mclaude\x1b[0m  \xc2\xb7  app/fix-login\r\n\r\n> Make the login retry with backoff.\r\n\r\n\x1b[32m\xe2\x97\x8f\x1b[0m \x1b[1mRead\x1b[0m src/auth/session.ts\r\n\r\n\x1b[31merror:\x1b[0m stream closed: connection reset by peer\r\nclaude exited with code 1\r\n";

/// The application whose open `fix-login` agent failed 12 minutes ago, after 41 minutes, its log
/// read.
fn failed_app(width: u16, height: u16) -> App {
    let mut fleets = fleets();
    if let Ok(fleet) = fleets[0].fleet.as_mut() {
        let fix = &mut fleet.worktrees[0];
        fix.holder = HolderStatus::Gone;
        fix.state = State::Failed;
        fix.changed_files = Some(2);
        fix.agent_end = Some(AgentEnd::new(
            Some(1),
            None,
            false,
            minutes_ago(53),
            minutes_ago(12),
        ));
    }
    let mut app = App::new(Rect::new(0, 0, width, height), None);
    app.set_fleets(fleets);
    app.changes_read(PathBuf::from("/code/app.wt/fix-login"), Ok(changes()));
    app.log_read(
        Path::new("/code/app.wt/fix-login"),
        Ok(FAILED_LOG.to_vec()),
        Instant::now(),
    );
    app
}

#[test]
fn a_failed_agent_shows_its_last_output_and_the_card_under_it() {
    check("failed", &failed_app(150, 44));
}

#[test]
fn l_shows_the_failed_agents_log() {
    let mut app = failed_app(150, 44);
    press(&mut app, KeyCode::Char('l'), KeyModifiers::NONE);
    check("log", &app);
}

#[test]
fn the_picker_floats_over_a_narrow_screen_filtered_by_what_is_typed() {
    let mut app = app(90, 30, fleets());
    prefixed(&mut app, KeyCode::Char('w'));
    press(&mut app, KeyCode::Char('o'), KeyModifiers::NONE);
    check("picker", &app);
}

#[test]
fn help_lists_the_keys() {
    let mut app = app(120, 36, fleets());
    prefixed(&mut app, KeyCode::Char('?'));
    check("help", &app);
}

#[test]
fn the_side_menu_filters_as_it_is_typed() {
    let mut app = app(120, 36, fleets());
    prefixed(&mut app, KeyCode::Char('h'));
    press(&mut app, KeyCode::Char('/'), KeyModifiers::NONE);
    for c in "doc".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
    }
    check("filter", &app);
}

#[test]
fn n_asks_the_new_worktrees_name_under_its_project() {
    let mut app = app(150, 44, fleets());
    prefixed(&mut app, KeyCode::Char('h'));
    press(&mut app, KeyCode::Char('n'), KeyModifiers::NONE);
    for c in "retry-jobs".chars() {
        press(&mut app, KeyCode::Char(c), KeyModifiers::NONE);
    }
    check("new-worktree", &app);
}

/// The design's 4f: `NO_COLOR` at 180 by 48, the split with Neovim focused and the prefix armed.
#[test]
fn no_color_keeps_attributes_only() {
    let mut app = app(180, 48, fleets());
    prefixed(&mut app, KeyCode::Char('|'));
    prefixed(&mut app, KeyCode::Char('l'));
    nvim_draws(&mut app);
    press(&mut app, KeyCode::Char('a'), KeyModifiers::CONTROL);
    check_in("no-color", &app, Mode::Mono);
}

/// The design's 4g: `NO_COLOR` at 120 by 36, the side menu focused, its items compact.
#[test]
fn no_color_at_120_by_36_with_the_side_menu_focused() {
    let mut app = app(120, 36, fleets());
    prefixed(&mut app, KeyCode::Char('h'));
    check_in("no-color-120x36", &app, Mode::Mono);
}
