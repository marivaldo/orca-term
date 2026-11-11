//! App: reads the open worktree's changed files and drift for the right column (#177), on a thread
//! of its own so the run loop never waits for git (`ops::worktree_changes`).
//!
//! The run loop tells the thread which worktree is open, as the fleets last listed it, each time
//! that changes and each time the fleets are read again (every second). The thread reads the
//! changes again only when it matters: when another worktree opens, when the open one's entry in
//! the fleet changed (its agent's hooks write its state as it works, so a file the agent wrote
//! shows within a second), and at least every [`AGAIN_EVERY`], for the person's own edits. Most
//! seconds it reads nothing: the read is about nine git processes, the fleets' one a project.

use std::path::PathBuf;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use crate::app::Msg;
use crate::domain::fleet::{PrimaryCheckout, Worktree};
use crate::ops::worktree_changes;

/// How often the open worktree's changes are read again when nothing says they changed.
const AGAIN_EVERY: Duration = Duration::from_secs(5);

/// The open worktree, as the fleets last listed it, and its repository's primary checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Watched {
    pub(crate) primary: PrimaryCheckout,
    pub(crate) worktree: Worktree,
}

/// Starts the thread that reads the changes of the worktree it is last told of, sending each read
/// to the run loop on `sender` as [`Msg::Changes`]. Returns where to tell it; the thread ends
/// once the run loop is gone.
pub(crate) fn start(sender: Sender<Msg>) -> Sender<Watched> {
    let (tell, told) = mpsc::channel::<Watched>();
    thread::spawn(move || {
        let mut current: Option<Watched> = None;
        let mut read_at: Option<Instant> = None;
        loop {
            let due = read_at.is_none_or(|at| at.elapsed() >= AGAIN_EVERY);
            match told.recv_timeout(AGAIN_EVERY) {
                Ok(watched) if current.as_ref() == Some(&watched) && !due => continue,
                Ok(watched) => current = Some(watched),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
            let Some(watched) = &current else {
                continue;
            };
            let read = worktree_changes::read(&watched.primary, &watched.worktree)
                .map_err(|err| format!("{err:#}"));
            read_at = Some(Instant::now());
            let path: PathBuf = watched.worktree.path.clone();
            if sender.send(Msg::Changes(path, read)).is_err() {
                return;
            }
        }
    });
    tell
}
