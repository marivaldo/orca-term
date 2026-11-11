//! App: `orca-term app`, the terminal application (ADR 0012), which `orca-term` with no arguments
//! opens too at a terminal (#193): the side menu with every project's worktrees and, in the
//! centre, the open worktree's panels: its agent's terminal attached to its holder (#174), its
//! Neovim and its shell (#176); on the right, its changed files, its pull request and its branch
//! (#177). It is a client of the core: it reads the fleets and the changes
//! and starts agents through the operations, and talks to each agent's holder as `agent attach`
//! does. Quitting stops no agent; it ends the Neovims and shells the application started.
//!
//! This file is the run loop. Everything that happens arrives as one message on one channel: a key
//! or a paste (from the terminal's reader), the fleets read again, what the open agent's holder
//! sent, an agent started, what a Neovim or a shell printed, the open worktree's changes read
//! (`watch.rs`), a signal to quit. Each message
//! changes the state (`state.rs`), which answers with effects the loop carries out, and the loop
//! draws the next frame.
//!
//! Live updates come from reading every project's fleet again each second, on a thread of its own
//! (`ops::fleets::run_quick`): one listing of the worktrees by git per project, and their small
//! state files. The core runs nothing of its own to push changes, and the push it has, to the
//! Neovims that registered (`orca-term nvim register`), speaks Neovim's own protocol; a second
//! kind of listener would mean a socket and a registry for the application, where the read is one
//! git process per project a second, and the side menu needs a clock anyway, for the ages and the
//! spinner.

mod running;
mod watch;

pub(crate) mod draw;
pub(crate) mod emulator;
#[cfg(test)]
pub(crate) mod fixture;
pub(crate) mod keys;
pub(crate) mod layout;
pub(crate) mod menu;
pub(crate) mod paint;
pub(crate) mod panel;
pub(crate) mod program;
pub(crate) mod state;
pub(crate) mod theme;
pub(crate) mod view;

use std::io;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use ratatui::crossterm::event::{Event, KeyEventKind};

use crate::adapters::holder::client::{Client, Heard};
use crate::adapters::pty::Printed;
use crate::adapters::screen::{self, Screen};
use crate::adapters::{env, git, terminal};
use crate::app::paint::Mode;
use crate::app::program::ProgramId;
use crate::app::running::Running;
use crate::app::state::{App, Effect};
use crate::app::watch::Watched;
use crate::domain::agent::{Size, StartMode};
use crate::domain::changes::Changes;
use crate::domain::project::ProjectFleet;
use crate::domain::worktree::{Target, WorktreeName};
use crate::ops::agent_start::{self, Wanted};
use crate::ops::{self, Approval, fleets, pr_create, project_add, worktree_new};

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
    /// What a worktree's Neovim or shell printed, or its end.
    Printed(ProgramId, Printed),
    /// The changes of the worktree at this path, read now, or why they could not be.
    Changes(PathBuf, Result<Changes, String>),
    /// The core created the worktree named `.1` in the project at `.0`, or could not.
    Created(PathBuf, String, Result<String, String>),
    /// The core opened a draft pull request, or could not.
    Drafted(Result<String, String>),
    /// A signal asked the application to end.
    Quit,
}

/// Runs the application in this terminal until the person quits or a signal ends it. The
/// repository of the current directory joins the project list first, unless it is refused for
/// want of a terminal. The terminal is restored in every case, then the Neovims and shells it
/// started are ended.
pub(crate) fn run() -> Result<()> {
    if !terminal::stdin_and_stdout_are_terminals() {
        bail!("`orca-term app` draws on a terminal: run it in one");
    }
    project_add::add_current_repository();
    let start_in = env::current_dir().ok();
    let (no_color, colorterm) = env::colour_env();
    let mode = Mode::detect(no_color, colorterm.as_deref());
    let (sender, messages) = mpsc::channel();
    let mut programs = Running::default();
    let served = Screen::enter().and_then(|mut screen| {
        let mut app = App::new(screen.area()?, start_in);
        listen(&sender)?;
        let mut parts = Parts {
            link: Link::default(),
            programs: &mut programs,
            sender: &sender,
            watch: watch::start(sender.clone()),
            watched: None,
        };
        serve(&mut screen, &mut app, &mut parts, &messages, mode)
    });
    programs.end_all();
    served
}

/// What the loop carries the effects out with.
struct Parts<'a> {
    link: Link,
    programs: &'a mut Running,
    sender: &'a Sender<Msg>,
    /// Where to tell the reader of the open worktree's changes which worktree is open.
    watch: Sender<Watched>,
    /// What it was told last.
    watched: Option<Watched>,
}

impl Parts<'_> {
    /// Tells the reader of the changes which worktree is open, when that or its entry in the
    /// fleets changed.
    fn watch_open(&mut self, app: &App) {
        let open = app.open.as_deref().and_then(|open| {
            let (project, worktree) = menu::find(&app.fleets, open)?;
            Some(Watched {
                primary: project.fleet.as_ref().ok()?.primary.clone(),
                worktree: worktree.clone(),
            })
        });
        if open.is_some() && open != self.watched {
            self.watched.clone_from(&open);
            if let Some(watched) = open {
                let _ = self.watch.send(watched);
            }
        }
    }
}

