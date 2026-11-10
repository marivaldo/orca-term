//! App: `orca-term app`, the terminal application (ADR 0012): the side menu with every project's
//! worktrees and, in the centre, the open worktree's agent terminal attached to its holder (#174).
//! It is a client of the core: it reads the fleets and starts agents through the operations, and
//! talks to each agent's holder as `agent attach` does. Quitting stops no agent.
//!
//! This file is the run loop. Everything that happens arrives as one message on one channel: a key
//! or a paste (from the terminal's reader), the fleets read again, what the open agent's holder
//! sent, an agent started, a signal to quit. Each message changes the state (`state.rs`), which
//! answers with effects the loop carries out, and the loop draws the next frame.
//!
//! Live updates come from reading every project's fleet again each second, on a thread of its own
//! (`ops::fleets::run_quick`): one listing of the worktrees by git per project, and their small
//! state files. The core runs nothing of its own to push changes, and the push it has, to the Neovims that
//! registered (`orca-term nvim register`), speaks Neovim's own protocol; a second kind of listener
//! would mean a socket and a registry for the application, where the read costs a few
//! milliseconds a second and the side menu needs a clock anyway, for the ages and the spinner.

pub(crate) mod draw;
#[cfg(test)]
pub(crate) mod fixture;
pub(crate) mod keys;
pub(crate) mod layout;
pub(crate) mod menu;
pub(crate) mod paint;
pub(crate) mod panel;
pub(crate) mod state;
pub(crate) mod theme;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use ratatui::crossterm::event::{Event, KeyEventKind};

use crate::adapters::holder::client::{Client, Heard};
use crate::adapters::screen::{self, Screen};
use crate::adapters::{env, git, terminal};
use crate::app::paint::Mode;
use crate::app::state::{App, Effect};
use crate::domain::agent::{Size, StartMode};
use crate::domain::project::ProjectFleet;
use crate::domain::worktree::Target;
use crate::ops::agent_start::{self, Wanted};
use crate::ops::{Approval, fleets};

/// How long the loop waits for a message before it draws again (the spinner, the prefix's
/// timeout, the ages).
const TICK: Duration = Duration::from_millis(100);

/// How often the fleets are read again.
const FLEETS_EVERY: Duration = Duration::from_secs(1);

/// Everything the loop is told.
#[derive(Debug)]
enum Msg {
    /// A key, a paste or a resize.
    Event(Event),
    /// Every project's fleet, read again, or why the project list could not be read.
    Fleets(Result<Vec<ProjectFleet>, String>),
    /// What the holder of connection number `.0` sent.
    Heard(u64, Heard),
    /// The core started the agent of the worktree at this path, or could not.
    Started(PathBuf, Result<(), String>),
    /// A signal asked the application to end.
    Quit,
}

/// Runs the application in this terminal until the person quits or a signal ends it. The
/// terminal is restored in every case.
pub(crate) fn run() -> Result<()> {
    if !terminal::stdin_and_stdout_are_terminals() {
        bail!("`orca-term app` draws on a terminal: run it in one");
    }
    let start_in = env::current_dir().ok();
    let (no_color, colorterm) = env::colour_env();
    let mode = Mode::detect(no_color, colorterm.as_deref());
    let (sender, messages) = mpsc::channel();
    let mut screen = Screen::enter()?;
    let mut app = App::new(screen.area()?, start_in);
    listen(&sender)?;
    let mut link = Link::default();
    loop {
        if let Some(size) = app.fit(screen.area()?) {
            link.send_size(size);
        }
        screen.draw(|frame| draw::frame(frame, &app, env::now_millis(), mode))?;
        let Some(batch) = next_batch(&messages) else {
            return Ok(());
        };
        let effects: Vec<Effect> = batch
            .into_iter()
            .flat_map(|message| handle(&mut app, &link, message))
            .collect();
        for effect in effects {
            if !apply(&mut app, &mut link, &sender, effect) {
                return Ok(());
            }
        }
        link.follow(&app);
        app.tick(Instant::now());
    }
}

/// Every message waiting, after waiting up to [`TICK`] for the first: a burst of the agent's output
/// is drawn once. `None` once nothing can send any more.
fn next_batch(messages: &Receiver<Msg>) -> Option<Vec<Msg>> {
    let first = match messages.recv_timeout(TICK) {
        Ok(message) => Some(message),
        Err(RecvTimeoutError::Timeout) => None,
        Err(RecvTimeoutError::Disconnected) => return None,
    };
    Some(first.into_iter().chain(messages.try_iter()).collect())
}

