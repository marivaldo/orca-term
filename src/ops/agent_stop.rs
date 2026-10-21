//! Ops: `agent stop`, which ends a worktree's agent through its holder (ADR 0009): the holder
//! hangs the agent up, escalates if it stays, and exits once the agent is gone. Stopping an agent
//! that is not running is not an error.

use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

use crate::adapters::{env, fs, holder};
use crate::domain::agent::{HolderRecord, StopOutcome, Stopped};
use crate::domain::fleet::Worktree;
use crate::domain::state::{AdminDir, LockStatus};
use crate::domain::worktree::Target;
use crate::ops::{derive_state, find_target_admin, notify_changed, read_fleet};

/// How long `agent stop` waits for the holder to go: longer than the holder's own escalation.
const STOP_TIMEOUT: Duration = Duration::from_secs(10);

/// Stops the agent of the worktree `target` names or locates in the repository containing the
/// current directory, and returns once its holder is gone.
pub(crate) fn run(target: &Target) -> Result<Stopped> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let (worktree, admin) = find_target_admin(&fleet, &cwd, target)?;
    if !worktree.holder.is_alive() {
        return Ok(Stopped {
            worktree: worktree.clone(),
            outcome: StopOutcome::NotRunning,
        });
    }
    stop_holder(&admin)?;
    notify_changed(&admin);
    let derived = derive_state(&admin)?;
    Ok(Stopped {
        worktree: Worktree {
            state: derived.state,
            activity: derived.activity,
            agent: derived.agent,
            holder: derived.holder,
            unseen: derived.unseen,
            notes: derived.notes,
            denials: derived.denials,
            operation: derived.operation,
            ..worktree.clone()
        },
        outcome: StopOutcome::Stopped,
    })
}

/// Asks the holder of `admin` to end its agent and waits until it is gone: what `agent stop` and
/// `agent swap` share.
pub(crate) fn stop_holder(admin: &AdminDir) -> Result<()> {
    holder::request_stop(admin)?;
    wait_until_gone(admin)
}

/// Waits for the holder of `admin` to release `agent.lock`. Nothing kills it by pid: a pid can be
/// reused, and the holder kills the agent itself.
fn wait_until_gone(admin: &AdminDir) -> Result<()> {
    let deadline = Instant::now() + STOP_TIMEOUT;
    while fs::lock_status_at(&admin.agent_lock())? == LockStatus::Held {
        if Instant::now() > deadline {
            let pid = fs::read_optional(&admin.agent_record())?
                .and_then(|bytes| HolderRecord::parse(&bytes).ok())
                .map_or_else(
                    || "unknown pid".to_owned(),
                    |record| format!("pid {}", record.pid),
                );
            bail!("the holder ({pid}) is still alive after {STOP_TIMEOUT:?}");
        }
        thread::sleep(Duration::from_millis(50));
    }
    Ok(())
}
