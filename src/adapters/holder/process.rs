//! Adapter: the holder's own process (ADR 0009), run by the hidden `agent holder` command. It
//! takes `agent.lock`, binds `agent.sock`, opens a PTY, starts the agent on it and then, until
//! the agent exits, keeps its last 8 MiB of output, replays them to every client that connects,
//! forwards each client's input and size, and ends the agent when asked. For an agent that
//! reports its events through a pipe (opencode, #35), it also runs the hook for each line
//! (`holder/events.rs`).
//!
//! Threads, all blocking:
//!
//! - the main thread reaps the agent;
//! - the reader pumps the PTY into the scrollback and to every client's queue;
//! - the acceptor starts one client thread per connection, which reads that client's frames;
//! - each client has a writer that sends the replay, then the queued frames, then closes;
//! - a stop spawns the escalation: hang up, then terminate after 3 s, then kill after 3 s more;
//! - for an agent with an events pipe, the events thread runs one hook per line, in order.
//!
//! A client that stops reading is dropped rather than stalling the agent: its queue is bounded.
//! Signals reach the agent only under the mutex that reaps it, so a reused pid is never hit.
#![expect(
    clippy::disallowed_types,
    reason = "this adapter is the holder process: it owns the PTY, the socket and the lock file"
)]

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, ErrorKind, PipeReader, Read as _, Write as _};
use std::net::Shutdown;
use std::os::fd::AsFd as _;
use std::os::unix::net::{UnixListener, UnixStream};
use std::os::unix::process::ExitStatusExt as _;
use std::process::{Child, ExitStatus};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use pty_process::blocking::Pty;
use rustix::process::{Pid, Signal};

use crate::adapters::fs::{self, FileLock};
use crate::adapters::holder::{HolderArgs, error_line, events, peer, ready_line};
use crate::domain::agent::{HolderRecord, Size};
use crate::domain::hook::EVENTS_FD_VAR;
use crate::domain::protocol::{self, Decoder, Frame, SCROLLBACK_CAP, Scrollback};
use crate::domain::state::{AGENT_SOCKET_FILE, AdminDir};

/// How long the holder keeps trying `agent.lock` before calling another holder alive: a read-only
/// probe (`worktree ls`) holds the lock for microseconds and must not be mistaken for one.
const LOCK_TRIES: u32 = 20;
const LOCK_RETRY: Duration = Duration::from_millis(25);

/// How long the holder watches a freshly started agent before reporting it alive, so an agent that
/// dies at once (bad settings, no sandbox) is reported as such instead of as started.
const EARLY_EXIT_GRACE: Duration = Duration::from_millis(500);

/// Frames a client may have queued before it is dropped as stuck.
const CLIENT_QUEUE: usize = 1024;

/// Between the two halves of a jiggle, and before the jiggle repeated after an attach.
const JIGGLE_PAUSE: Duration = Duration::from_millis(150);
const JIGGLE_REPEAT: Duration = Duration::from_secs(1);

/// Between the hang-up, the terminate and the kill of an agent that does not end.
const ESCALATION_STEP: Duration = Duration::from_secs(3);

/// How long a write to a client may block before the client counts as stuck (an attach suspended
/// in its shell, a peer that never reads) and is dropped.
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(3);

/// How long the holder waits for its writers once the agent exited, before leaving without them.
const WRITERS_DEADLINE: Duration = Duration::from_secs(5);

/// How long the holder waits, once the agent exited, for the hooks of the events it reported last.
const EVENTS_DEADLINE: Duration = Duration::from_secs(5);

/// Runs the holder: reports on stdout, then serves until the agent exits.
pub(crate) fn serve(args: &HolderArgs) -> Result<()> {
    match prepare(args) {
        Err(err) => {
            report(&error_line(&format!("{err:#}")));
            Err(err)
        }
        Ok((holder, listener, lock)) => {
            report(&ready_line(std::process::id()));
            silence_stdout()?;
            let result = run(&holder, listener);
            drop(lock);
            result
        }
    }
}

