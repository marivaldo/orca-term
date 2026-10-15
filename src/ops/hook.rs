//! Ops: the hidden `hook claude` command, which claude runs for every event its settings register
//! (`domain/launch.rs`): it reads the event's payload from stdin and applies it to the worktree's
//! live status and turn log (`domain/hook.rs`, `domain/turn.rs`).
//!
//! It never fails and never prints: nothing it does may reach claude. What goes wrong is appended
//! as one line to `hook.log` in the admin directory, when that directory exists; nothing creates
//! it. It takes only its own lock, `hook.lock`, waiting at most a second, never the worktree's
//! lock, so a hook never makes a command busy. It runs no git, to stay fast.

use std::path::Path;
use std::time::Duration;

use anyhow::{Result, anyhow, bail};

use crate::adapters::{env, fs, stdin};
use crate::domain::hook::{self, Payload};
use crate::domain::state::AdminDir;
use crate::domain::turn::{self, StatusRead};

/// The only agent whose hooks the core understands so far.
const AGENT: &str = "claude";

/// How long a hook waits for another hook of the same worktree to finish.
const LOCK_WAIT: Duration = Duration::from_secs(1);

/// The size past which `hook.log` is emptied before the next line.
const HOOK_LOG_MAX: u64 = 256 * 1024;

/// How much of the end of `turns.jsonl` is read to find the latest turn's number: far more than
/// one line, whose texts are capped.
const TURN_LOG_TAIL: u64 = 64 * 1024;

/// Applies the event claude wrote to stdin to the worktree whose admin directory is `admin`, for
/// the agent `agent`. Both come from the hook's command line and may be missing; then there is
/// nothing to do, nor anywhere to log it.
pub(crate) fn run(agent: Option<&str>, admin: Option<&Path>) {
    let Some(admin) = admin else {
        return;
    };
    let admin = AdminDir::new(admin.to_owned());
    if !fs::is_dir(admin.path()) {
        return;
    }
    if let Err(err) = record(agent, &admin) {
        log(&admin, &err);
    }
}

/// Reads the payload, takes `hook.lock`, and writes what the event changes: the new turn log
/// lines first, then the status, replaced whole.
fn record(agent: Option<&str>, admin: &AdminDir) -> Result<()> {
    if agent != Some(AGENT) {
        bail!("no hooks are known for the agent {agent:?}");
    }
    let payload = Payload::parse(&stdin::read_all()?)?;
    let Some(_lock) = fs::lock_file_waiting(&admin.hook_lock(), LOCK_WAIT)? else {
        bail!(
            "hook.lock was still held after {LOCK_WAIT:?}; {} was not recorded",
            payload.event
        );
    };
    let read = turn::read_status(fs::read_optional(&admin.status_file())?.as_deref());
    turn::check_status_overwrite(&read)?;
    if let StatusRead::Unreadable(reason) = &read {
        log(
            admin,
            &anyhow!("status.json could not be read ({reason}); it starts afresh"),
        );
    }
    let status = read.usable().unwrap_or_default();
    let tail = fs::read_tail(&admin.turn_log(), TURN_LOG_TAIL)?.unwrap_or_default();
    let next = turn::next_turn_number(turn::last_turn_number(&tail)?, &status);
    let applied = hook::apply(status, &payload, env::now_millis(), next);
    let mut lines = Vec::new();
    for record in &applied.records {
        lines.extend(record.to_line()?);
    }
    if !lines.is_empty() {
        fs::append(&admin.turn_log(), &lines)?;
    }
    fs::write_atomically(&admin.status_file(), &turn::status_bytes(&applied.status)?)
}

/// Appends `err` to `hook.log` as one line, after the time. A failure to log is dropped: there
/// is nowhere else to say it.
fn log(admin: &AdminDir, err: &anyhow::Error) {
    let message = format!("{err:#}").replace('\n', " ");
    let line = format!("{} {message}\n", env::now_millis());
    let _ = fs::append_bounded(&admin.hook_log(), line.as_bytes(), HOOK_LOG_MAX);
}
