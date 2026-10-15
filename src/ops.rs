//! Ops: one module per command, each a short sequence of adapter and domain calls. This file holds
//! what several commands share: reading the fleet, finding the worktree a command names, locking
//! a worktree, writing its state, probing its holder and running its setup.

pub(crate) mod agent_attach;
pub(crate) mod agent_holder;
pub(crate) mod agent_start;
pub(crate) mod agent_stop;
pub(crate) mod hook;
pub(crate) mod worktree_ls;
pub(crate) mod worktree_new;
pub(crate) mod worktree_prune;
pub(crate) mod worktree_rm;
pub(crate) mod worktree_setup;

use std::io::Write;
use std::path::Path;

use anyhow::{Result, bail};

use crate::adapters::fs::{self, WorktreeLock};
use crate::adapters::{env, git, script};
use crate::domain::agent::{Agent, HolderStatus};
use crate::domain::config::{self, Env, Layer};
use crate::domain::error::{BusyReason, TypedError};
use crate::domain::fleet::{Fleet, PrimaryCheckout, Worktree};
use crate::domain::setup::{self, SetupOutcome, SetupRecord, SetupReport};
use crate::domain::state::{self, AdminDir, BrokenReason, LockStatus, State, WorktreeFile};
use crate::domain::worktree::{self, Target};

/// Reads the fleet of the repository containing `dir`, with each worktree's state. Never writes.
pub(crate) fn read_fleet(dir: &Path) -> Result<Fleet> {
    let entries = git::worktree_list(dir)?;
    let prunable: Vec<Option<String>> = entries
        .iter()
        .skip(1)
        .map(|entry| entry.prunable.clone())
        .collect();
    let mut fleet = Fleet::from_entries(entries)?;
    for (worktree, prunable) in fleet.worktrees.iter_mut().zip(prunable) {
        let derived = read_state(&worktree.path, prunable);
        worktree.state = derived.state;
        worktree.agent = derived.agent;
        worktree.holder = derived.holder;
    }
    Ok(fleet)
}

/// A worktree's state and the facts beside it, derived on read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Derived {
    pub(crate) state: State,
    /// The agent recorded in `worktree.json`, by name.
    pub(crate) agent: Option<String>,
    /// Whether a holder is alive right now.
    pub(crate) holder: HolderStatus,
}

impl Derived {
    fn broken(reason: BrokenReason) -> Self {
        Self {
            state: State::Broken(reason),
            agent: None,
            holder: HolderStatus::Gone,
        }
    }
}

/// The state of the worktree at `worktree`, which git may mark `prunable`. A worktree whose
/// directory is gone, or whose state cannot be read, is broken: reading one worktree never fails
/// the fleet.
fn read_state(worktree: &Path, prunable: Option<String>) -> Derived {
    if !fs::is_dir(worktree) {
        return Derived::broken(BrokenReason::DirectoryMissing);
    }
    if let Some(reason) = prunable {
        return Derived::broken(BrokenReason::Prunable(reason));
    }
    match git::admin_dir(worktree).and_then(|admin| derive_state(&admin)) {
        Ok(derived) => derived,
        Err(err) => Derived::broken(BrokenReason::StateUnreadable(format!("{err:#}"))),
    }
}

/// The state of the worktree whose admin directory is `admin`, from its state file and the
/// volatile facts behind it. Only reads: the worktree's lock is probed only for a setup recorded
/// as running, the holder's lock is probed and released at once, and the setup log is read only
/// for a failed setup.
pub(crate) fn derive_state(admin: &AdminDir) -> Result<Derived> {
    let mut recorded = read_recorded(admin)?;
    if recorded.setup == Some(SetupRecord::Running) {
        if fs::lock_status(admin)? == LockStatus::Held {
            return Ok(Derived {
                state: state::setup_running(LockStatus::Held),
                agent: recorded.agent,
                holder: HolderStatus::Gone,
            });
        }
        // The lock is free, but the setup may have ended between the read and the probe: it
        // records its end before it releases the lock, so reading the file once more tells a
        // finished setup from a dead one. Still only a read.
        recorded = read_recorded(admin)?;
    }
    let holder = HolderStatus::from_lock(fs::lock_status_at(&admin.agent_lock())?);
    let state = match recorded.setup {
        // Still recorded as running, with its lock seen free: the process died.
        Some(SetupRecord::Running) => state::setup_running(LockStatus::Free),
        Some(SetupRecord::Failed(exit)) => {
            let log = fs::read_optional(&admin.setup_log())?.unwrap_or_default();
            state::setup_failed(exit, &log)
        }
        Some(SetupRecord::Succeeded) | None => state::ready(recorded.agent.as_deref()),
    };
    Ok(Derived {
        state,
        agent: recorded.agent,
        holder,
    })
}

/// What the state file in `admin` records.
fn read_recorded(admin: &AdminDir) -> Result<state::Recorded> {
    let path = admin.state_file();
    state::read(&path, fs::read_optional(&path)?.as_deref())
}

/// The worktree `target` names, with its admin directory, for a command that must not change the
/// fleet: the agent commands.
pub(crate) fn find_target_admin<'f>(
    fleet: &'f Fleet,
    dir: &Path,
    target: &Target,
) -> Result<(&'f Worktree, AdminDir)> {
    let worktree = find_target(fleet, dir, target)?;
    let admin = git::admin_dir(&worktree.path)?;
    Ok((worktree, admin))
}

