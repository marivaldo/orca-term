//! Ops: one module per command, each a short sequence of adapter and domain calls. This file holds
//! what several commands share: reading the fleet, finding the worktree a command names, locking
//! a worktree, writing its state, probing its holder, running its setup, reading and changing its
//! notes, reading its turns and its sessions, changing its current session, telling the running Neovims that a worktree changed and checking the
//! programs a command needs (git, the agents).

pub(crate) mod agent_attach;
pub(crate) mod agent_holder;
pub(crate) mod agent_start;
pub(crate) mod agent_stop;
pub(crate) mod agent_swap;
pub(crate) mod doctor;
pub(crate) mod hook;
pub(crate) mod note_add;
pub(crate) mod note_edit;
pub(crate) mod note_ls;
pub(crate) mod note_rm;
pub(crate) mod note_send;
pub(crate) mod nvim_register;
pub(crate) mod nvim_unregister;
pub(crate) mod session_ls;
pub(crate) mod session_new;
pub(crate) mod session_resume;
pub(crate) mod turn_blame;
pub(crate) mod turn_diff;
pub(crate) mod turn_ls;
pub(crate) mod worktree_detail;
pub(crate) mod worktree_ls;
pub(crate) mod worktree_new;
pub(crate) mod worktree_prune;
pub(crate) mod worktree_review;
pub(crate) mod worktree_rm;
pub(crate) mod worktree_seen;
pub(crate) mod worktree_setup;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};

use crate::adapters::fs::{self, FileLock, WorktreeLock};
use crate::adapters::nvim::Sent;
use crate::adapters::{self, env, git, program, script};
use crate::domain::agent::{Agent, HolderStatus};
use crate::domain::config::{self, Env, Layer};
use crate::domain::error::{BusyReason, TypedError};
use crate::domain::fleet::{Fleet, PrimaryCheckout, Worktree};
use crate::domain::history::{self, History};
use crate::domain::note::translate;
use crate::domain::note::{self, Anchor, BlobId, Note, Notes, PendingNote};
use crate::domain::nvim::{self, Address};
use crate::domain::operation::{self, Operation, RebaseStep};
use crate::domain::prerequisite::{self, Check, Probe, Requirement};
use crate::domain::seen::{self, Seen};
use crate::domain::session::{self, Session};
use crate::domain::setup::{self, SetupOutcome, SetupRecord, SetupReport};
use crate::domain::state::{self, AdminDir, BrokenReason, LockStatus, State, WorktreeFile};
use crate::domain::trust::{self, Consent, Gate, Kind, Subject, Unapproved};
use crate::domain::turn::{self, Status, StatusRead};
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
        worktree.notes = derived.notes;
        worktree.denials = derived.denials;
        worktree.operation = derived.operation;
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
    /// How many notes are pending on it (`domain/note.rs`).
    pub(crate) notes: usize,
    /// How many tool calls its agent was denied in its latest turn (`domain/turn.rs`).
    pub(crate) denials: usize,
    /// The git operation it is in the middle of (`domain/operation.rs`).
    pub(crate) operation: Option<Operation>,
}

