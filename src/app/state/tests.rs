//! App: the state's tests: what each key and each change does (#174, #176, #177).

use super::*;
use crate::adapters::pty::Printed;
use crate::app::fixture;
use crate::app::program::Status as ProgramStatus;
use crate::domain::agent::HolderStatus;
use crate::domain::state::State;

const SCREEN: Rect = Rect::new(0, 0, 120, 36);

fn press(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn ctrl_a() -> KeyEvent {
    KeyEvent::new(KeyCode::Char('a'), KeyModifiers::CONTROL)
}

/// The application on two projects: `api` with `fix` (its agent running) and `docs` (none),
/// and `web` with `ui`.
fn app() -> (App, Instant) {
    let mut fix = fixture::worktree("/p/api.wt/fix", State::Working);
    fix.holder = HolderStatus::Alive;
    let fleets = vec![
        fixture::project(
            "/p/api",
            vec![fix, fixture::worktree("/p/api.wt/docs", State::Idle)],
        ),
        fixture::project(
            "/p/web",
            vec![fixture::worktree("/p/web.wt/ui", State::NoAgent)],
        ),
    ];
    let mut app = App::new(SCREEN, None);
    let effects = app.set_fleets(fleets);
    assert_eq!(effects, [Effect::Attach(PathBuf::from("/p/api.wt/fix"))]);
    (app, Instant::now())
}

fn open(app: &App) -> &str {
    app.open
        .as_deref()
        .and_then(Path::to_str)
        .unwrap_or_default()
}

#[test]
fn keys_go_to_the_agent_until_the_prefix_is_armed() {
    let (mut app, now) = app();
    assert_eq!(
        app.key(press(KeyCode::Char('j')), now),
        [Effect::Type(b"j".to_vec())]
    );
    assert_eq!(app.key(ctrl_a(), now), []);
    assert!(app.prefix.is_some());
    assert_eq!(app.key(press(KeyCode::Char('j')), now), []);
    assert_eq!(open(&app), "/p/api.wt/docs", "^a j opens the next worktree");
    assert!(app.prefix.is_none(), "the prefix lasts one key");
}

#[test]
fn the_prefix_sends_a_literal_ctrl_a_quits_and_cancels() {
    let (mut app, now) = app();
    app.key(ctrl_a(), now);
    assert_eq!(
        app.key(press(KeyCode::Char('a')), now),
        [Effect::Type(vec![0x01])]
    );
    app.key(ctrl_a(), now);
    assert_eq!(app.key(press(KeyCode::Esc), now), []);
    assert_eq!(
        app.key(press(KeyCode::Char('q')), now),
        [Effect::Type(b"q".to_vec())]
    );
    app.key(ctrl_a(), now);
    assert_eq!(app.key(press(KeyCode::Char('q')), now), [Effect::Quit]);
}

#[test]
fn the_prefix_expires() {
    let (mut app, now) = app();
    app.key(ctrl_a(), now);
    app.tick(now + PREFIX_FOR);
    assert!(app.prefix.is_none());
    app.key(ctrl_a(), now);
    let later = now + PREFIX_FOR + Duration::from_millis(1);
    assert_eq!(
        app.key(press(KeyCode::Char('q')), later),
        [Effect::Type(b"q".to_vec())]
    );
}

#[test]
fn the_previous_worktree_of_the_first_is_the_last() {
    let (mut app, now) = app();
    app.key(ctrl_a(), now);
    app.key(press(KeyCode::Char('k')), now);
    assert_eq!(open(&app), "/p/web.wt/ui");
}

#[test]
fn the_side_menu_moves_its_cursor_and_opens_with_enter() {
    let (mut app, now) = app();
    app.key(ctrl_a(), now);
    app.key(press(KeyCode::Char('h')), now);
    assert_eq!(app.focus, Focus::SideMenu);
    assert_eq!(app.key(press(KeyCode::Char('j')), now), []);
    assert_eq!(app.key(press(KeyCode::Down), now), []);
    assert_eq!(app.cursor, Some(PathBuf::from("/p/web.wt/ui")));
    assert_eq!(
        open(&app),
        "/p/api.wt/fix",
        "moving the cursor opens nothing"
    );
    assert_eq!(app.key(press(KeyCode::Enter), now), []);
    assert_eq!(open(&app), "/p/web.wt/ui");
    assert_eq!(app.focus, Focus::Centre);
    app.key(ctrl_a(), now);
    app.key(press(KeyCode::Char('l')), now);
    assert_eq!(app.focus, Focus::Centre);
}

#[test]
fn enter_starts_the_agent_where_none_runs() {
    let (mut app, now) = app();
    app.open(PathBuf::from("/p/api.wt/docs"));
    assert_eq!(app.key(press(KeyCode::Char('x')), now), []);
    let effects = app.key(press(KeyCode::Enter), now);
    assert!(
        matches!(&effects[..], [Effect::Start(path, _)] if path == Path::new("/p/api.wt/docs")),
        "{effects:?}"
    );
    assert_eq!(app.key(press(KeyCode::Enter), now), [], "already starting");
    let path = Path::new("/p/api.wt/docs");
    assert_eq!(
        app.started(path, Ok(()), now),
        [Effect::Attach(path.to_owned())]
    );
    app.attached(path);
    assert_eq!(
        app.key(press(KeyCode::Char('x')), now),
        [Effect::Type(b"x".to_vec())]
    );
}

#[test]
fn a_failed_start_says_why() {
    let (mut app, now) = app();
    let path = PathBuf::from("/p/api.wt/docs");
    app.open(path.clone());
    app.key(press(KeyCode::Enter), now);
    app.started(&path, Err("claude is not installed".to_owned()), now);
    assert_eq!(
        app.panel.as_ref().map(|panel| &panel.status),
        Some(&Status::NoAgent)
    );
    assert_eq!(
        app.message.as_ref().map(|(text, _)| text.as_str()),
        Some("claude is not installed")
    );
}

#[test]
fn an_agent_started_elsewhere_is_attached_on_the_next_read() {
    let (mut app, _) = app();
    app.open(PathBuf::from("/p/api.wt/docs"));
    let mut fleets = std::mem::take(&mut app.fleets);
    if let Ok(fleet) = fleets[0].fleet.as_mut() {
        fleet.worktrees[1].holder = HolderStatus::Alive;
    }
    assert_eq!(
        app.set_fleets(fleets),
        [Effect::Attach(PathBuf::from("/p/api.wt/docs"))]
    );
}

#[test]
fn under_100_columns_the_focus_stays_in_the_centre() {
    let (mut app, now) = app();
    app.key(ctrl_a(), now);
    app.key(press(KeyCode::Char('h')), now);
    assert_eq!(app.focus, Focus::SideMenu);
    app.fit(Rect::new(0, 0, 90, 30));
    assert_eq!(app.focus, Focus::Centre);
    app.key(ctrl_a(), now);
    app.key(press(KeyCode::Char('h')), now);
    assert_eq!(app.focus, Focus::Centre);
}

#[test]
fn a_paste_reaches_the_agent_bracketed_when_it_asked() {
    let (mut app, now) = app();
    assert_eq!(app.paste("a\nb"), [Effect::Type(b"a\rb".to_vec())]);
    app.key(ctrl_a(), now);
    app.key(press(KeyCode::Char('h')), now);
    assert_eq!(app.paste("a"), [], "the side menu takes no paste");
}

#[test]
fn the_panel_follows_the_screens_size() {
    let (mut app, _) = app();
    assert_eq!(app.fit(SCREEN), []);
    assert_eq!(
        app.fit(Rect::new(0, 0, 160, 40)),
        [Effect::SizeAgent(Size { rows: 35, cols: 84 })],
        "160 columns: the side menu and the right column take 34 each"
    );
}

#[test]
fn the_application_opens_the_worktree_it_was_started_in() {
    let (app, _) = app();
    let mut started = App::new(SCREEN, Some(PathBuf::from("/p/web.wt/ui/src")));
    started.set_fleets(std::mem::take(&mut { app }.fleets));
    assert_eq!(open(&started), "/p/web.wt/ui");
}

#[test]
fn started_in_a_worktree_inside_the_primary_checkout_it_opens_that_worktree() {
    let fleets = || {
        vec![fixture::project(
            "/p/api",
            vec![
                fixture::primary("/p/api", State::Idle),
                fixture::worktree("/p/api/.worktrees/fix", State::NoAgent),
            ],
        )]
    };
    let mut inside = App::new(SCREEN, Some(PathBuf::from("/p/api/.worktrees/fix/src")));
    inside.set_fleets(fleets());
    assert_eq!(open(&inside), "/p/api/.worktrees/fix");
    let mut primary = App::new(SCREEN, Some(PathBuf::from("/p/api/src")));
    primary.set_fleets(fleets());
    assert_eq!(
        open(&primary),
        "/p/api",
        "the primary checkout opens like any"
    );
}

fn prefixed(app: &mut App, now: Instant, code: KeyCode) -> Vec<Effect> {
    app.key(ctrl_a(), now);
    app.key(press(code), now)
}

fn id(worktree: &str, kind: Kind) -> ProgramId {
    ProgramId {
        worktree: PathBuf::from(worktree),
        kind,
    }
}

#[test]
fn the_nvim_tab_starts_the_worktrees_neovim_once_and_keys_go_to_it() {
    let (mut app, now) = app();
    let nvim = id("/p/api.wt/fix", Kind::Nvim);
    let size = Size { rows: 31, cols: 87 };
    assert_eq!(
        prefixed(&mut app, now, KeyCode::Char('2')),
        [Effect::Run(nvim.clone(), size)]
    );
    assert_eq!(app.view.tab, Tab::Nvim);
    assert_eq!(
        app.key(press(KeyCode::Char('x')), now),
        [Effect::TypeInto(nvim.clone(), b"x".to_vec())]
    );
    assert_eq!(
        prefixed(&mut app, now, KeyCode::Char('a')),
        [Effect::TypeInto(nvim.clone(), vec![0x01])]
    );
    assert_eq!(
        app.paste("a\nb"),
        [Effect::TypeInto(nvim.clone(), b"a\rb".to_vec())]
    );
    assert_eq!(prefixed(&mut app, now, KeyCode::Char('1')), []);
    assert_eq!(
        app.key(press(KeyCode::Char('x')), now),
        [Effect::Type(b"x".to_vec())],
        "back to the agent"
    );
    assert_eq!(
        prefixed(&mut app, now, KeyCode::Char('2')),
        [],
        "kept running"
    );
}

#[test]
fn the_shell_tab_starts_the_shell_and_another_worktree_its_own() {
    let (mut app, now) = app();
    let effects = prefixed(&mut app, now, KeyCode::Char('3'));
    assert!(
        matches!(&effects[..], [Effect::Run(shell, _)] if *shell == id("/p/api.wt/fix", Kind::Shell)),
        "{effects:?}"
    );
    let effects = prefixed(&mut app, now, KeyCode::Char('j'));
    assert!(
        matches!(&effects[..], [Effect::Run(shell, _)] if *shell == id("/p/api.wt/docs", Kind::Shell)),
        "{effects:?}"
    );
    assert_eq!(app.programs.len(), 2, "the first worktree's shell is kept");
}

#[test]
fn the_split_shows_the_agent_beside_neovim_and_h_l_move_between_them() {
    let (mut app, now) = app();
    let nvim = id("/p/api.wt/fix", Kind::Nvim);
    let effects = prefixed(&mut app, now, KeyCode::Char('|'));
    assert!(
        matches!(&effects[..], [Effect::Run(started, _)] if *started == nvim),
        "{effects:?}"
    );
    assert_eq!(app.layout.cards.len(), 2);
    assert_eq!(
        app.key(press(KeyCode::Char('x')), now),
        [Effect::Type(b"x".to_vec())],
        "the focus stays on the agent"
    );
    prefixed(&mut app, now, KeyCode::Char('l'));
    assert_eq!(
        app.key(press(KeyCode::Char('x')), now),
        [Effect::TypeInto(nvim, b"x".to_vec())]
    );
    prefixed(&mut app, now, KeyCode::Char('h'));
    assert_eq!((app.focus, app.view.tab), (Focus::Centre, Tab::Agent));
    prefixed(&mut app, now, KeyCode::Char('h'));
    assert_eq!(app.focus, Focus::SideMenu);
    prefixed(&mut app, now, KeyCode::Char('l'));
    assert_eq!((app.focus, app.view.tab), (Focus::Centre, Tab::Agent));
    prefixed(&mut app, now, KeyCode::Char('|'));
    assert_eq!(app.layout.cards.len(), 1, "the split off");
}

#[test]
fn the_zoom_fills_the_screen_with_the_focused_panel_and_restores_the_split() {
    let (mut app, now) = app();
    prefixed(&mut app, now, KeyCode::Char('|'));
    prefixed(&mut app, now, KeyCode::Char('l'));
    prefixed(&mut app, now, KeyCode::Char('z'));
    assert_eq!(app.layout.side_menu, None);
    assert_eq!(app.layout.cards.len(), 1);
    assert_eq!(app.layout.cards[0].tab, Tab::Nvim);
    let effects = app.fit(SCREEN);
    assert!(
        matches!(
            &effects[..],
            [Effect::Resize(
                _,
                Size {
                    rows: 31,
                    cols: 116
                }
            )]
        ),
        "Neovim takes the whole width: {effects:?}"
    );
    prefixed(&mut app, now, KeyCode::Char('h'));
    assert_eq!(app.layout.cards[0].tab, Tab::Agent, "h switches panel");
    assert!(app.view.zoom, "and stays zoomed");
    assert_eq!(app.focus, Focus::Centre);
    prefixed(&mut app, now, KeyCode::Char('z'));
    assert_eq!(app.layout.cards.len(), 2, "the split is back");
    assert!(app.layout.side_menu.is_some());
    assert_eq!(app.view.tab, Tab::Agent, "with the focus kept");
}

#[test]
fn the_zoom_takes_the_focus_from_the_side_menu() {
    let (mut app, now) = app();
    prefixed(&mut app, now, KeyCode::Char('h'));
    assert_eq!(app.focus, Focus::SideMenu);
    prefixed(&mut app, now, KeyCode::Char('z'));
    assert_eq!(app.focus, Focus::Centre);
    prefixed(&mut app, now, KeyCode::Char('h'));
    assert_eq!(app.focus, Focus::Centre, "no side menu while zoomed");
}

#[test]
fn a_program_answers_its_terminals_questions_and_starts_again_once_ended() {
    let (mut app, now) = app();
    let nvim = id("/p/api.wt/fix", Kind::Nvim);
    prefixed(&mut app, now, KeyCode::Char('2'));
    assert_eq!(
        app.printed(&nvim, Printed::Output(b"\x1b[c".to_vec())),
        [Effect::TypeInto(nvim.clone(), b"\x1b[?62;22c".to_vec())]
    );
    app.printed(&nvim, Printed::Ended("exit 0".to_owned()));
    assert_eq!(app.key(press(KeyCode::Char('x')), now), [], "nothing runs");
    let effects = app.key(press(KeyCode::Enter), now);
    assert!(
        matches!(&effects[..], [Effect::Run(started, _)] if *started == nvim),
        "{effects:?}"
    );
    app.run_failed(&nvim, "nvim: not found".to_owned());
    assert_eq!(
        app.programs.get(&nvim).map(|program| &program.status),
        Some(&ProgramStatus::Failed("nvim: not found".to_owned()))
    );
    assert_eq!(app.key(press(KeyCode::Enter), now).len(), 1, "tries again");
}

/// `fix`'s changes: three files.
fn three_files() -> crate::domain::changes::Changes {
    use crate::domain::changes::{Change, ChangedFile, Changes};
    let file = |path: &str| ChangedFile {
        path: path.to_owned(),
        change: Change::Modified,
        lines: None,
    };
    Changes {
        files: vec![file("a.rs"), file("b.rs"), file("c.rs")],
        drift: None,
    }
}

#[test]
fn from_150_columns_l_reaches_the_right_column_and_enter_opens_the_diff_in_neovim() {
    let (mut app, now) = app();
    app.fit(Rect::new(0, 0, 180, 48));
    app.changes_read(PathBuf::from("/p/api.wt/fix"), Ok(three_files()));
    assert!(app.layout.right.is_some_and(|right| !right.overlay));

    prefixed(&mut app, now, KeyCode::Char('l'));
    assert_eq!(app.focus, Focus::RightColumn);
    assert_eq!(
        app.key(press(KeyCode::Char('j')), now),
        [],
        "j moves the cursor"
    );
    app.key(press(KeyCode::Char('j')), now);
    app.key(press(KeyCode::Char('j')), now);
    assert_eq!(app.file_cursor, 2, "and stops at the last file");
    app.key(press(KeyCode::Char('k')), now);

    let effects = app.key(press(KeyCode::Enter), now);
    assert!(
        matches!(&effects[0], Effect::Run(id, _) if id.kind == Kind::Nvim),
        "Neovim starts first, so the diff can wait for it: {effects:?}"
    );
    assert_eq!(
        effects[1..],
        [Effect::OpenDiff {
            primary: PathBuf::from("/p/api"),
            worktree: PathBuf::from("/p/api.wt/fix"),
            file: "b.rs".to_owned(),
        }]
    );
    assert_eq!((app.focus, app.view.tab), (Focus::Centre, Tab::Nvim));
    assert!(!app.view.zoom, "a column: no zoom");

    prefixed(&mut app, now, KeyCode::Char('l'));
    assert_eq!(app.focus, Focus::RightColumn);
    prefixed(&mut app, now, KeyCode::Char('h'));
    assert_eq!(app.focus, Focus::Centre, "^a h goes back to the centre");
}

#[test]
fn f_hides_and_shows_the_column_from_150_columns() {
    let (mut app, now) = app();
    app.fit(Rect::new(0, 0, 180, 48));
    prefixed(&mut app, now, KeyCode::Char('l'));
    prefixed(&mut app, now, KeyCode::Char('f'));
    assert_eq!(app.layout.right, None);
    assert_eq!(
        app.focus,
        Focus::Centre,
        "the focus leaves the hidden column"
    );
    prefixed(&mut app, now, KeyCode::Char('f'));
    assert!(app.layout.right.is_some());
    assert_eq!(app.focus, Focus::Centre);
}

#[test]
fn under_150_columns_f_lays_the_right_column_over_the_centre_and_enter_zooms_neovim() {
    let (mut app, now) = app();
    app.changes_read(PathBuf::from("/p/api.wt/fix"), Ok(three_files()));
    assert_eq!(app.layout.right, None, "hidden under 150 columns");
    prefixed(&mut app, now, KeyCode::Char('l'));
    assert_eq!(app.focus, Focus::Centre, "nothing to reach");

    prefixed(&mut app, now, KeyCode::Char('f'));
    assert!(app.layout.right.is_some_and(|right| right.overlay));
    assert_eq!(app.focus, Focus::RightColumn, "the overlay takes the focus");
    app.key(press(KeyCode::Esc), now);
    assert_eq!(app.layout.right, None, "esc closes it");
    assert_eq!(app.focus, Focus::Centre);

    prefixed(&mut app, now, KeyCode::Char('f'));
    let effects = app.key(press(KeyCode::Enter), now);
    assert!(matches!(effects.last(), Some(Effect::OpenDiff { file, .. }) if file == "a.rs"));
    assert_eq!(app.layout.right, None, "the overlay closes");
    assert!(app.view.zoom, "Neovim is zoomed");
    assert_eq!((app.focus, app.view.tab), (Focus::Centre, Tab::Nvim));
}

#[test]
fn enter_opens_nothing_before_the_changes_are_read_or_with_none() {
    let (mut app, now) = app();
    app.fit(Rect::new(0, 0, 180, 48));
    prefixed(&mut app, now, KeyCode::Char('l'));
    assert_eq!(app.key(press(KeyCode::Enter), now), []);
    app.changes_read(PathBuf::from("/p/api.wt/docs"), Ok(three_files()));
    assert_eq!(app.open_changes(), None, "another worktree's changes");
    assert_eq!(app.key(press(KeyCode::Enter), now), []);
    app.changes_read(PathBuf::from("/p/api.wt/fix"), Err("git failed".to_owned()));
    assert_eq!(app.changes_error(), Some("git failed"));
    assert_eq!(app.key(press(KeyCode::Enter), now), []);
}