/// Starts what sends messages: the terminal's reader, the signals and the fleets' reader.
fn listen(sender: &Sender<Msg>) -> Result<()> {
    let events = sender.clone();
    screen::read_events(move |event| events.send(Msg::Event(event)).is_ok());
    let signals = sender.clone();
    screen::on_quit_signal(move || {
        let _ = signals.send(Msg::Quit);
    })?;
    let fleets = sender.clone();
    thread::spawn(move || {
        loop {
            let read = fleets::run_quick().map_err(|err| format!("{err:#}"));
            if fleets.send(Msg::Fleets(read)).is_err() {
                return;
            }
            thread::sleep(FLEETS_EVERY);
        }
    });
    Ok(())
}

/// What `message` does to the state, as effects to carry out.
fn handle(app: &mut App, link: &Link, message: Msg) -> Vec<Effect> {
    let now = Instant::now();
    match message {
        Msg::Event(Event::Key(key)) if key.kind != KeyEventKind::Release => app.key(key, now),
        Msg::Event(Event::Paste(text)) => app.paste(&text),
        Msg::Event(_) => Vec::new(),
        Msg::Fleets(Ok(fleets)) => app.set_fleets(fleets),
        Msg::Fleets(Err(why)) => {
            app.say(why, now);
            Vec::new()
        }
        Msg::Heard(connection, heard) => {
            if connection == link.number
                && let Some(panel) = app.panel.as_mut()
            {
                panel.heard(heard);
            }
            Vec::new()
        }
        Msg::Started(path, result) => app.started(&path, result, now),
        Msg::Quit => vec![Effect::Quit],
    }
}

/// Carries out `effect`. Returns `false` to quit.
fn apply(app: &mut App, link: &mut Link, sender: &Sender<Msg>, effect: Effect) -> bool {
    match effect {
        Effect::Type(bytes) => link.type_bytes(&bytes),
        Effect::Attach(path) => match link.attach(&path, sender) {
            Ok(()) => {
                app.attached(&path);
                if let Some(panel) = &app.panel {
                    let (rows, cols) = panel.size();
                    link.send_size(Size { rows, cols });
                }
            }
            Err(err) => app.attach_failed(&path, format!("{err:#}"), Instant::now()),
        },
        Effect::Start(path, size) => start_agent(path, size, sender.clone()),
        Effect::Quit => return false,
    }
    true
}

/// Starts the agent of the worktree at `path` through the core, on a thread of its own (the core
/// waits for the agent's first moments), and says how it went. Nobody can be asked to approve
/// anything here: a start that needs approval fails, saying so.
fn start_agent(path: PathBuf, size: Size, sender: Sender<Msg>) {
    thread::spawn(move || {
        let wanted = Wanted {
            agent: None,
            rows: Some(size.rows),
            cols: Some(size.cols),
            mode: StartMode::Resume,
        };
        let target = Target::Path(path.clone());
        let result = agent_start::run_in(&path, &target, &wanted, &mut Approval::Unavailable)
            .map(|_| ())
            .map_err(|err| format!("{err:#}"));
        let _ = sender.send(Msg::Started(path, result));
    });
}

/// The connection to the open worktree's holder, when there is one.
#[derive(Debug, Default)]
struct Link {
    client: Option<Client>,
    /// The worktree whose holder it is.
    worktree: Option<PathBuf>,
    /// Counts the connections, so a message from a closed one is told apart.
    number: u64,
}

impl Link {
    /// Connects to the holder of the worktree at `path`, closing any other connection first.
    fn attach(&mut self, path: &std::path::Path, sender: &Sender<Msg>) -> Result<()> {
        self.close();
        self.number += 1;
        let number = self.number;
        let admin = git::admin_dir(path)?;
        let sender = sender.clone();
        let client = Client::connect(&admin, move |heard| {
            let _ = sender.send(Msg::Heard(number, heard));
        })?;
        self.client = Some(client);
        self.worktree = Some(path.to_owned());
        Ok(())
    }

    /// Closes the connection when the panel shows another worktree's agent, or none.
    fn follow(&mut self, app: &App) {
        let showing = app.panel.as_ref().map(|panel| &panel.worktree);
        if self.worktree.as_ref() != showing {
            self.close();
        }
    }

    fn close(&mut self) {
        self.client = None;
        self.worktree = None;
    }

    fn type_bytes(&self, bytes: &[u8]) {
        if let Some(client) = &self.client {
            // A holder that went away says so through its connection closing.
            let _ = client.send_input(bytes);
        }
    }

    fn send_size(&self, size: Size) {
        if let Some(client) = &self.client {
            let _ = client.send_size(size);
        }
    }
}
