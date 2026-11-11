//! App: a failed agent (#178). When the open worktree's agent ended with an error that nobody asked
//! for (its holder recorded it, `AgentEnd`), the centre shows its last output and, under it, a card
//! saying so, with its keys: `r` resumes the session (`agent start`), `R` restarts clean on a new
//! session (`agent start --fresh`), `l` shows the log, `x` dismisses the card.
//!
//! The log is the agent's last output as its holder kept it (`agent.log`), read once when the card
//! first shows and fed to a screen that keeps what scrolled off its top. `l` shows it in the
//! agent's panel, scrolled with `j`/`k`, `ctrl-u`/`ctrl-d` (or the page keys), `g`/`G`; `esc`, `q`
//! or `l` close it. A scrollable view rather than the worktree's Neovim: the log is a terminal's
//! output, escape codes and all, which reads as the agent drew it only through a terminal emulator,
//! and the application has one at hand.

use std::path::{Path, PathBuf};
use std::time::Instant;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::{App, Effect};
use crate::app::emulator::Emulator;
use crate::app::menu;
use crate::app::panel::Status;
use crate::app::view::Tab;
use crate::domain::agent::{AgentEnd, StartMode};

/// How many rows of the log are kept beyond its last screen.
const LOG_LINES: usize = 10_000;

/// A failed agent's last output.
#[derive(Debug)]
pub(crate) struct Log {
    /// The worktree whose agent it is, and when that agent ended.
    pub(crate) worktree: PathBuf,
    pub(crate) ended_at: u64,
    /// The output on a screen, once read.
    pub(crate) screen: Option<Emulator>,
    /// Whether the log fills the agent's panel (`l`), and how many rows back it is scrolled.
    pub(crate) shown: bool,
    pub(crate) up: usize,
}

impl App {
    /// How the open worktree's agent failed, while its card shows: no agent runs or starts there,
    /// it failed, and the card was not dismissed.
    pub(crate) fn failure(&self) -> Option<&AgentEnd> {
        let open = self.open.as_ref()?;
        self.panel
            .as_ref()
            .filter(|panel| matches!(panel.status, Status::NoAgent | Status::Ended(_)))?;
        let (_, worktree) = menu::find(&self.fleets, open)?;
        let end = worktree.agent_end.as_ref().filter(|end| end.failed)?;
        let dismissed = self.dismissed.contains(&(open.clone(), end.ended_at));
        (!dismissed).then_some(end)
    }

    /// The log, while it fills the open worktree's agent panel.
    pub(crate) fn log_shown(&self) -> Option<&Log> {
        self.failure()?;
        self.log
            .as_ref()
            .filter(|log| log.shown && self.open.as_ref() == Some(&log.worktree))
    }

    /// Asks for the failed agent's log once its card shows, unless it was asked for already.
    pub(super) fn want_log(&mut self) -> Vec<Effect> {
        let (Some(open), Some(ended_at)) = (self.open.clone(), self.failure().map(|e| e.ended_at))
        else {
            return Vec::new();
        };
        let asked = self
            .log
            .as_ref()
            .is_some_and(|log| log.worktree == open && log.ended_at == ended_at);
        if asked {
            return Vec::new();
        }
        self.log = Some(Log {
            worktree: open.clone(),
            ended_at,
            screen: None,
            shown: false,
            up: 0,
        });
        vec![Effect::ReadLog(open)]
    }

    /// The log of the agent of the worktree at `path`, read (`Err` says why it could not be).
    pub(crate) fn log_read(&mut self, path: &Path, read: Result<Vec<u8>, String>, now: Instant) {
        let body = self.layout.body_of(Tab::Agent);
        let asked = self.log.as_ref().is_some_and(|log| log.worktree == path);
        if !asked {
            return;
        }
        let mut screen = Emulator::with_scrollback(body, LOG_LINES);
        match read {
            Ok(bytes) => {
                screen.feed(&bytes);
            }
            Err(why) => self.say(format!("could not read the agent's log: {why}"), now),
        }
        if let Some(log) = self.log.as_mut() {
            log.screen = Some(screen);
        }
    }

