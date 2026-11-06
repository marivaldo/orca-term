// THROWAWAY PROTOTYPE (issue #170): orca-term as a terminal application, drawn to the "Quiet v2"
// design (`design/README.md`, `design/screenshots/`).
//
// The fleet in the side menu is fake; the terminals in the panes are real (a PTY each). Nothing
// here is meant to be kept: no tests, no error handling beyond what keeps it going.
//
//   just prototype                          the prototype, live
//   just prototype --snapshot main          one frame as plain text, no child processes
//   just prototype --snapshot main --ansi   the same frame with its colours (ANSI escapes)
//
// Screens: main (4a), split, prefix (4b), small (4c, 120x36), zoom (4d), failed (4e), nocolor
// (4f; 4g with --width 120 --height 36). --width and --height override the size.
//
// Environment: ORCA_PROTO_AGENT_CMD and ORCA_PROTO_EDITOR_CMD replace `claude` and `nvim`;
// ORCA_PROTO_PREFIX (one letter) replaces Ctrl-a; ORCA_PROTO_MOUSE=0 leaves the mouse to the
// terminal the prototype is in. NO_COLOR and COLORTERM choose the colours.

mod app;
mod draw;
mod fleet;
mod keys;
mod mock;
mod paint;
mod pty;
#[rustfmt::skip]
mod theme;

use std::fmt::Write as _;
use std::io::{Write, stdout};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};

use app::{AGENT, App, Focus, Msg, NVIM};
use paint::Mode;
use pty::Wake;

const USAGE: &str = "usage: orca-term-prototype [--snapshot \
main|split|prefix|small|zoom|failed|nocolor [--width N] [--height N] [--ansi]]";

/// How often the working glyph turns.
const TICK: Duration = Duration::from_millis(120);
/// How long the loop waits when nothing on screen moves by itself.
const IDLE: Duration = Duration::from_millis(500);
/// The least time between two frames, so a terminal printing fast does not redraw for each read.
const FRAME: Duration = Duration::from_millis(8);

const SCREENS: [&str; 7] = [
    "main", "split", "prefix", "small", "zoom", "failed", "nocolor",
];

struct Snapshot {
    screen: String,
    width: Option<u16>,
    height: Option<u16>,
    ansi: bool,
}