/// What the holder keeps for its whole life.
struct Holder {
    admin: AdminDir,
    shared: Mutex<Shared>,
    pty: Pty,
    /// Serialises writes to the PTY, so input from two clients never interleaves mid-write.
    input: Mutex<()>,
    /// The agent. It is reaped, and signalled, only under this mutex.
    child: Mutex<Child>,
    /// The size the PTY was last set to.
    size: Mutex<Size>,
    /// The thread running the hook for each event the agent reports through its pipe, for an
    /// agent that does (opencode).
    events: Mutex<Option<JoinHandle<()>>>,
}

/// What the reader and the client threads share, under one mutex: a client takes its replay and
/// registers its queue in the same critical section the reader appends and broadcasts in, so every
/// byte lands either in the replay or in the queue, never in both or in neither.
struct Shared {
    scrollback: Scrollback,
    clients: HashMap<u64, SyncSender<Vec<u8>>>,
    writers: Vec<JoinHandle<()>>,
    next_client: u64,
    /// Set once the agent ended: a client attaching afterwards gets the replay and the exit.
    exit: Option<String>,
    /// Set once the reader saw the PTY close.
    reader_done: bool,
}

/// Everything before the ready line: the session, the lock, the socket, the PTY and the agent.
fn prepare(args: &HolderArgs) -> Result<(Arc<Holder>, UnixListener, FileLock)> {
    // A session of our own: no controlling terminal, and out of the starter's process group, so
    // whatever ends the starter leaves the holder alone.
    rustix::process::setsid().context("could not start a session for the holder")?;
    let lock = take_agent_lock(&args.admin)?;
    // Relative bind after the change of directory: see the module doc of `holder`.
    std::env::set_current_dir(args.admin.path())
        .with_context(|| format!("could not enter {}", args.admin.path().display()))?;
    fs::remove_if_exists(&args.admin.agent_socket())?;
    let listener = UnixListener::bind(AGENT_SOCKET_FILE)
        .with_context(|| format!("could not bind {}", args.admin.agent_socket().display()))?;
    let (pty, pts) = pty_process::blocking::open().context("could not open a pty")?;
    pty.resize(pty_size(args.size))
        .context("could not size the pty")?;
    let (child, events) = spawn_agent(args, pts)?;
    let holder = Arc::new(Holder {
        admin: args.admin.clone(),
        shared: Mutex::new(Shared {
            scrollback: Scrollback::new(SCROLLBACK_CAP),
            clients: HashMap::new(),
            writers: Vec::new(),
            next_client: 0,
            exit: None,
            reader_done: false,
        }),
        pty,
        input: Mutex::new(()),
        child: Mutex::new(child),
        size: Mutex::new(args.size),
        events: Mutex::new(None),
    });
    start_reader(&holder);
    if let Some(status) = exited_early(&holder)? {
        let last = lock_shared(&holder)
            .scrollback
            .last_line()
            .unwrap_or_default();
        fs::remove_if_exists(&args.admin.agent_socket())?;
        bail!(
            "{} exited at once ({}): {last}",
            args.argv.first().map_or("the agent", String::as_str),
            protocol::exit_text(status.code(), status.signal())
        );
    }
    if let Some(reader) = events {
        start_events(&holder, args, reader)?;
    }
    let record = HolderRecord {
        pid: std::process::id(),
        socket: args.admin.agent_socket(),
    };
    fs::write_atomically(&args.admin.agent_record(), &record.to_bytes()?)?;
    Ok((holder, listener, lock))
}

