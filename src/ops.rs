//! Ops: one module per command, each a short sequence of adapter and domain calls. This file holds
//! what several commands share: reading the fleet, finding the worktree a command names, locking
//! a worktree, writing its state, probing its holder, running its setup and telling the running
//! Neovims that a worktree changed.

pub(crate) mod agent_attach;
pub(crate) mod agent_holder;
pub(crate) mod agent_start;
pub(crate) mod agent_stop;
pub(crate) mod hook;
pub(crate) mod nvim_register;
pub(crate) mod nvim_unregister;
pub(crate) mod worktree_ls;
pub(crate) mod worktree_new;
pub(crate) mod worktree_prune;
pub(crate) mod worktree_rm;
pub(crate) mod worktree_seen;
pub(crate) mod worktree_setup;

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::adapters::fs::{self, WorktreeLock};
use crate::adapters::nvim::Sent;
use crate::adapters::{self, env, git, script};
use crate::domain::agent::{Agent, HolderStatus};
use crate::domain::config::{self, Env, Layer};
use crate::domain::error::{BusyReason, TypedError};
use crate::domain::fleet::{Fleet, PrimaryCheckout, Worktree};
use crate::domain::nvim::{self, Address};
use crate::domain::seen::{self, Seen};
use crate::domain::setup::{self, SetupOutcome, SetupRecord, SetupReport};
use crate::domain::state::{self, AdminDir, BrokenReason, LockStatus, State, WorktreeFile};
use crate::domain::turn::{self, Status};
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
        worktree.activity = derived.activity;
        worktree.agent = derived.agent;
        worktree.holder = derived.holder;
        worktree.unseen = derived.unseen;
    }
    Ok(fleet)
}

/// A worktree's state and the facts beside it, derived on read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Derived {
    pub(crate) state: State,
    /// What the agent is doing or said last, beside a state from its hooks.
    pub(crate) activity: Option<String>,
    /// The agent recorded in `worktree.json`, by name.
    pub(crate) agent: Option<String>,
    /// Whether a holder is alive right now.
    pub(crate) holder: HolderStatus,
    /// Whether the worktree is unseen (`domain/seen.rs`).
    pub(crate) unseen: bool,
}

