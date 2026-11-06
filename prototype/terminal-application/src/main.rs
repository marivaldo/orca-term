// THROWAWAY PROTOTYPE (issue #170): what should orca-term look like as a terminal application? Three layouts over the same real terminals, switched with the prefix then `v`: A "Orca", B "Panes", C "Focus".
//
// The fleet in the side menu is fake; the terminals in the panels are real (a PTY each). Nothing
// here is meant to be kept: no tests, no error handling beyond what keeps it going.
//
//   just prototype                      the prototype, live
//   just prototype --snapshot A        one frame as plain text, no child processes
//
// Environment: ORCA_PROTO_AGENT_CMD and ORCA_PROTO_EDITOR_CMD replace `claude` and `nvim`;
// ORCA_PROTO_PREFIX (one letter) replaces Ctrl-a; ORCA_PROTO_MOUSE=0 leaves the mouse to the
// terminal the prototype is in.

mod app;
mod draw;
mod fleet;
mod keys;
mod menu;
mod pty;

use std::io::{Write, stdout};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::cursor::SetCursorStyle;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
};
use ratatui::crossterm::execute;
use ratatui::layout::Rect;

use app::{App, Focus, Msg, Variant};
use pty::Wake;

const USAGE: &str = "usage: orca-term-prototype [--snapshot <A|B|C> [--width N] [--height N] \
[--focus menu|centre|right] [--zoom] [--split] [--overlay menu|right]]";

/// How often the working glyph turns, and so how long the loop waits with nothing to do.
const TICK: Duration = Duration::from_millis(150);
/// The least time between two frames, so a terminal printing fast does not redraw for each read.
const FRAME: Duration = Duration::from_millis(8);

struct Snapshot {
    variant: Variant,
    width: u16,
    height: u16,
    focus: Option<Focus>,
    zoom: bool,
    split: bool,
    overlay: Option<Focus>,
}

fn column(word: &str) -> Result<Focus, String> {
    match word {
        "menu" => Ok(Focus::Menu),
        "centre" | "center" => Ok(Focus::Centre),
        "right" => Ok(Focus::Right),
        other => Err(format!("unknown column `{other}`")),
    }
}

/// The arguments: none for the live prototype, or `--snapshot` and what it draws.
fn parse(args: &[String]) -> Result<Option<Snapshot>, String> {
    let mut snapshot: Option<Snapshot> = None;
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let mut value = || args.next().ok_or(format!("{arg} needs a value"));
        if arg == "--snapshot" {
            let variant = match value()?.to_ascii_uppercase().as_str() {
                "A" => Variant::A,
                "B" => Variant::B,
                "C" => Variant::C,
                other => return Err(format!("unknown variant `{other}`")),
            };
            snapshot = Some(Snapshot {
                variant,
                width: 150,
                height: 42,
                focus: None,
                zoom: false,
                split: false,
                overlay: None,
            });
            continue;
        }
        let Some(snapshot) = snapshot.as_mut() else {
            return Err(format!("unknown argument `{arg}`"));
        };
        match arg.as_str() {
            "--width" => snapshot.width = value()?.parse().map_err(|_| "bad --width")?,
            "--height" => snapshot.height = value()?.parse().map_err(|_| "bad --height")?,
            "--focus" => snapshot.focus = Some(column(value()?)?),
            "--overlay" => snapshot.overlay = Some(column(value()?)?),
            "--zoom" => snapshot.zoom = true,
            "--split" => snapshot.split = true,
            other => return Err(format!("unknown argument `{other}`")),
        }
    }
    Ok(snapshot)
}

/// One frame on ratatui's test backend, printed as plain text. No process is started.
fn snapshot(options: &Snapshot) {
    let mut app = App::new(None);
    app.set_variant(options.variant);
    app.split = options.split;
    if let Some(column) = options.overlay {
        if options.variant == Variant::A {
            app.focus_on(column);
        } else {
            app.overlay = Some(column);
            app.focus = column;
        }
    }
    if let Some(column) = options.focus {
        app.focus_on(column);
    }
    app.zoom = options.zoom;

    let area = Rect::new(0, 0, options.width, options.height);
    let geometry = draw::layout(&app, area);
    let backend = TestBackend::new(options.width, options.height);
    // The test backend cannot fail.
    let Ok(mut terminal) = Terminal::new(backend);
    let _ = terminal.draw(|frame| draw::draw(frame, &app, &geometry));
    let buffer = terminal.backend().buffer();
    let mut out = String::new();
    for y in 0..options.height {
        let mut line = String::new();
        for x in 0..options.width {
            line.push_str(buffer[(x, y)].symbol());
        }
        out.push_str(line.trim_end());
        out.push('\n');
    }
    out.push_str(&format!(
        "# variant {}, focus {}, {}x{}{}{}\n",
        app.variant.label(),
        app.focus.word(),
        options.width,
        options.height,
        if app.zoom { ", zoom" } else { "" },
        if app.split { ", split" } else { "" },
    ));
    let _ = stdout().write_all(out.as_bytes());
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

    let mut app = App::new(Some(wake.clone()));
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

        let size = match terminal.size() {
            Ok(size) => size,
            Err(error) => break Err(error),
        };
        let geometry = draw::layout(&app, Rect::new(0, 0, size.width, size.height));
        app.fit_terminals(&geometry);
        if let Err(error) = terminal.draw(|frame| draw::draw(frame, &app, &geometry)) {
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

        match rx.recv_timeout(TICK) {
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