/// Starts the agent `args` names on the PTY `pts`, in the worktree. For an agent that reports its
/// events through a pipe (`args.events`), the agent inherits the pipe's write end, its number in
/// [`EVENTS_FD_VAR`], and the holder keeps only the read end, which it returns.
fn spawn_agent(
    args: &HolderArgs,
    pts: pty_process::blocking::Pts,
) -> Result<(Child, Option<PipeReader>)> {
    let (program, rest) = args
        .argv
        .split_first()
        .context("the holder was given no command to run")?;
    let mut command = pty_process::blocking::Command::new(program)
        .args(rest)
        .current_dir(&args.worktree);
    let pipe = match args.events {
        Some(_) => Some(events::Pipe::open()?),
        None => None,
    };
    if let Some(pipe) = &pipe {
        command = command.env(EVENTS_FD_VAR, pipe.writer_fd());
    }
    let child = command
        .spawn(pts)
        .with_context(|| format!("could not start {program}"))?;
    // The agent holds the write end now: the holder closes its own copy, so the pipe ends with
    // the agent and whatever it started that kept it.
    let reader = pipe.map(|pipe| pipe.reader);
    Ok((child, reader))
}

/// Starts the thread that runs the hook of `args.events` for each line the agent writes to
/// `reader`, and keeps it so the holder waits for the last events before it leaves.
fn start_events(holder: &Holder, args: &HolderArgs, reader: PipeReader) -> Result<()> {
    let Some(agent) = &args.events else {
        return Ok(());
    };
    let hook = events::Hook {
        exe: std::env::current_exe().context("could not find the orca-term binary")?,
        agent: agent.clone(),
        admin: args.admin.path().to_owned(),
        worktree: args.worktree.clone(),
    };
    *lock_or_recover(&holder.events) = Some(events::start(reader, hook));
    Ok(())
}

/// Takes `agent.lock`, retrying for a moment; still held means another holder is alive.
fn take_agent_lock(admin: &AdminDir) -> Result<FileLock> {
    let path = admin.agent_lock();
    for _ in 0..LOCK_TRIES {
        if let Some(lock) = fs::try_lock_file(&path)? {
            return Ok(lock);
        }
        thread::sleep(LOCK_RETRY);
    }
    bail!("an agent is already running in this worktree")
}

/// The agent's exit status when it died within the grace period, `None` when it is still running.
fn exited_early(holder: &Holder) -> Result<Option<ExitStatus>> {
    let deadline = Instant::now() + EARLY_EXIT_GRACE;
    while Instant::now() < deadline {
        if let Some(status) = lock_child(holder).try_wait()? {
            // Give the reader a moment to drain what the agent printed before dying.
            thread::sleep(Duration::from_millis(50));
            return Ok(Some(status));
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(None)
}

/// Writes `line` to stdout, the starter's pipe.
fn report(line: &str) {
    let mut stdout = io::stdout().lock();
    let _ = stdout.write_all(line.as_bytes());
    let _ = stdout.flush();
}

/// Points stdout at `/dev/null`, so nothing can write to the starter's pipe again and the starter
/// sees it close.
fn silence_stdout() -> Result<()> {
    let devnull = File::options()
        .write(true)
        .open("/dev/null")
        .context("could not open /dev/null")?;
    rustix::stdio::dup2_stdout(devnull.as_fd()).context("could not silence the holder's stdout")?;
    Ok(())
}

/// Serves until the agent exits, then lets every client know, cleans up and returns.
fn run(holder: &Arc<Holder>, listener: UnixListener) -> Result<()> {
    start_acceptor(holder, listener);
    let status = loop {
        if let Some(status) = lock_child(holder).try_wait()? {
            break status;
        }
        thread::sleep(Duration::from_millis(50));
    };
    // Let the reader drain the agent's last output; a grandchild still holding the terminal would
    // keep it blocked, so the wait is bounded.
    let drained = Instant::now() + Duration::from_secs(1);
    while Instant::now() < drained && !lock_shared(holder).reader_done {
        thread::sleep(Duration::from_millis(20));
    }
    // Let the hooks of the agent's last events run (its turn's end, its own end); a grandchild
    // still holding the pipe would keep it open, so the wait is bounded too.
    let events = lock_or_recover(&holder.events).take();
    if let Some(events) = events {
        join_within(vec![events], EVENTS_DEADLINE);
    }
    let writers = {
        let mut shared = lock_shared(holder);
        let text = protocol::exit_text(status.code(), status.signal());
        let frame = Frame::Exit(text.clone()).encode();
        for queue in shared.clients.values() {
            let _ = queue.try_send(frame.clone());
        }
        shared.exit = Some(text);
        // Dropping the queues lets each writer drain what it holds, then close its socket.
        shared.clients.clear();
        std::mem::take(&mut shared.writers)
    };
    join_within(writers, WRITERS_DEADLINE);
    fs::remove_if_exists(&holder.admin.agent_socket())?;
    fs::remove_if_exists(&holder.admin.agent_record())?;
    Ok(())
}

/// Joins the writers that end within `deadline`; one still stuck in a write past it is left behind,
/// since the process is about to exit anyway.
fn join_within(mut writers: Vec<JoinHandle<()>>, deadline: Duration) {
    let end = Instant::now() + deadline;
    while !writers.is_empty() && Instant::now() < end {
        let (done, pending): (Vec<_>, Vec<_>) =
            writers.into_iter().partition(JoinHandle::is_finished);
        for writer in done {
            let _ = writer.join();
        }
        writers = pending;
        thread::sleep(Duration::from_millis(20));
    }
}

/// Starts the reader: PTY output into the scrollback and to every client's queue.
fn start_reader(holder: &Arc<Holder>) {
    let holder = Arc::clone(holder);
    thread::spawn(move || {
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            let n = match (&holder.pty).read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(err) if err.kind() == ErrorKind::Interrupted => continue,
                // Linux answers EIO once the agent closed its side; macOS answers 0. Either way
                // the agent's terminal is gone.
                Err(_) => break,
            };
            let mut shared = lock_shared(&holder);
            shared.scrollback.push(&buf[..n]);
            let frame = Frame::Output(buf[..n].to_vec()).encode();
            // A client whose queue is full, or whose writer is gone, is dropped here.
            shared
                .clients
                .retain(|_, queue| queue.try_send(frame.clone()).is_ok());
        }
        lock_shared(&holder).reader_done = true;
    });
}