impl Derived {
    fn broken(reason: BrokenReason) -> Self {
        Self {
            state: State::Broken(reason),
            activity: None,
            agent: None,
            holder: HolderStatus::Gone,
            unseen: false,
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

/// The state of the worktree whose admin directory is `admin`, and whether it is unseen. Only
/// reads.
pub(crate) fn derive_state(admin: &AdminDir) -> Result<Derived> {
    let mut derived = derive_agent_state(admin)?;
    derived.unseen = read_unseen(admin, derived.holder);
    Ok(derived)
}

/// Whether the worktree whose admin directory is `admin`, whose holder is `holder`, is unseen:
/// from what the person has seen (`seen.json`) and the hooks' live status. Only reads, and never
/// fails: a file that cannot be used reads as nothing seen, or no turn ended.
fn read_unseen(admin: &AdminDir, holder: HolderStatus) -> bool {
    let ended = seen::latest_ended(read_status(admin).as_ref(), holder);
    seen::is_unseen(read_seen(admin), ended)
}

/// What the person has seen of the worktree whose admin directory is `admin`, or nothing seen and
/// no mark when `seen.json` is missing, unreadable, corrupt or of a newer schema.
fn read_seen(admin: &AdminDir) -> Seen {
    let bytes = fs::read_optional(&admin.seen_file()).ok().flatten();
    seen::read(bytes.as_deref()).usable()
}

/// The state of the worktree whose admin directory is `admin`, from its state file, the hooks'
/// live status and the volatile facts behind them; not yet whether it is unseen. Only reads: the
/// worktree's lock is probed only for a setup recorded as running, the holder's lock is probed and
/// released at once, the setup log is read only for a failed setup, and the live status only for a
/// ready worktree with an agent.
fn derive_agent_state(admin: &AdminDir) -> Result<Derived> {
    let mut recorded = read_recorded(admin)?;
    if recorded.setup == Some(SetupRecord::Running) {
        if fs::lock_status(admin)? == LockStatus::Held {
            return Ok(Derived {
                state: state::setup_running(LockStatus::Held),
                activity: None,
                agent: recorded.agent,
                holder: HolderStatus::Gone,
                unseen: false,
            });
        }
        // The lock is free, but the setup may have ended between the read and the probe: it
        // records its end before it releases the lock, so reading the file once more tells a
        // finished setup from a dead one. Still only a read.
        recorded = read_recorded(admin)?;
    }
    let holder = HolderStatus::from_lock(fs::lock_status_at(&admin.agent_lock())?);
    let (state, activity) = match recorded.setup {
        // Still recorded as running, with its lock seen free: the process died.
        Some(SetupRecord::Running) => (state::setup_running(LockStatus::Free), None),
        Some(SetupRecord::Failed(exit)) => {
            let log = fs::read_optional(&admin.setup_log())?.unwrap_or_default();
            (state::setup_failed(exit, &log), None)
        }
        Some(SetupRecord::Succeeded) | None => {
            let status = recorded.agent.as_ref().and_then(|_| read_status(admin));
            state::ready(recorded.agent.as_deref(), status.as_ref(), holder)
        }
    };
    Ok(Derived {
        state,
        activity,
        agent: recorded.agent,
        holder,
        unseen: false,
    })
}

/// The hooks' live status in `admin`, or `None` when there is none this core can use: missing,
/// unreadable, corrupt or of a newer schema. It never fails the fleet; the next hook rewrites a
/// corrupt one, saying so in `hook.log`.
pub(crate) fn read_status(admin: &AdminDir) -> Option<Status> {
    let bytes = fs::read_optional(&admin.status_file()).ok().flatten();
    turn::read_status(bytes.as_deref()).usable()
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
        notify_changed(lock.admin());
        let derived = derive_state(lock.admin())?;
        return Ok(SetupReport {
            outcome: SetupOutcome::NoScript,
            script: None,
            log: None,
            state: derived.state,
            activity: derived.activity,
            agent: derived.agent,
            unseen: derived.unseen,
        });
    };
    write_state(lock, Some(SetupRecord::Running))?;
    notify_changed(lock.admin());
    let log_path = lock.admin().setup_log();
    let mut log = fs::create_truncated(&log_path)?;
    let vars = setup::environment(&primary.path, worktree);
    let outcome = script::run_setup(&script.value, worktree, &vars, &mut log, echo)?;
    write_state(lock, outcome.record())?;
    notify_changed(lock.admin());
    let derived = derive_state(lock.admin())?;
    Ok(SetupReport {
        outcome,
        script: Some(script),
        log: Some(log_path),
        state: derived.state,
        activity: derived.activity,
        agent: derived.agent,
        unseen: derived.unseen,
    })
}

/// The registry of running Neovims of the repository containing the current directory, in its
/// git common dir: the same from the primary checkout and from every worktree.
pub(crate) fn nvim_registry() -> Result<PathBuf> {
    let cwd = env::current_dir()?;
    let primary = git::primary_checkout(&cwd)?;
    Ok(nvim::registry_dir(&git::common_dir(&primary)?))
}

/// A worktree whose state was just written, as the running Neovims are told about it: the git
/// common dir of its repository, which holds their registry, and the worktree's path as
/// `worktree ls` prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Changed {
    common: PathBuf,
    worktree: PathBuf,
}

impl Changed {
    /// The worktree whose admin directory is `admin`, found from git's own files beside it
    /// (`commondir` and `gitdir`) without running git, so that a hook stays fast. `None` when
    /// they cannot be read: then nobody is told, which is never an error.
    pub(crate) fn of(admin: &AdminDir) -> Option<Self> {
        let commondir = admin.commondir_file()?;
        let gitdir = admin.gitdir_file()?;
        let commondir_text = fs::read_text_optional(&commondir).ok().flatten()?;
        let gitdir_text = fs::read_text_optional(&gitdir).ok().flatten()?;
        Some(Self {
            common: state::common_dir_from(&commondir, &commondir_text)?,
            worktree: state::worktree_path_from(&gitdir, &gitdir_text)?,
        })
    }

    /// Tells every Neovim registered in the repository that this worktree changed, firing
    /// `User OrcaTermWorktreeChanged` there with its path. A Neovim that is gone has its file
    /// removed from the registry; any other failure is skipped. It never fails, and all the
    /// Neovims together take a bounded time (`adapters/nvim.rs`), so the write that came before is
    /// never held up.
    pub(crate) fn notify(&self) {
        let registry = nvim::registry_dir(&self.common);
        let Ok(names) = fs::list_file_names(&registry) else {
            return;
        };
        // Each registry file, with the Neovim address it holds.
        let mut entries = Vec::new();
        let mut sockets = Vec::new();
        for name in names.iter().filter(|name| nvim::is_entry_name(name)) {
            let entry = registry.join(name);
            let address = fs::read_optional(&entry)
                .ok()
                .flatten()
                .and_then(|bytes| Address::from_entry(&bytes));
            if let Some(address) = address {
                sockets.push(address.as_path().to_owned());
                entries.push(entry);
            }
        }
        let message = nvim::worktree_changed(&self.worktree.to_string_lossy());
        let sent = adapters::nvim::send_all(&sockets, &message);
        for (entry, sent) in entries.iter().zip(sent) {
            if sent == Sent::Gone {
                let _ = fs::remove_if_exists(entry);
            }
        }
    }
}

/// Tells the running Neovims that the worktree whose admin directory is `admin` changed: what
/// every command does after it writes a worktree's state. Never fails.
pub(crate) fn notify_changed(admin: &AdminDir) {
    if let Some(changed) = Changed::of(admin) {
        changed.notify();
    }
}