/// Draws, waits for messages and carries out their effects, until the application quits.
fn serve(
    screen: &mut Screen,
    app: &mut App,
    parts: &mut Parts<'_>,
    messages: &Receiver<Msg>,
    mode: Mode,
) -> Result<()> {
    loop {
        for effect in app.fit(screen.area()?) {
            apply(app, parts, effect);
        }
        screen.draw(|frame| draw::frame(frame, app, env::now_millis(), mode))?;
        let Some(batch) = next_batch(messages) else {
            return Ok(());
        };
        let effects: Vec<Effect> = batch
            .into_iter()
            .flat_map(|message| handle(app, parts, message))
            .collect();
        for effect in effects {
            if !apply(app, parts, effect) {
                return Ok(());
            }
        }
        parts.link.follow(app);
        parts.watch_open(app);
        parts.programs.register_listening();
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
fn handle(app: &mut App, parts: &mut Parts<'_>, message: Msg) -> Vec<Effect> {
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
            if connection == parts.link.number
                && let Some(panel) = app.panel.as_mut()
            {
                panel.heard(heard);
            }
            Vec::new()
        }
        Msg::Started(path, result) => app.started(&path, result, now),
        Msg::Changes(path, read) => {
            app.changes_read(path, read);
            Vec::new()
        }
        Msg::Printed(id, printed) => {
            if matches!(printed, Printed::Ended(_)) {
                parts.programs.ended(&id);
            }
            app.printed(&id, printed)
        }
        Msg::Created(project, name, result) => {
            app.created(&project, &name, result, now);
            Vec::new()
        }
        Msg::Drafted(result) => {
            app.drafted(result, now);
            Vec::new()
        }
        Msg::Quit => vec![Effect::Quit],
    }
}

/// Carries out `effect`. Returns `false` to quit.
fn apply(app: &mut App, parts: &mut Parts<'_>, effect: Effect) -> bool {
    let link = &mut parts.link;
    match effect {
        Effect::Type(bytes) => link.type_bytes(&bytes),
        Effect::Attach(path) => match link.attach(&path, parts.sender) {
            Ok(()) => {
                app.attached(&path);
                if let Some(panel) = &app.panel {
                    let (rows, cols) = panel.screen.size();
                    link.send_size(Size { rows, cols });
                }
            }
            Err(err) => app.attach_failed(&path, format!("{err:#}"), Instant::now()),
        },
        Effect::Start(path, size, mode) => start_agent(path, size, mode, parts.sender.clone()),
        Effect::SizeAgent(size) => link.send_size(size),
        Effect::Run(id, size) => {
            if let Err(err) = parts.programs.run(&id, size, parts.sender) {
                app.run_failed(&id, format!("{err:#}"));
            }
        }
        Effect::TypeInto(id, bytes) => parts.programs.type_into(&id, bytes),
        Effect::Resize(id, size) => parts.programs.resize(&id, size),
        Effect::OpenDiff {
            primary,
            worktree,
            file,
        } => parts.programs.open_diff(primary, worktree, file),
        Effect::ReadLog(path) => {
            let read = ops::read_agent_log(&path).map_err(|err| format!("{err:#}"));
            app.log_read(&path, read, Instant::now());
        }
        Effect::NewWorktree { project, name } => {
            new_worktree(project, name, parts.sender.clone());
        }
        Effect::DraftPr(path) => draft_pr(path, parts.sender.clone()),
        Effect::Quit => return false,
    }
    true
}

/// Starts the agent of the worktree at `path` through the core, on the session `mode` says, on a
/// thread of its own (the core waits for the agent's first moments), and says how it went. Nobody
/// can be asked to approve anything here: a start that needs approval fails, saying so.
fn start_agent(path: PathBuf, size: Size, mode: StartMode, sender: Sender<Msg>) {
    thread::spawn(move || {
        let wanted = Wanted {
            agent: None,
            rows: Some(size.rows),
            cols: Some(size.cols),
            mode,
        };
        let target = Target::Path(path.clone());
        let result = agent_start::run_in(&path, &target, &wanted, &mut Approval::Unavailable)
            .map(|_| ())
            .map_err(|err| format!("{err:#}"));
        let _ = sender.send(Msg::Started(path, result));
    });
}

/// Creates the worktree `name` in the project at `project` through the core (`worktree new`), on
/// a thread of its own since its setup may take a while, and says how it went. The setup's output
/// goes to its log only, and a committed setup script that needs approval does not run.
fn new_worktree(project: PathBuf, name: WorktreeName, sender: Sender<Msg>) {
    thread::spawn(move || {
        let created = worktree_new::run_in(
            &project,
            &name,
            None,
            &mut Approval::Unavailable,
            &mut io::sink(),
        );
        let result = match created {
            Ok(created) if created.setup.outcome.failed() => Ok(format!(
                "created worktree {name}, but its setup failed: see {}",
                created
                    .setup
                    .log
                    .as_ref()
                    .map_or_else(String::new, |log| log.display().to_string())
            )),
            Ok(_) => Ok(format!("created worktree {name}")),
            Err(err) => Err(format!("{err:#}")),
        };
        let _ = sender.send(Msg::Created(project, name.to_string(), result));
    });
}

/// Opens a draft pull request for the worktree at `path` through the core (`pr create --draft`,
/// with the message `pr compose` makes), on a thread of its own since it pushes and asks GitHub,
/// and says how it went.
fn draft_pr(path: PathBuf, sender: Sender<Msg>) {
    thread::spawn(move || {
        let result = match pr_create::draft_in(&path, &Target::Path(path.clone())) {
            Ok(created) => Ok(created.pr().map_or_else(
                || format!("opened a draft pull request for {}", created.name),
                |pr| {
                    format!(
                        "opened draft pull request #{} for {}",
                        pr.number, created.name
                    )
                },
            )),
            Err(err) => Err(format!("{err:#}")),
        };
        let _ = sender.send(Msg::Drafted(result));
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