impl Derived {
    fn broken(reason: BrokenReason) -> Self {
        Self {
            state: State::Broken(reason),
            activity: None,
            agent: None,
            holder: HolderStatus::Gone,
            unseen: false,
            notes: 0,
            denials: 0,
            operation: None,
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

/// The state of the worktree whose admin directory is `admin`, whether it is unseen, how many
/// notes are pending on it, how many tool calls its agent was denied in its latest turn and the
/// git operation it is in the middle of. Only reads.
pub(crate) fn derive_state(admin: &AdminDir) -> Result<Derived> {
    let mut derived = derive_agent_state(admin)?;
    derived.unseen = read_unseen(admin, derived.holder);
    derived.notes = read_note_count(admin);
    derived.denials = read_denial_count(admin);
    derived.operation = read_operation(admin);
    Ok(derived)
}

/// The git operation (a merge, a rebase, a cherry-pick or a revert) the worktree whose admin
/// directory is `admin` is in the middle of, from the markers git leaves in the worktree's git
/// dir, which holds `admin`. Only looks at which files are there: it runs no git and never fails.
fn read_operation(admin: &AdminDir) -> Option<Operation> {
    let git_dir = admin.git_dir()?;
    operation::detect(|marker| fs::exists(&git_dir.join(marker)))
}

/// The step the rebase in progress in the worktree whose admin directory is `admin` is at, from
/// git's step files in `rebase-merge/` (or `rebase-apply/`). `None` when no rebase is in progress
/// or git records no step. Only reads, and never fails.
pub(crate) fn read_rebase_step(admin: &AdminDir) -> Option<RebaseStep> {
    let git_dir = admin.git_dir()?;
    let (dir, (current, total)) = if fs::is_dir(&git_dir.join(operation::REBASE_MERGE_DIR)) {
        (
            git_dir.join(operation::REBASE_MERGE_DIR),
            operation::REBASE_MERGE_STEP_FILES,
        )
    } else {
        (
            git_dir.join(operation::REBASE_APPLY_DIR),
            operation::REBASE_APPLY_STEP_FILES,
        )
    };
    let read = |name: &str| fs::read_text_optional(&dir.join(name)).ok().flatten();
    RebaseStep::from_texts(&read(current)?, &read(total)?)
}

/// How many tool calls the agent of the worktree whose admin directory is `admin` was denied in
/// its latest turn, open or ended, from the hooks' live status. Only reads, and never fails: a
/// `status.json` that cannot be used counts none. It never changes the state.
fn read_denial_count(admin: &AdminDir) -> usize {
    read_status(admin).map_or(0, |status| status.latest_denials().len())
}

/// How many notes are pending on the worktree whose admin directory is `admin`. Only reads, and
/// never fails: a `notes.json` that cannot be used counts none.
fn read_note_count(admin: &AdminDir) -> usize {
    let bytes = fs::read_optional(&admin.notes_file()).ok().flatten();
    note::read(bytes.as_deref()).count()
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
                notes: 0,
                denials: 0,
                operation: None,
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
        Some(SetupRecord::NotApproved) => {
            (state::setup_not_approved(&worktree_name_of(admin)), None)
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
        notes: 0,
        denials: 0,
        operation: None,
    })
}

/// The name of the worktree whose admin directory is `admin`, as the fleet names it: its
/// directory's basename, from git's `gitdir` file; git's id for it when that cannot be read.
fn worktree_name_of(admin: &AdminDir) -> String {
    worktree_of(admin)
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .or_else(|| admin.worktree_id().map(str::to_owned))
        .unwrap_or_default()
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

/// How a command gets the person's approval of what a committed `orca-term.yaml` asks for, a
/// setup script or the sandbox's writable paths (`domain/trust.rs`).
pub(crate) enum Approval<'a> {
    /// Given beforehand, with `--approve-setup` or `--approve-sandbox`.
    Given,
    /// Asked of the person at the terminal through this function, which shows the script or the
    /// paths and says whether the person approved them.
    Ask(&'a mut dyn FnMut(&Unapproved) -> Result<bool>),
    /// Nobody can be asked: no terminal, or `--json`.
    Unavailable,
}

impl Approval<'_> {
    fn consent(&self) -> Consent {
        match self {
            Self::Given => Consent::Given,
            Self::Ask(_) => Consent::Ask,
            Self::Unavailable => Consent::Unavailable,
        }
    }
}

/// Runs the setup script of the worktree `name` at `worktree`, read from the worktree's own
/// config, and records it in `worktree.json`: running before it starts, then how it ended. Its
/// output goes to the setup log, overwritten, and to `echo`. With no script configured, records
/// that there is no setup. A failed script is not an error: it leaves the worktree not ready, and
/// nothing is undone.
///
/// A script from a committed `orca-term.yaml` runs only once the person approved its text, through
/// `approval`. When it is not approved, it does not run: the worktree is recorded not ready and
/// the typed error `untrusted` is returned.
pub(crate) fn set_up(
    lock: &WorktreeLock,
    primary: &PrimaryCheckout,
    worktree: &Path,
    name: &str,
    approval: &mut Approval<'_>,
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
            notes: derived.notes,
            denials: derived.denials,
            operation: derived.operation,
        });
    };
    if let Some(file) = config::committed_setup_file(&layers) {
        let unapproved = Unapproved {
            name: name.to_owned(),
            subject: Subject::Setup {
                script: script.value.clone(),
            },
            file: fs::canonicalize(file)?,
        };
        if !is_approved(primary, worktree, &unapproved, approval)? {
            write_state(lock, Some(SetupRecord::NotApproved))?;
            notify_changed(lock.admin());
            return Err(unapproved.refused().into());
        }
    }
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
        notes: derived.notes,
        denials: derived.denials,
        operation: derived.operation,
    })
}

/// How long a command waits for another one to finish recording an approval.
const TRUST_LOCK_WAIT: Duration = Duration::from_secs(2);