/// Refuses to change a worktree whose holder is alive: git must not touch the admin directory
/// under it, and its state is the agent's. The typed error `busy`, naming `agent stop`.
pub(crate) fn check_no_holder(worktree: &Worktree) -> Result<()> {
    if worktree.holder.is_alive() {
        return Err(TypedError::Busy {
            name: worktree.name.clone(),
            path: worktree.path.clone(),
            reason: BusyReason::AgentRunning,
        }
        .into());
    }
    Ok(())
}

/// The worktree `target` names (a basename) or locates (a path, relative to `dir`).
pub(crate) fn find_target<'f>(
    fleet: &'f Fleet,
    dir: &Path,
    target: &Target,
) -> Result<&'f Worktree> {
    match target {
        Target::Name(name) => worktree::find_by_name(fleet, name),
        Target::Path(path) => find_by_path(fleet, &dir.join(path)),
    }
}

/// The worktree at `wanted`, compared through symlinks. Refuses the primary checkout and a path
/// that is no worktree of the fleet.
fn find_by_path<'f>(fleet: &'f Fleet, wanted: &Path) -> Result<&'f Worktree> {
    let primary = &fleet.primary.path;
    if fs::same_path(primary, wanted) {
        bail!(
            "{} is the primary checkout, which is never a worktree",
            primary.display()
        );
    }
    match fleet
        .worktrees
        .iter()
        .find(|worktree| fs::same_path(&worktree.path, wanted))
    {
        Some(worktree) => Ok(worktree),
        None => bail!("{} is not a worktree of this repository", wanted.display()),
    }
}

/// Takes the lock of the worktree `name` at `path` without waiting. Fails with the typed error
/// `busy` when another process holds it: a setup or another orca-term command is running on it.
pub(crate) fn lock_worktree(name: &str, path: &Path) -> Result<WorktreeLock> {
    let admin = git::admin_dir(path)?;
    match fs::try_lock_worktree(&admin)? {
        Some(lock) => Ok(lock),
        None => Err(TypedError::Busy {
            name: name.to_owned(),
            path: path.to_owned(),
            reason: BusyReason::Locked,
        }
        .into()),
    }
}

/// Writes the worktree's `worktree.json` with its setup recorded as `setup`, keeping the agent it
/// records, and refusing to overwrite a newer schema. Taking `lock` proves the worktree's lock is
/// held.
pub(crate) fn write_state(lock: &WorktreeLock, setup: Option<SetupRecord>) -> Result<()> {
    let agent = read_recorded(lock.admin())?.agent;
    write_file(lock, agent.as_deref(), setup)
}

/// Writes the worktree's `worktree.json` with `agent` as its agent, keeping the setup it records.
pub(crate) fn write_agent(lock: &WorktreeLock, agent: Agent) -> Result<()> {
    let setup = read_recorded(lock.admin())?.setup;
    write_file(lock, Some(agent.name()), setup)
}

fn write_file(lock: &WorktreeLock, agent: Option<&str>, setup: Option<SetupRecord>) -> Result<()> {
    let path = lock.admin().state_file();
    state::check_overwrite(&path, fs::read_optional(&path)?.as_deref())?;
    fs::write_atomically(&path, &WorktreeFile::new(agent, setup).to_bytes()?)
}

/// Reads every config file that exists, highest precedence first, with the committed file taken
/// from the root of `checkout`.
pub(crate) fn config_layers(
    primary: &PrimaryCheckout,
    checkout: &Path,
    env: &Env,
) -> Result<Vec<Layer>> {
    let common_dir = git::common_dir(primary)?;
    let mut layers = Vec::new();
    for source in config::sources(primary, checkout, &common_dir, env) {
        if let Some(text) = fs::read_text_optional(&source.path)? {
            layers.push(Layer::parse(source, &text)?);
        }
    }
    Ok(layers)
}

/// Runs the setup script of the worktree at `worktree`, read from the worktree's own config, and
/// records it in `worktree.json`: running before it starts, then how it ended. Its output goes to
/// the setup log, overwritten, and to `echo`. With no script configured, records that there is
/// no setup. A failed script is not an error: it leaves the worktree not ready, and nothing is
/// undone.
pub(crate) fn set_up(
    lock: &WorktreeLock,
    primary: &PrimaryCheckout,
    worktree: &Path,
    echo: &mut dyn Write,
) -> Result<SetupReport> {
    let layers = config_layers(primary, worktree, &env::config_env())?;
    let Some(script) = config::resolve_setup(&layers) else {
        write_state(lock, None)?;
        let derived = derive_state(lock.admin())?;
        return Ok(SetupReport {
            outcome: SetupOutcome::NoScript,
            script: None,
            log: None,
            state: derived.state,
            agent: derived.agent,
        });
    };
    write_state(lock, Some(SetupRecord::Running))?;
    let log_path = lock.admin().setup_log();
    let mut log = fs::create_truncated(&log_path)?;
    let vars = setup::environment(&primary.path, worktree);
    let outcome = script::run_setup(&script.value, worktree, &vars, &mut log, echo)?;
    write_state(lock, outcome.record())?;
    let derived = derive_state(lock.admin())?;
    Ok(SetupReport {
        outcome,
        script: Some(script),
        log: Some(log_path),
        state: derived.state,
        agent: derived.agent,
    })
}
