//! Ops: the hidden `hook claude` command, which claude runs for every event its settings register
//! (`domain/launch.rs`), and `hook opencode`, which the holder runs for every line opencode's
//! plugin writes to its events pipe (#35): it reads the event's payload from stdin (opencode's
//! line read as claude's payload by `domain/hook/opencode.rs`) and applies it to the worktree's
//! live status and turn log (`domain/hook.rs`, `domain/turn.rs`).
//!
//! It never fails and never prints: nothing it does may reach claude. What goes wrong is appended
//! as one line to `hook.log` in the admin directory, when that directory exists; nothing creates
//! it. It takes only its own lock, `hook.lock`, waiting at most a second, never the worktree's
//! lock, so a hook never makes a command busy.
//!
//! An event that opens or closes a turn takes one snapshot of the worktree's tree
//! (`domain/snapshot.rs`, ADR 0006) under `hook.lock`: copy the real index to a throwaway one,
//! `git add -A` and `git write-tree` through it, `git commit-tree`, then move the worktree's chain
//! ref with a compare-and-swap, all before the turn log and the status are written. Every other
//! event runs no git, to stay fast. A snapshot that fails never loses the turn: the error goes to
//! `hook.log`, the turn is recorded without the snapshot's id, and the chain's tip stays as it was.
//! A status that fails to write after the ref moved leaves a ref that does not match the recorded
//! tip, so the next snapshot starts the chain again.
//!
//! Once the status is written, and `hook.lock` released, it tells the running Neovims that the
//! worktree changed (`ops::notify_changed`), which never fails and is bounded in time.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use crate::adapters::{env, fs, git, stdin};
use crate::domain::agent::Agent;
use crate::domain::hook::{self, Applied, Payload};
use crate::domain::review::Commit;
use crate::domain::snapshot::{self, Bounds, Identity};
use crate::domain::state::AdminDir;
use crate::domain::turn::{self, StatusRead};
use crate::ops::{notify_changed, worktree_of};

/// How long a hook waits for another hook of the same worktree to finish.
const LOCK_WAIT: Duration = Duration::from_secs(1);

/// The size past which `hook.log` is emptied before the next line.
const HOOK_LOG_MAX: u64 = 256 * 1024;

/// The mode of the throwaway copy of the index: only its owner reads it.
const INDEX_COPY_MODE: u32 = 0o600;

/// How much of the end of `turns.jsonl` is read to find the latest turn's number: far more than
/// one line, whose texts are capped.
const TURN_LOG_TAIL: u64 = 64 * 1024;

/// Applies the event the agent wrote to stdin to the worktree whose admin directory is `admin`,
/// for the agent `agent` as its hooks name it (`claude`, `opencode`). Both come from the hook's
/// command line and may be missing; then there is nothing to do, nor anywhere to log it.
pub(crate) fn run(agent: Option<&str>, admin: Option<&Path>) {
    let Some(admin) = admin else {
        return;
    };
    let admin = AdminDir::new(admin.to_owned());
    if !fs::is_dir(admin.path()) {
        return;
    }
    match record(agent, &admin) {
        Ok(true) => notify_changed(&admin),
        Ok(false) => {}
        Err(err) => log(&admin, &err),
    }
}

/// The payload on stdin, as the agent `agent` wrote it: claude's own, or a line of opencode's
/// plugin read as claude's (`None` for a line that changes nothing).
fn read_payload(agent: Agent) -> Result<Option<Payload>> {
    let bytes = stdin::read_all()?;
    match agent {
        Agent::ClaudeCode => Ok(Some(Payload::parse(&bytes)?)),
        Agent::OpenCode => hook::opencode::payload(&bytes),
    }
}