/// Starts the acceptor: one client thread per connection. A client the holder does not serve
/// (on Linux, one from another network namespace: see `holder::peer`) is closed at once, before
/// any replay is written to it and before any of its input is read.
fn start_acceptor(holder: &Arc<Holder>, listener: UnixListener) {
    let holder = Arc::clone(holder);
    thread::spawn(move || {
        for connection in listener.incoming() {
            let Ok(connection) = connection else { continue };
            let holder = Arc::clone(&holder);
            thread::spawn(move || serve_if_admitted(&holder, connection));
        }
    });
}

/// Serves the client at `connection` if the holder admits it; otherwise drops the connection,
/// which closes it, having written nothing to it and read nothing from it.
fn serve_if_admitted(holder: &Arc<Holder>, connection: UnixStream) {
    if peer::admitted(&connection) {
        serve_client(holder, connection);
    }
}

/// One client, from its connection to its end: registers it, starts its writer, then reads its
/// frames until it closes.
fn serve_client(holder: &Arc<Holder>, connection: UnixStream) {
    let Ok(writer_stream) = connection.try_clone() else {
        return;
    };
    let (queue, frames) = mpsc::sync_channel::<Vec<u8>>(CLIENT_QUEUE);
    let (id, replay, exit) = {
        let mut shared = lock_shared(holder);
        let id = shared.next_client;
        shared.next_client += 1;
        let exit = shared.exit.clone();
        if exit.is_none() {
            shared.clients.insert(id, queue);
        }
        (id, shared.scrollback.snapshot(), exit)
    };
    let writer = thread::spawn(move || write_client(writer_stream, replay, exit, &frames));
    lock_shared(holder).writers.push(writer);
    read_client(holder, connection);
    lock_shared(holder).clients.remove(&id);
}

