//! App: the Neovims and shells the application runs (#176), the run loop's side of them: their
//! processes (`adapters/pty.rs`), and where each Neovim listens. With `app.rs`, the one part of
//! the application that touches the outside world.
//!
//! A Neovim is registered like any other (`orca-term nvim register`, `domain/nvim.rs`) once its
//! socket is there, so the core tells it when a worktree changes; registering earlier would not
//! do, since the core forgets an address with nothing behind it. When the Neovim ends, or the
//! application quits, its address leaves the registry and a socket file left behind is removed.
//!
//! A diff to open (#177) goes to every Neovim registered in the repository (`ops::nvim_diff`), on
//! a thread of its own. When the worktree's own Neovim was just started and is not registered yet,
//! the diff waits for it, and goes once it is.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;

use crate::adapters::pty::Process;
use crate::adapters::{env, fs};
use crate::app::Msg;
use crate::app::program::{self, Kind, ProgramId};
use crate::domain::agent::Size;
use crate::domain::nvim::Address;
use crate::domain::open::Program as NvimProgram;
use crate::ops::{nvim_diff, nvim_register, nvim_unregister};

/// How long the programs have to end on quit, once hung up, before they are killed.
const END_WITHIN: Duration = Duration::from_secs(1);

/// The Neovims and shells running, and the sockets of the Neovims.
#[derive(Debug, Default)]
pub(crate) struct Running {
    processes: BTreeMap<ProgramId, Process>,
    sockets: BTreeMap<ProgramId, Socket>,
    /// How many Neovims were started, to name each one's socket.
    started: u64,
}

/// Where a Neovim listens, and whether its address is in the registry.
#[derive(Debug)]
struct Socket {
    path: PathBuf,
    registered: Registered,
    /// A diff to ask for once it is registered.
    diff: Option<Diff>,
}

/// A diff to open: the primary checkout, the worktree and the file.
#[derive(Debug, Clone)]
struct Diff {
    primary: PathBuf,
    worktree: PathBuf,
    file: String,
}

impl Diff {
    /// Asks the repository's Neovims for it, on a thread of its own: sending waits for their
    /// answers, up to a bound.
    fn send(self) {
        thread::spawn(move || {
            let _ = nvim_diff::run_in(&self.primary, &self.worktree, &self.file);
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Registered {
    /// Not yet: the socket is not there yet.
    Waiting,
    Yes,
    /// Registering failed: it is not tried again.
    Failed,
}

impl Running {
    /// Starts `id` at `size`; what it prints comes back on `sender`.
    pub(crate) fn run(&mut self, id: &ProgramId, size: Size, sender: &Sender<Msg>) -> Result<()> {
        self.ended(id);
        let argv = match id.kind {
            Kind::Nvim => {
                let socket = self.new_socket()?;
                let nvim = match NvimProgram::from_override(env::nvim_override().as_deref()) {
                    NvimProgram::Path(path) => path,
                    NvimProgram::Name(name) => name,
                };
                let argv = program::nvim_argv(&nvim, &socket);
                self.sockets.insert(
                    id.clone(),
                    Socket {
                        path: socket,
                        registered: Registered::Waiting,
                        diff: None,
                    },
                );
                argv
            }
            Kind::Shell => program::shell_argv(env::shell().as_deref()),
        };
        let (sender, tag) = (sender.clone(), id.clone());
        let started = Process::start(&argv, &id.worktree, size, move |printed| {
            let _ = sender.send(Msg::Printed(tag.clone(), printed));
        });
        match started {
            Ok(process) => {
                self.processes.insert(id.clone(), process);
                Ok(())
            }
            Err(err) => {
                self.ended(id);
                Err(err)
            }
        }
    }

    /// A fresh socket path for the next Neovim, in a directory of this user's alone.
    fn new_socket(&mut self) -> Result<PathBuf> {
        let dir = program::socket_dir(env::runtime_dir().as_deref(), env::uid());
        fs::create_private_dir(&dir)?;
        self.started += 1;
        let socket = program::socket_path(&dir, env::pid(), self.started);
        // One left by an earlier application that had the same process id.
        fs::remove_if_exists(&socket)?;
        Ok(socket)
    }

    pub(crate) fn type_into(&self, id: &ProgramId, bytes: Vec<u8>) {
        if let Some(process) = self.processes.get(id) {
            process.type_bytes(bytes);
        }
    }

    pub(crate) fn resize(&self, id: &ProgramId, size: Size) {
        if let Some(process) = self.processes.get(id) {
            process.resize(size);
        }
    }

    /// Asks the Neovims of the repository whose primary checkout is `primary` to open the diff of
    /// `file` in the worktree at `worktree`: now, or once the worktree's own Neovim, just started,
    /// is registered.
    pub(crate) fn open_diff(&mut self, primary: PathBuf, worktree: PathBuf, file: String) {
        let id = ProgramId {
            worktree: worktree.clone(),
            kind: Kind::Nvim,
        };
        let diff = Diff {
            primary,
            worktree,
            file,
        };
        match self.sockets.get_mut(&id) {
            Some(socket) if socket.registered == Registered::Waiting => socket.diff = Some(diff),
            _ => diff.send(),
        }
    }

    /// Registers each Neovim whose socket has appeared since the last look, and asks it for the
    /// diff waiting for it.
    pub(crate) fn register_listening(&mut self) {
        for (id, socket) in &mut self.sockets {
            if socket.registered != Registered::Waiting || !fs::is_socket(&socket.path) {
                continue;
            }
            let registered = address(&socket.path)
                .is_some_and(|address| nvim_register::run_in(&id.worktree, &address).is_ok());
            socket.registered = if registered {
                Registered::Yes
            } else {
                Registered::Failed
            };
            if let Some(diff) = socket.diff.take() {
                diff.send();
            }
        }
    }

    /// `id` ended: its Neovim's address leaves the registry and its socket goes.
    pub(crate) fn ended(&mut self, id: &ProgramId) {
        self.processes.remove(id);
        let Some(socket) = self.sockets.remove(id) else {
            return;
        };
        if socket.registered == Registered::Yes
            && let Some(address) = address(&socket.path)
        {
            let _ = nvim_unregister::run_in(&id.worktree, &address);
        }
        if fs::is_socket(&socket.path) {
            let _ = fs::remove_if_exists(&socket.path);
        }
    }

    /// Ends every program: hangs them all up, gives them [`END_WITHIN`] together, then kills
    /// those still there, and forgets their sockets.
    pub(crate) fn end_all(&mut self) {
        for process in self.processes.values() {
            process.hang_up();
        }
        let deadline = Instant::now() + END_WITHIN;
        for process in self.processes.values() {
            process.end_by(deadline);
        }
        let ids: Vec<ProgramId> = self.processes.keys().cloned().collect();
        for id in ids {
            self.ended(&id);
        }
    }
}

/// The address of the Neovim listening at `socket`.
fn address(socket: &std::path::Path) -> Option<Address> {
    socket.to_str()?.parse().ok()
}