/// Whether what the committed file of the worktree at `worktree` asks for (`unapproved`: its setup
/// script or its writable paths) may be used: it was approved before in this repository, or it is
/// approved now through `approval` (given beforehand, or asked at the terminal), which records the
/// approval.
pub(crate) fn is_approved(
    primary: &PrimaryCheckout,
    worktree: &Path,
    unapproved: &Unapproved,
    approval: &mut Approval<'_>,
) -> Result<bool> {
    let common = git::common_dir(primary)?;
    let kind = unapproved.subject.kind();
    let hash = unapproved.subject.sha256();
    let file = trust::trust_file(&common);
    let approved = trust::read(fs::read_optional(&file)?.as_deref())?.is_approved(kind, &hash);
    let approve = match trust::gate(true, approved, approval.consent()) {
        Gate::Run => return Ok(true),
        Gate::Refuse => return Ok(false),
        Gate::ApproveAndRun => true,
        Gate::Ask => match approval {
            Approval::Ask(ask) => ask(unapproved)?,
            Approval::Given | Approval::Unavailable => false,
        },
    };
    if approve {
        let checkout = fs::canonicalize(worktree)?;
        let source = unapproved
            .file
            .strip_prefix(&checkout)
            .unwrap_or(&unapproved.file);
        record_approval(&common, kind, &hash, &source.display().to_string())?;
    }
    Ok(approve)
}

/// Records, in the approvals' file of the repository whose git common dir is `common`, that the
/// script or list of `kind` whose sha256 is `hash`, from `source`, is approved. Holds the
/// approvals' lock from the read to the write (waiting for another command at most
/// [`TRUST_LOCK_WAIT`]), replaces the file whole, and never writes over a newer schema.
fn record_approval(common: &Path, kind: Kind, hash: &str, source: &str) -> Result<()> {
    let _lock = lock_trust(common)?;
    let file = trust::trust_file(common);
    let mut approvals = trust::read(fs::read_optional(&file)?.as_deref())?;
    approvals.approve(kind, hash, source, env::now_millis());
    fs::write_atomically(&file, &approvals.to_bytes()?)
}

/// Writes an approvals' file approving nothing in the git common dir `common` when there is none,
/// under the approvals' lock. opencode's sandbox on Linux keeps the agent from writing a file only
/// when the file exists (#35): one the agent made itself would be read as approvals.
pub(crate) fn ensure_trust_file(common: &Path) -> Result<()> {
    let file = trust::trust_file(common);
    if fs::exists(&file) {
        return Ok(());
    }
    let _lock = lock_trust(common)?;
    if !fs::exists(&file) {
        fs::write_atomically(&file, &trust::Trust::default().to_bytes()?)?;
    }
    Ok(())
}