    /// A key for the failure card.
    pub(super) fn failure_key(&mut self, key: KeyEvent) -> Vec<Effect> {
        match key.code {
            KeyCode::Char('r') | KeyCode::Enter => self.start_agent(StartMode::Resume),
            KeyCode::Char('R') => self.start_agent(StartMode::Fresh),
            KeyCode::Char('l') => {
                if let Some(log) = self.log.as_mut() {
                    log.shown = true;
                }
                Vec::new()
            }
            KeyCode::Char('x') => {
                if let (Some(open), Some(end)) = (self.open.clone(), self.failure()) {
                    let ended_at = end.ended_at;
                    self.dismissed.insert((open, ended_at));
                }
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    /// A key for the log shown: it scrolls, or closes.
    pub(super) fn log_key(&mut self, key: KeyEvent) {
        let page = usize::from(self.layout.body_of(Tab::Agent).height.max(2) - 1);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let Some(log) = self.log.as_mut() else {
            return;
        };
        let up = match key.code {
            KeyCode::Esc | KeyCode::Char('q' | 'l') => {
                log.shown = false;
                0
            }
            KeyCode::Char('k') | KeyCode::Up => log.up + 1,
            KeyCode::Char('j') | KeyCode::Down => log.up.saturating_sub(1),
            KeyCode::Char('u') if ctrl => log.up + page / 2,
            KeyCode::Char('d') if ctrl => log.up.saturating_sub(page / 2),
            KeyCode::PageUp => log.up + page,
            KeyCode::PageDown => log.up.saturating_sub(page),
            KeyCode::Char('g') | KeyCode::Home => usize::MAX,
            KeyCode::Char('G') | KeyCode::End => 0,
            _ => log.up,
        };
        log.up = log.screen.as_mut().map_or(0, |screen| screen.scroll(up));
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Rect;

    use super::*;
    use crate::app::fixture;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    const DOCS: &str = "/p/api.wt/docs";

    /// The application showing `docs`, whose agent ended as `end`.
    fn ended(end: AgentEnd) -> (App, Vec<Effect>, Instant) {
        let mut fleets = fixture::fleets();
        if let Ok(fleet) = fleets[0].fleet.as_mut() {
            fleet.worktrees[1].agent_end = Some(end);
        }
        let mut app = App::new(Rect::new(0, 0, 120, 36), Some(PathBuf::from(DOCS)));
        let effects = app.set_fleets(fleets);
        (app, effects, Instant::now())
    }

    fn failed() -> AgentEnd {
        AgentEnd::new(Some(1), None, false, 0, 41 * 60_000)
    }

    #[test]
    fn a_failed_agent_shows_its_card_and_its_log_is_read_once() {
        let (mut app, effects, now) = ended(failed());
        assert_eq!(effects, [Effect::ReadLog(PathBuf::from(DOCS))]);
        assert_eq!(app.failure().map(|end| end.exit.as_str()), Some("exit 1"));
        assert_eq!(app.key(key(KeyCode::Char('z')), now), [], "read once");
        let (app, effects, _) = ended(AgentEnd::new(Some(129), None, true, 0, 0));
        assert!(app.failure().is_none(), "stopped, not failed");
        assert!(effects.is_empty());
    }

    #[test]
    fn r_resumes_the_session_and_capital_r_starts_a_new_one() {
        let (mut app, _, now) = ended(failed());
        let effects = app.key(key(KeyCode::Char('r')), now);
        assert!(
            matches!(&effects[..], [Effect::Start(path, _, StartMode::Resume)] if path == Path::new(DOCS)),
            "{effects:?}"
        );
        assert!(app.failure().is_none(), "starting");
        app.started(Path::new(DOCS), Err("no claude".to_owned()), now);
        assert!(app.failure().is_some(), "back once the start failed");
        let effects = app.key(key(KeyCode::Char('R')), now);
        assert!(
            matches!(&effects[..], [Effect::Start(_, _, StartMode::Fresh)]),
            "{effects:?}"
        );
    }

    #[test]
    fn x_dismisses_the_card_and_enter_still_starts_the_agent() {
        let (mut app, _, now) = ended(failed());
        assert_eq!(app.key(key(KeyCode::Char('x')), now), []);
        assert!(app.failure().is_none());
        let effects = app.key(key(KeyCode::Enter), now);
        assert!(matches!(&effects[..], [Effect::Start(..)]), "{effects:?}");
    }

    #[test]
    fn l_shows_the_log_which_scrolls_and_closes() {
        let (mut app, _, now) = ended(failed());
        let lines = (1..=100).fold(String::new(), |all, n| all + &format!("line {n}\r\n"));
        app.log_read(Path::new(DOCS), Ok(lines.into_bytes()), now);
        app.key(key(KeyCode::Char('l')), now);
        assert!(app.log_shown().is_some());
        app.key(key(KeyCode::Char('k')), now);
        app.key(key(KeyCode::Char('k')), now);
        assert_eq!(app.log_shown().map(|log| log.up), Some(2));
        app.key(key(KeyCode::Char('j')), now);
        assert_eq!(app.log_shown().map(|log| log.up), Some(1));
        app.key(key(KeyCode::Char('g')), now);
        assert!(app.log_shown().is_some_and(|log| log.up > 60), "the top");
        assert_eq!(app.key(key(KeyCode::Char('r')), now), [], "scrolling");
        app.key(key(KeyCode::Esc), now);
        assert!(app.log_shown().is_none());
        assert!(app.failure().is_some(), "the card again");
    }
}