/// The arguments: none for the live prototype, or `--snapshot` and what it draws.
fn parse(args: &[String]) -> Result<Option<Snapshot>, String> {
    let mut snapshot: Option<Snapshot> = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        if arg == "--snapshot" {
            let screen = value()?.to_ascii_lowercase();
            if !SCREENS.contains(&screen.as_str()) {
                return Err(format!("unknown screen `{screen}`"));
            }
            snapshot = Some(Snapshot {
                screen,
                width: None,
                height: None,
                ansi: false,
            });
            continue;
        }
        let Some(snapshot) = snapshot.as_mut() else {
            return Err(format!("unknown argument `{arg}`"));
        };
        match arg.as_str() {
            "--width" => snapshot.width = Some(value()?.parse().map_err(|_| "bad --width")?),
            "--height" => snapshot.height = Some(value()?.parse().map_err(|_| "bad --height")?),
            "--ansi" => snapshot.ansi = true,
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(snapshot)
}

/// The state of each named screen, as its screenshot shows it.
fn stage(app: &mut App, screen: &str, width: u16) {
    let split_on_nvim = |app: &mut App| {
        app.split = true;
        app.show_terminal(AGENT);
        app.show_terminal(NVIM);
        app.opened.insert(app.open, "src/auth/login.rs".to_owned());
    };
    match screen {
        "split" => split_on_nvim(app),
        "prefix" => {
            split_on_nvim(app);
            app.pending = Some(Instant::now());
        }
        "small" => app.focus_on(Focus::Sidebar),
        "zoom" => {
            split_on_nvim(app);
            app.zoom = true;
        }
        "failed" => {
            app.open_worktree((1, 1));
            // fix-login was never opened in 4e: its unseen dot shows.
            app.fleet[0].worktrees[1].unseen = true;
        }
        "nocolor" if width < 150 => app.focus_on(Focus::Sidebar),
        "nocolor" => {
            split_on_nvim(app);
            app.pending = Some(Instant::now());
        }
        _ => {}
    }
}

/// One frame on ratatui's test backend, printed as plain text (or with its colours). No process
/// is started; the terminal panes show placeholders.
fn snapshot(options: &Snapshot) {
    let small = options.screen == "small";
    let width = options.width.unwrap_or(if small { 120 } else { 180 });
    let height = options.height.unwrap_or(if small { 36 } else { 48 });
    let mode = if options.screen == "nocolor" {
        Mode::Mono
    } else {
        Mode::detect()
    };
    let mut app = App::new(None);
    app.width = width;
    stage(&mut app, &options.screen, width);

    let mut geometry = draw::layout(&app, Rect::new(0, 0, width, height));
    let backend = TestBackend::new(width, height);
    // The test backend cannot fail.
    let Ok(mut terminal) = Terminal::new(backend);
    let _ = terminal.draw(|frame| draw::draw(frame, &app, &mut geometry, mode));
    let buffer = terminal.backend().buffer();
    let mut out = if options.ansi {
        ansi(buffer)
    } else {
        let mut out = String::new();
        for y in 0..height {
            let mut line = String::new();
            for x in 0..width {
                line.push_str(buffer[(x, y)].symbol());
            }
            out.push_str(line.trim_end());
            out.push('\n');
        }
        out
    };
    let _ = writeln!(
        out,
        "# screen {}, {width}x{height}, {mode:?}",
        options.screen
    );
    let _ = stdout().write_all(out.as_bytes());
}

/// The frame with SGR escapes, cell by cell, to check where the colours land.
fn ansi(buffer: &Buffer) -> String {
    let colour = |c: Color, base: u8| -> String {
        match c {
            Color::Rgb(r, g, b) => format!("{};2;{r};{g};{b}", base + 8),
            Color::Indexed(i) => format!("{};5;{i}", base + 8),
            _ => format!("{}", base + 9),
        }
    };
    let mut out = String::new();
    let area = buffer.area;
    for y in 0..area.height {
        for x in 0..area.width {
            let cell = &buffer[(x, y)];
            let mut sgr = vec!["0".to_owned(), colour(cell.fg, 30), colour(cell.bg, 40)];
            for (m, code) in [
                (Modifier::BOLD, "1"),
                (Modifier::DIM, "2"),
                (Modifier::ITALIC, "3"),
                (Modifier::UNDERLINED, "4"),
                (Modifier::REVERSED, "7"),
            ] {
                if cell.modifier.contains(m) {
                    sgr.push(code.to_owned());
                }
            }
            let _ = write!(out, "\x1b[{}m{}", sgr.join(";"), cell.symbol());
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

/// The child's cursor shape (DECSCUSR), for the terminal the prototype is in.
fn cursor_style(shape: u16) -> SetCursorStyle {
    match shape {
        1 => SetCursorStyle::BlinkingBlock,
        2 => SetCursorStyle::SteadyBlock,
        3 => SetCursorStyle::BlinkingUnderScore,
        4 => SetCursorStyle::SteadyUnderScore,
        5 => SetCursorStyle::BlinkingBar,
        6 => SetCursorStyle::SteadyBar,
        _ => SetCursorStyle::DefaultUserShape,
    }
}

fn live() -> std::io::Result<()> {
    let mouse = std::env::var("ORCA_PROTO_MOUSE").map_or(true, |value| value != "0");
    let mode = Mode::detect();
    let (tx, rx) = mpsc::channel();
    let wake = Wake {
        tx: tx.clone(),
        pending: Arc::new(AtomicBool::new(false)),
    };

    // A panic gives the terminal back first: ratatui's hook leaves the alternate screen and raw
    // mode, then calls this one.
    let earlier = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = execute!(
            stdout(),
            DisableMouseCapture,
            DisableBracketedPaste,
            SetCursorStyle::DefaultUserShape
        );
        earlier(info);
    }));
    let mut terminal = ratatui::try_init()?;
    execute!(stdout(), EnableBracketedPaste)?;
    if mouse {
        execute!(stdout(), EnableMouseCapture)?;
    }

    // The keyboard and the mouse, read on their own so the loop waits on one channel.
    std::thread::spawn(move || {
        while let Ok(event) = event::read() {
            if tx.send(Msg::Input(event)).is_err() {
                break;
            }
        }
    });

    let size = terminal.size()?;
    let mut app = App::new(Some(wake.clone()));
    app.width = size.width;
    let started = Instant::now();
    let mut last_frame = Instant::now() - FRAME;
    let mut shape = 0;
    let result = loop {
        if let Some(wait) = FRAME.checked_sub(last_frame.elapsed()) {
            std::thread::sleep(wait);
        }
        last_frame = Instant::now();
        wake.pending.store(false, Ordering::SeqCst);
        app.tick = (started.elapsed().as_millis() / TICK.as_millis()) as usize;
        app.expire();

        let size = match terminal.size() {
            Ok(size) => size,
            Err(error) => break Err(error),
        };
        app.width = size.width;
        let mut geometry = draw::layout(&app, Rect::new(0, 0, size.width, size.height));
        app.fit_terminals(&geometry);
        if let Err(error) = terminal.draw(|frame| draw::draw(frame, &app, &mut geometry, mode)) {
            break Err(error);
        }
        app.geometry = Some(geometry);

        // The cursor's shape follows the child with the focus (nvim's bar in insert mode).
        let wanted = match app.focus {
            Focus::Centre => app
                .term(app.current())
                .map_or(0, |term| term.parser().callbacks().cursor_shape),
            _ => 0,
        };
        if wanted != shape {
            shape = wanted;
            let _ = execute!(stdout(), cursor_style(shape));
        }

        let wait = if app.animated() { TICK } else { IDLE };
        match rx.recv_timeout(wait) {
            Ok(first) => {
                // Everything that is waiting, before the next frame.
                for msg in std::iter::once(first).chain(rx.try_iter()) {
                    if let Msg::Input(event) = msg {
                        app.on_event(event);
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break Ok(()),
        }
        if app.quit {
            break Ok(());
        }
    };

    app.kill_all();
    let _ = execute!(
        stdout(),
        DisableMouseCapture,
        DisableBracketedPaste,
        SetCursorStyle::DefaultUserShape
    );
    ratatui::restore();
    result
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse(&args) {
        Ok(Some(options)) => snapshot(&options),
        Ok(None) => {
            if let Err(error) = live() {
                eprintln!("orca-term-prototype: {error}");
                std::process::exit(1);
            }
        }
        Err(problem) => {
            eprintln!("orca-term-prototype: {problem}\n{USAGE}");
            std::process::exit(2);
        }
    }
}