/// One client's writer: the replay, then the agent's end if it already ended, else every frame
/// queued for the client until its queue is dropped, then the socket is closed. A write that
/// blocks for [`CLIENT_WRITE_TIMEOUT`] ends the writer: the client stopped reading.
fn write_client(
    mut stream: UnixStream,
    replay: Vec<u8>,
    exit: Option<String>,
    frames: &mpsc::Receiver<Vec<u8>>,
) {
    if stream
        .set_write_timeout(Some(CLIENT_WRITE_TIMEOUT))
        .is_err()
    {
        return;
    }
    if stream.write_all(&Frame::Output(replay).encode()).is_err() {
        return;
    }
    match exit {
        Some(text) => {
            let _ = stream.write_all(&Frame::Exit(text).encode());
        }
        None => {
            for frame in frames {
                if stream.write_all(&frame).is_err() {
                    break;
                }
            }
        }
    }
    let _ = stream.shutdown(Shutdown::Both);
}

/// Reads one client's frames until it closes or sends something that is not the protocol.
fn read_client(holder: &Arc<Holder>, mut connection: UnixStream) {
    let mut decoder = Decoder::default();
    let mut buf = vec![0u8; 16 * 1024];
    let mut first_resize = true;
    loop {
        let n = match connection.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => n,
        };
        decoder.push(&buf[..n]);
        loop {
            match decoder.next_frame() {
                Ok(Some(Frame::Input(bytes))) => {
                    let _guard = lock_or_recover(&holder.input);
                    let _ = (&holder.pty).write_all(&bytes);
                }
                Ok(Some(Frame::Resize(size))) => {
                    resize(holder, size, std::mem::take(&mut first_resize));
                }
                Ok(Some(Frame::Stop)) => stop_agent(holder),
                Ok(Some(Frame::Output(_) | Frame::Exit(_))) => {}
                Ok(None) => break,
                Err(_) => return,
            }
        }
    }
}

/// Sets the agent's terminal size. A client's first size also jiggles, now and again a second
/// later, so the agent redraws its screen for the new terminal even when the size is unchanged
/// (a resize to the same size changes nothing for it, and a size sent within the agent's first
/// moments can be lost).
fn resize(holder: &Arc<Holder>, size: Size, first_from_client: bool) {
    *lock_or_recover(&holder.size) = size;
    if !first_from_client {
        let _ = holder.pty.resize(pty_size(size));
        return;
    }
    jiggle(holder, size);
    let holder = Arc::clone(holder);
    thread::spawn(move || {
        thread::sleep(JIGGLE_REPEAT);
        let current = *lock_or_recover(&holder.size);
        jiggle(&holder, current);
    });
}

fn jiggle(holder: &Holder, size: Size) {
    let _ = holder.pty.resize(pty_size(size.jiggled()));
    thread::sleep(JIGGLE_PAUSE);
    let _ = holder.pty.resize(pty_size(size));
}

fn pty_size(size: Size) -> pty_process::Size {
    pty_process::Size::new(size.rows, size.cols)
}

/// Hangs the agent up, as closing its terminal would, and escalates if it stays: terminate, then
/// kill. The holder ends once the agent is reaped.
fn stop_agent(holder: &Arc<Holder>) {
    signal_agent(holder, Signal::HUP);
    let holder = Arc::clone(holder);
    thread::spawn(move || {
        thread::sleep(ESCALATION_STEP);
        signal_agent(&holder, Signal::TERM);
        thread::sleep(ESCALATION_STEP);
        signal_agent(&holder, Signal::KILL);
    });
}

/// Sends `signal` to the agent's process group, under the mutex that reaps it and only while it
/// is still running, so a pid reused by another process is never hit.
fn signal_agent(holder: &Holder, signal: Signal) {
    let mut child = lock_child(holder);
    if matches!(child.try_wait(), Ok(None)) {
        let _ = rustix::process::kill_process_group(Pid::from_child(&child), signal);
    }
}

fn lock_shared(holder: &Holder) -> MutexGuard<'_, Shared> {
    lock_or_recover(&holder.shared)
}

fn lock_child(holder: &Holder) -> MutexGuard<'_, Child> {
    lock_or_recover(&holder.child)
}

/// A mutex guard even when another thread panicked while holding it: the data is plain state, and
/// the holder should keep serving.
fn lock_or_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}
