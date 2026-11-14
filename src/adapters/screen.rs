//! Adapter: the terminal the application draws on (#174), through ratatui and crossterm. Entering
//! it switches the terminal to raw mode, the alternate screen and bracketed paste; leaving it puts
//! everything back, and so does a panic, before its message is printed. SIGTERM, SIGHUP and
//! SIGINT are handed to the application, which quits and leaves the screen the same way.
//!
//! This adapter writes to stdout: not a report, the application's own screen.

use std::io::{self, Stdout, Write as _};
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use anyhow::{Context, Result};
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};
use ratatui::crossterm::{cursor, execute};
use ratatui::layout::Rect;
use ratatui::{Frame, Terminal};
use signal_hook::consts::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;

/// The application's terminal, from entering it until it is dropped.
pub(crate) struct Screen {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    /// PROTOTYPE (#203): what `supports_keyboard_enhancement` answered on entering (`None`: an
    /// error or no answer in time).
    pub(crate) probe: Option<bool>,
}

impl std::fmt::Debug for Screen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Screen").finish_non_exhaustive()
    }
}

impl Screen {
    /// Takes over the terminal: raw mode, the alternate screen, bracketed paste.
    pub(crate) fn enter() -> Result<Self> {
        restore_on_panic();
        enable_raw_mode().context("could not put the terminal in raw mode")?;
        TAKEN.store(true, Ordering::SeqCst);
        let entered = execute!(io::stdout(), EnterAlternateScreen, EnableBracketedPaste);
        if let Err(err) = entered {
            restore();
            return Err(err).context("could not switch to the alternate screen");
        }
        // PROTOTYPE (#203): does the terminal speak the kitty keyboard protocol? Asked before the
        // reader thread starts, since the answer comes on stdin.
        let probe = supports_keyboard_enhancement().ok();
        let terminal = Terminal::new(CrosstermBackend::new(io::stdout()));
        match terminal {
            Ok(terminal) => Ok(Self { terminal, probe }),
            Err(err) => {
                restore();
                Err(err).context("could not draw on the terminal")
            }
        }
    }

    /// The terminal's size, in cells.
    pub(crate) fn area(&self) -> Result<Rect> {
        let size = self
            .terminal
            .size()
            .context("could not read the terminal's size")?;
        Ok(Rect::new(0, 0, size.width, size.height))
    }

    /// Draws one frame: `render` paints it, and only the cells that changed are written.
    pub(crate) fn draw(&mut self, render: impl FnOnce(&mut Frame<'_>)) -> Result<()> {
        self.terminal
            .draw(render)
            .context("could not draw on the terminal")?;
        Ok(())
    }
}

impl Screen {
    /// PROTOTYPE (#203): pushes or pops the kitty keyboard protocol (flag 1, disambiguate) and
    /// captures the mouse or not, as `kkp` and `mouse` say. Returns what changed, for the log.
    pub(crate) fn set_modes(&mut self, kkp: bool, mouse: bool) -> Vec<&'static str> {
        let mut changed = Vec::new();
        let mut stdout = io::stdout();
        if kkp != PUSHED.load(Ordering::SeqCst) {
            let done = if kkp {
                execute!(
                    stdout,
                    PushKeyboardEnhancementFlags(
                        KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    )
                )
            } else {
                execute!(stdout, PopKeyboardEnhancementFlags)
            };
            if done.is_ok() {
                PUSHED.store(kkp, Ordering::SeqCst);
                changed.push(if kkp {
                    "pushed kitty keyboard flags 1"
                } else {
                    "popped kitty keyboard flags"
                });
            }
        }
        if mouse != MOUSE.load(Ordering::SeqCst) {
            let done = if mouse {
                execute!(stdout, EnableMouseCapture)
            } else {
                execute!(stdout, DisableMouseCapture)
            };
            if done.is_ok() {
                MOUSE.store(mouse, Ordering::SeqCst);
                changed.push(if mouse {
                    "mouse capture on"
                } else {
                    "mouse capture off"
                });
            }
        }
        changed
    }
}

/// PROTOTYPE (#203): whether the kitty keyboard flags are pushed, and the mouse captured.
static PUSHED: AtomicBool = AtomicBool::new(false);
static MOUSE: AtomicBool = AtomicBool::new(false);

impl Drop for Screen {
    fn drop(&mut self) {
        restore();
    }
}

/// Whether the application holds the terminal, so it is restored exactly once (a panic restores
/// it, then unwinding drops the `Screen`).
static TAKEN: AtomicBool = AtomicBool::new(false);

/// Puts the terminal back as the shell left it: the main screen, the cursor shown, no bracketed
/// paste, cooked mode. Does nothing when it is back already.
fn restore() {
    if !TAKEN.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut stdout = io::stdout();
    if PUSHED.swap(false, Ordering::SeqCst) {
        let _ = execute!(stdout, PopKeyboardEnhancementFlags);
    }
    if MOUSE.swap(false, Ordering::SeqCst) {
        let _ = execute!(stdout, DisableMouseCapture);
    }
    let _ = execute!(
        stdout,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        cursor::Show
    );
    let _ = stdout.flush();
    let _ = disable_raw_mode();
}

/// Restores the terminal before a panic's message is printed, so the message is readable and the
/// shell usable. Installed once.
fn restore_on_panic() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            previous(info);
        }));
    });
}

/// Reads the person's keys, pastes and resizes on a thread of its own, handing each to `on_event`
/// until it returns `false` or the terminal cannot be read.
pub(crate) fn read_events(mut on_event: impl FnMut(Event) -> bool + Send + 'static) {
    thread::spawn(move || {
        while let Ok(event) = event::read() {
            if !on_event(event) {
                return;
            }
        }
    });
}

/// Calls `on_signal` on a thread of its own whenever the process is asked to end: SIGTERM, SIGHUP
/// (the terminal closed) or SIGINT.
pub(crate) fn on_quit_signal(mut on_signal: impl FnMut() + Send + 'static) -> Result<()> {
    let mut signals =
        Signals::new([SIGTERM, SIGHUP, SIGINT]).context("could not watch for signals")?;
    thread::spawn(move || {
        for _ in signals.forever() {
            on_signal();
        }
    });
    Ok(())
}