/// Takes the approvals' lock of the git common dir `common`, waiting for another command at most
/// [`TRUST_LOCK_WAIT`].
fn lock_trust(common: &Path) -> Result<FileLock> {
    let lock = trust::trust_lock(common);
    fs::create_dir_all(lock.parent().unwrap_or(common))?;
    fs::lock_file_waiting(&lock, TRUST_LOCK_WAIT)?.ok_or_else(|| {
        anyhow!("another orca-term command is recording an approval; try again once it finishes")
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
        let commondir_text = fs::read_text_optional(&commondir).ok().flatten()?;
        Some(Self {
            common: state::common_dir_from(&commondir, &commondir_text)?,
            worktree: worktree_of(admin)?,
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

/// The path of the worktree whose admin directory is `admin`, as `worktree ls` prints it, from
/// git's `gitdir` file beside it, without running git. `None` when it cannot be read.
pub(crate) fn worktree_of(admin: &AdminDir) -> Option<PathBuf> {
    let gitdir = admin.gitdir_file()?;
    let gitdir_text = fs::read_text_optional(&gitdir).ok().flatten()?;
    state::worktree_path_from(&gitdir, &gitdir_text)
}

/// Tells the running Neovims that the worktree whose admin directory is `admin` changed: what
/// every command does after it writes a worktree's state. Never fails.
pub(crate) fn notify_changed(admin: &AdminDir) {
    if let Some(changed) = Changed::of(admin) {
        changed.notify();
    }
}

/// How long a note command waits for another one to finish changing the same worktree's notes.
const NOTES_LOCK_WAIT: Duration = Duration::from_secs(2);

/// The worktree `target` names or locates in the repository containing the current directory,
/// for a note command, with its admin directory. Refuses the primary checkout, an unknown worktree
/// and one whose directory is gone.
pub(crate) fn find_noted(target: &Target) -> Result<(Worktree, AdminDir)> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = find_target(&fleet, &cwd, target)?;
    note::check_can_note(worktree)?;
    let admin = git::admin_dir(&worktree.path)?;
    Ok((worktree.clone(), admin))
}

/// The pending notes in `admin`, none when `notes.json` does not exist. Refuses a file this core
/// cannot use. Only reads: `notes.json` is only ever replaced whole, so no lock is needed.
pub(crate) fn read_notes(admin: &AdminDir) -> Result<Notes> {
    let bytes = fs::read_optional(&admin.notes_file())?;
    note::read(bytes.as_deref()).usable()
}

/// Changes the pending notes of the worktree `name`, whose admin directory is `admin`, through
/// `change`, and returns what it returns. Holds `notes.lock` from the read to the write (waiting
/// for another note command at most [`NOTES_LOCK_WAIT`]), replaces `notes.json` whole, then
/// removes the kept contents no note refers to any more. Nothing is written when `change` fails.
pub(crate) fn change_notes<T>(
    admin: &AdminDir,
    name: &str,
    change: impl FnOnce(&mut Notes) -> Result<T>,
) -> Result<T> {
    // A worktree made by hand with git has no admin directory of ours yet.
    fs::create_dir_all(admin.path())?;
    let _lock = fs::lock_file_waiting(&admin.notes_lock(), NOTES_LOCK_WAIT)?.ok_or_else(|| {
        anyhow!(
            "another orca-term command is changing the notes of worktree {name}; try again once \
             it finishes"
        )
    })?;
    let mut notes = read_notes(admin)?;
    let result = change(&mut notes)?;
    fs::write_atomically(&admin.notes_file(), &note::to_bytes(&notes)?)?;
    remove_unused_copies(admin, &notes)?;
    Ok(result)
}

/// Removes every kept content in `admin` that none of `notes` refers to. Only files named by a
/// blob id are touched.
fn remove_unused_copies(admin: &AdminDir, notes: &Notes) -> Result<()> {
    let in_use = notes.copies_in_use();
    let dir = admin.note_copies();
    for name in fs::list_file_names(&dir)? {
        if let Ok(blob) = BlobId::parse(&name)
            && !in_use.contains(&blob)
        {
            fs::remove_if_exists(&dir.join(name))?;
        }
    }
    Ok(())
}

/// Keeps `content`, a file of the worktree at `worktree` as it is now, in `admin` under its git
/// blob id, once whatever number of notes refer to it, and returns that id. Call it under
/// `notes.lock`, so a copy is never removed before the note referring to it is written.
pub(crate) fn keep_copy(worktree: &Path, admin: &AdminDir, content: &[u8]) -> Result<BlobId> {
    let blob = git::blob_id(worktree, content)?;
    let path = admin.note_copies().join(blob.as_str());
    if !fs::is_file(&path) {
        fs::create_dir_all(&admin.note_copies())?;
        fs::write_atomically(&path, content)?;
    }
    Ok(blob)
}

/// `note` with its anchor translated to the file of the worktree at `worktree` as it is now.
pub(crate) fn pending(worktree: &Path, admin: &AdminDir, note: &Note) -> Result<PendingNote> {
    Ok(PendingNote {
        current: current_anchor(worktree, admin, note)?,
        note: note.clone(),
    })
}

/// Where `note`'s anchor is in the file of the worktree at `worktree` now: its kept content is
/// compared with the file on disk, and the lines mapped over the differences
/// (`domain/note/translate.rs`). A note whose file is gone, or whose kept content is, is on the
/// whole file.
fn current_anchor(worktree: &Path, admin: &AdminDir, note: &Note) -> Result<Anchor> {
    if !note.anchor.is_on_lines() {
        return Ok(Anchor::File);
    }
    let Some(copy) = &note.copy else {
        return Ok(Anchor::File);
    };
    let file = note.path.in_worktree(worktree);
    let kept = admin.note_copies().join(copy.as_str());
    if !fs::is_file(&file) || !fs::is_file(&kept) {
        return Ok(Anchor::File);
    }
    let diff = git::diff_files(worktree, &kept, &file)?;
    let hunks = translate::parse_hunks(&String::from_utf8_lossy(&diff))
        .with_context(|| format!("cannot translate note {} on {}", note.id, note.path))?;
    Ok(translate::translate(note.anchor, &hunks))
}

/// The worktree `target` names or locates in the repository containing the current directory,
/// for a turn command, with its admin directory. Refuses the primary checkout, an unknown worktree
/// and one whose directory is gone.
pub(crate) fn find_turned(target: &Target) -> Result<(Worktree, AdminDir)> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let worktree = find_target(&fleet, &cwd, target)?;
    if let State::Broken(reason) = &worktree.state
        && reason.is_gone()
    {
        bail!(
            "worktree {} at {} is broken ({reason}): its directory is gone, and its turns with it",
            worktree.name,
            worktree.path.display(),
        );
    }
    let admin = git::admin_dir(&worktree.path)?;
    Ok((worktree.clone(), admin))
}

/// The turns of `worktree`, whose admin directory is `admin`, from its turn log and its live
/// status. Only reads, and takes no lock: the hook only ever appends to the log, and replaces
/// the status whole.
pub(crate) fn read_history(worktree: &Worktree, admin: &AdminDir) -> Result<History> {
    let log = fs::read_optional(&admin.turn_log())?.unwrap_or_default();
    let records = turn::read_log(&log)?;
    let status = read_status(admin);
    Ok(History {
        name: worktree.name.clone(),
        path: worktree.path.clone(),
        turns: history::turns(&records, status.as_ref()),
    })
}

/// The sessions of the worktree whose admin directory is `admin`, the most recently active first,
/// from its turn log and its live status (`domain/session.rs`). Only reads, and takes no lock, as
/// [`read_history`].
pub(crate) fn read_sessions(admin: &AdminDir) -> Result<Vec<Session>> {
    let log = fs::read_optional(&admin.turn_log())?.unwrap_or_default();
    let records = turn::read_log(&log)?;
    let status = read_status(admin);
    let turns = history::turns(&records, status.as_ref());
    Ok(session::sessions(&turns, status.as_ref()))
}

/// How long a session command waits for a hook of the same worktree to finish.
const HOOK_LOCK_WAIT: Duration = Duration::from_secs(1);

/// Changes the live status of the worktree `name`, whose admin directory is `admin`, through
/// `change`: what `session resume` and `session new` do to move its current session (#41), while
/// no agent runs there. Holds `hook.lock`, as a hook does, from the read to the write (waiting at
/// most [`HOOK_LOCK_WAIT`]), replaces `status.json` whole, and never writes over a newer schema or
/// a file it cannot read.
pub(crate) fn change_status(
    admin: &AdminDir,
    name: &str,
    change: impl FnOnce(Status) -> Status,
) -> Result<()> {
    let _lock = fs::lock_file_waiting(&admin.hook_lock(), HOOK_LOCK_WAIT)?.ok_or_else(|| {
        anyhow!("a hook of worktree {name} is still recording an event; try again once it finishes")
    })?;
    let read = turn::read_status(fs::read_optional(&admin.status_file())?.as_deref());
    turn::check_status_overwrite(&read)?;
    if let StatusRead::Unreadable(reason) = &read {
        bail!("status.json of worktree {name} cannot be read ({reason})");
    }
    // A missing file is a status with nothing in it yet.
    let status = read.usable().unwrap_or_default();
    fs::write_atomically(&admin.status_file(), &turn::status_bytes(&change(status))?)
}

/// The check of the program `requirement` names, as a shell would find it on `path_dirs`: not
/// found, or what its `--version` printed. Returns the program's path too, when found.
pub(crate) fn check_program(
    path_dirs: &[PathBuf],
    requirement: &Requirement,
) -> (Option<PathBuf>, Check) {
    let path = fs::executable_in(path_dirs, requirement.program);
    let probe = match &path {
        None => Probe::NotFound,
        Some(path) => match program::version(path) {
            Ok(printed) => Probe::Printed(printed),
            Err(err) => Probe::Failed(format!("{err:#}")),
        },
    };
    (path, prerequisite::check_program(requirement, &probe))
}

/// Refuses, as the typed `prerequisite`, when git is not on PATH or older than orca-term needs.
/// Every command that runs git calls it once, as it starts, before its first git call.
pub(crate) fn check_git() -> Result<()> {
    let (_, check) = check_program(&env::path_dirs(), &prerequisite::GIT);
    prerequisite::require(&check)
}

/// The path of `agent`'s program on `path_dirs`, refusing as the typed `prerequisite` when it is
/// not there or older than orca-term needs. One newer than the tested version is not refused.
pub(crate) fn find_agent(path_dirs: &[PathBuf], agent: Agent) -> Result<PathBuf> {
    let (path, check) = check_program(path_dirs, &prerequisite::agent_requirement(agent));
    prerequisite::require(&check)?;
    path.with_context(|| format!("{} is not on PATH", agent.program()))
}