/// Reads the payload, takes `hook.lock`, and writes what the event changes: the new turn log
/// lines first, then the status, replaced whole. Returns whether anything was written.
fn record(agent: Option<&str>, admin: &AdminDir) -> Result<bool> {
    let Some(agent) = agent.and_then(Agent::from_program) else {
        bail!("no hooks are known for the agent {agent:?}");
    };
    let Some(payload) = read_payload(agent)? else {
        return Ok(false);
    };
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
    let now = env::now_millis();
    let mut applied = hook::apply(status, &payload, agent.program(), now, next);
    if let Some(bounds) = snapshot::bounds(&applied.records) {
        match take_snapshot(admin, agent, &applied, &bounds, now) {
            Ok(id) => snapshot::fill(&mut applied.records, &mut applied.status, id.as_str()),
            Err(err) => log(
                admin,
                &err.context("no snapshot was taken; the turn is recorded without it"),
            ),
        }
    }
    let mut lines = Vec::new();
    for record in &applied.records {
        lines.extend(record.to_line()?);
    }
    if !lines.is_empty() {
        fs::append(&admin.turn_log(), &lines)?;
    }
    fs::write_atomically(&admin.status_file(), &turn::status_bytes(&applied.status)?)?;
    Ok(true)
}

/// Takes the snapshot that bounds what `applied` opened or closed (`bounds`), at `now`, and moves
/// the worktree's chain ref to it. The end of a turn is `agent`'s. Returns the snapshot's commit.
fn take_snapshot(
    admin: &AdminDir,
    agent: Agent,
    applied: &Applied,
    bounds: &Bounds,
    now: u64,
) -> Result<Commit> {
    let id = admin
        .worktree_id()
        .context("the admin directory is not in git's admin directory of a worktree")?;
    let worktree = worktree_of(admin).context("git's gitdir file names no worktree")?;
    let name = worktree.file_name().unwrap_or_default().to_string_lossy();
    let author = if snapshot::authored_by_agent(bounds) {
        Identity::agent(agent.program())
    } else {
        Identity::person(
            git::config_value(&worktree, "user.name")?,
            git::config_value(&worktree, "user.email")?,
        )
    };
    let tree = snapshot_tree(admin, &worktree)?;
    let chain = snapshot::chain_ref(id);
    let current = git::read_ref(&worktree, &chain)?;
    let step = snapshot::chain_step(
        applied.status.snapshot.as_deref(),
        current.as_ref().map(Commit::as_str),
    );
    let commit = git::commit_tree(
        &worktree,
        &tree,
        step.parent.as_deref(),
        &snapshot::message(bounds, &name),
        &author,
        &Identity::committer(),
        &snapshot::git_date(now),
    )?;
    git::update_ref(&worktree, &chain, &commit, step.expected.as_deref())?;
    Ok(commit)
}

/// The tree of the worktree at `worktree`, taken through a throwaway copy of its index in the
/// admin directory (an empty one when it has no index yet), removed afterwards whatever happened.
fn snapshot_tree(admin: &AdminDir, worktree: &Path) -> Result<String> {
    let copy = admin.snapshot_index();
    // A copy left by a hook that died would stand in for a missing index.
    fs::remove_if_exists(&copy)?;
    let tree = copy_index(worktree, &copy).and_then(|()| git::write_tree_through(worktree, &copy));
    let removed = fs::remove_if_exists(&copy);
    let tree = tree?;
    removed?;
    Ok(tree)
}

/// Copies the real index of the worktree at `worktree` to `copy`; nothing when it has none.
fn copy_index(worktree: &Path, copy: &Path) -> Result<()> {
    let real = git::index_path(worktree)?;
    if fs::is_file(&real) {
        fs::byte_copy(&real, copy, INDEX_COPY_MODE)
            .with_context(|| format!("could not copy the index {}", real.display()))?;
    }
    Ok(())
}

/// Appends `err` to `hook.log` as one line, after the time. A failure to log is dropped: there
/// is nowhere else to say it.
fn log(admin: &AdminDir, err: &anyhow::Error) {
    let message = format!("{err:#}").replace('\n', " ");
    let line = format!("{} {message}\n", env::now_millis());
    let _ = fs::append_bounded(&admin.hook_log(), line.as_bytes(), HOOK_LOG_MAX);
}
