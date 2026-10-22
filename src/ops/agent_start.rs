//! Ops: `agent start`, which launches a worktree's agent inside a detached holder (ADR 0009),
//! sandboxed with everything allowed inside the worktree (ADR 0007). claude runs under its native
//! sandbox, with hooks that call this orca-term back on every turn event; opencode runs inside the
//! sandbox orca-term wraps it in, with a plugin that reports its events through the holder (#35).
//! The agent is the one asked for, else the one the worktree recorded, else claude-code, and
//! starting records it. Starting an agent that already runs is not an error: the report says so.
//! With no holder alive, the agent continues the current session `status.json` records when that
//! agent recorded it, unless the person asks for a fresh one.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde_json::Value;

use crate::adapters::fs::{self, WorktreeLock};
use crate::adapters::holder::{self, HolderArgs};
use crate::adapters::{env, git, terminal};
use crate::domain::agent::{Agent, HolderRecord, Size, StartMode, StartOutcome, Started};
use crate::domain::config::{self, Layer};
use crate::domain::error::{BusyReason, TypedError};
use crate::domain::fleet::{Fleet, PrimaryCheckout, Worktree};
use crate::domain::launch::{self, SessionId, opencode};
use crate::domain::sandbox::policy::{self, Policy};
use crate::domain::sandbox::{self, Platform};
use crate::domain::state::{ADMIN_DIR_NAME, AdminDir, LockStatus};
use crate::domain::trust::{self, Subject, Unapproved};
use crate::domain::turn;
use crate::domain::worktree::{self, Target};
use crate::ops::{
    Approval, config_layers, derive_state, ensure_trust_file, find_agent, find_target_admin,
    is_approved, lock_worktree, notify_changed, read_fleet, write_agent,
};

/// What `agent start` is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Wanted {
    /// The agent to start (`--agent`); `None` for the one the worktree recorded, else claude-code.
    pub(crate) agent: Option<Agent>,
    /// The agent's first terminal size, where given (else the starter's terminal size, else 24 by
    /// 80).
    pub(crate) rows: Option<u16>,
    pub(crate) cols: Option<u16>,
    /// Which session it starts on.
    pub(crate) mode: StartMode,
}

/// Starts the agent `wanted` names in the worktree `target` names or locates in the repository
/// containing the current directory. Fails as `busy` when the worktree's lock is held or another
/// agent runs there, as `no_sandbox` when the agent's sandbox cannot run here, as `prerequisite`
/// when the agent is not on PATH or older than orca-term needs, and as `session_gone` when the
/// recorded session can no longer be resumed. Extra writable paths from a committed
/// `orca-term.yaml` not approved through `approval` start nothing: the typed error `untrusted`.
pub(crate) fn run(
    target: &Target,
    wanted: &Wanted,
    approval: &mut Approval<'_>,
) -> Result<Started> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let (worktree, admin) = find_target_admin(&fleet, &cwd, target)?;
    worktree::check_can_run_agent(worktree)?;
    let lock = lock_worktree(&worktree.name, &worktree.path)?;
    start(&fleet, worktree, &admin, &lock, wanted, approval)
}

/// Starts the agent `wanted` names in `worktree` of `fleet`, whose admin directory is `admin` and
/// whose lock is `lock`, and records it: what `agent start` and `agent swap` share.
pub(crate) fn start(
    fleet: &Fleet,
    worktree: &Worktree,
    admin: &AdminDir,
    lock: &WorktreeLock,
    wanted: &Wanted,
    approval: &mut Approval<'_>,
) -> Result<Started> {
    let agent = chosen_agent(worktree, wanted.agent);
    if let Some(running) = already_running(worktree, admin, agent)? {
        return Ok(running);
    }
    let resumed = session_to_resume(&worktree.name, admin, agent, wanted.mode)?;

    let path_dirs = env::path_dirs();
    sandbox::check_available(Platform::current(), agent, &|tool| {
        fs::executable_in(&path_dirs, tool).is_some()
    })?;
    let program = find_agent(&path_dirs, agent)?;

    let config_env = env::config_env();
    let layers = config_layers(&fleet.primary, &worktree.path, &config_env)?;
    let writable = config::resolve_writable(&layers, &config_env)?;
    check_writable_approved(&fleet.primary, worktree, &layers, approval)?;
    let common = fs::canonicalize(&git::common_dir(&fleet.primary)?)?;
    let admin_dirs = prepare_admin_dirs(&common)?;
    let admin = AdminDir::new(fs::canonicalize(admin.path())?);
    let worktree_path = fs::canonicalize(&worktree.path)?;
    let plan = Plan {
        program: &program,
        common: &common,
        admin_dirs: &admin_dirs,
        writable: &writable.value,
        admin: &admin,
        worktree: &worktree_path,
        path_dirs: &path_dirs,
        resumed: resumed.as_ref(),
    };
    let launched = match agent {
        Agent::ClaudeCode => claude_launch(&plan)?,
        Agent::OpenCode => opencode_launch(&plan)?,
    };

    let size = terminal::stdin_size()
        .unwrap_or(Size::DEFAULT)
        .with(wanted.rows, wanted.cols);
    let holder_args = HolderArgs {
        worktree: worktree_path.clone(),
        admin: admin.clone(),
        size,
        argv: launched.argv,
        events: launched.events,
    };
    let pid = holder::spawn(&holder_args, launch::STRIPPED_VARS, &launched.vars)?;
    write_agent(lock, agent)?;
    notify_changed(&admin);
    Ok(Started {
        worktree: refreshed(worktree, &admin)?,
        agent,
        outcome: StartOutcome::Started { pid, resumed },
        writable: Some(writable),
    })
}

/// The agent to start in `worktree`: the one asked for, else the one it recorded, else
/// claude-code.
fn chosen_agent(worktree: &Worktree, asked: Option<Agent>) -> Agent {
    asked
        .or_else(|| recorded_agent(worktree))
        .unwrap_or(Agent::ClaudeCode)
}

/// The agent `worktree` records, when it is one this core knows.
fn recorded_agent(worktree: &Worktree) -> Option<Agent> {
    worktree.agent.as_deref().and_then(Agent::from_name)
}

/// What a launch needs to know, resolved before it.
struct Plan<'a> {
    /// The agent's program, by its absolute path.
    program: &'a Path,
    /// The git common dir, resolved.
    common: &'a Path,
    /// Every worktree's admin directory, made before the launch.
    admin_dirs: &'a [PathBuf],
    /// The extra writable paths the config allows, approved.
    writable: &'a [PathBuf],
    admin: &'a AdminDir,
    /// The worktree, resolved.
    worktree: &'a Path,
    path_dirs: &'a [PathBuf],
    /// The session to continue, if any.
    resumed: Option<&'a SessionId>,
}

/// What the holder is handed for the agent: its command line, the variables set for it, and the
/// agent whose hook runs for the events it reports through a pipe, if it does.
struct Launched {
    argv: Vec<String>,
    vars: Vec<(&'static str, String)>,
    events: Option<String>,
}

/// claude's launch: its settings, which turn its sandbox on and register its hooks, written to
/// `claude-settings.json` once the guard passed the command line.
fn claude_launch(plan: &Plan<'_>) -> Result<Launched> {
    let hooks = hook_target(plan.admin)?;
    let settings = launch::claude_settings(plan.common, plan.admin_dirs, plan.writable, &hooks);
    let argv = launch::claude_argv(
        &plan.program.display().to_string(),
        &plan.admin.claude_settings(),
        plan.resumed,
    );
    // The guard: no launch bypasses permissions, whatever the code above produced.
    launch::check_launch(&argv, &settings)?;
    fs::write_atomically(
        &plan.admin.claude_settings(),
        &launch::settings_bytes(&settings)?,
    )?;
    Ok(Launched {
        argv,
        vars: Vec::new(),
        events: None,
    })
}

/// opencode's launch (#35): its command line inside the sandbox of a policy generated now (the
/// macOS profile written to `opencode-sandbox.sb` once the guard passed it), and its config
/// directory, holding our plugin, which `OPENCODE_CONFIG_DIR` names. Its events come through the
/// holder.
fn opencode_launch(plan: &Plan<'_>) -> Result<Launched> {
    ensure_trust_file(plan.common)?;
    let policy = opencode_policy(plan)?;
    let (wrapper, profile) = sandbox_wrapper(plan, &policy)?;
    let argv = opencode::opencode_argv(wrapper, plan.program, plan.resumed);
    // The guard: no launch bypasses permissions, whatever the code above produced.
    launch::check_launch(&argv, &Value::Null)?;
    if let Some(profile) = profile {
        fs::write_atomically(&plan.admin.opencode_profile(), profile.as_bytes())?;
    }
    let config_dir = write_plugin(plan.admin)?;
    Ok(Launched {
        argv,
        vars: vec![(opencode::CONFIG_DIR_VAR, config_dir.display().to_string())],
        events: Some(Agent::OpenCode.program().to_owned()),
    })
}

/// What opencode's sandbox allows and denies: writes to the worktree, the git common dir,
/// opencode's own directories (made first, since inside the sandbox it could not make the
/// directories above them), the temporary directories and the approved extra paths; never to our
/// state.
fn opencode_policy(plan: &Plan<'_>) -> Result<Policy> {
    let dirs_env = env::opencode_dirs_env();
    let mut writable = vec![plan.worktree.to_owned(), plan.common.to_owned()];
    for dir in opencode::own_dirs(&dirs_env) {
        fs::create_dir_all(&dir)?;
        writable.push(fs::canonicalize(&dir)?);
    }
    writable.extend(opencode::home_config_dir(&dirs_env).map(|dir| fs::physical(&dir)));
    writable.extend(
        opencode::temp_dirs(&dirs_env)
            .iter()
            .map(|dir| fs::physical(dir)),
    );
    writable.extend(plan.writable.iter().map(|path| fs::physical(path)));

    let common = plan.common;
    let mut denied = plan.admin_dirs.to_vec();
    denied.push(common.join("refs").join(ADMIN_DIR_NAME));
    denied.push(trust::trust_file(common));
    denied.push(common.join(ADMIN_DIR_NAME));
    let mut denied_missing = Vec::new();
    let local_config = common.join(config::FILE_NAME);
    if fs::exists(&local_config) {
        denied.push(local_config);
    } else {
        denied_missing.push(local_config);
    }
    Ok(Policy {
        writable,
        denied,
        denied_missing,
        common: common.to_owned(),
    })
}

/// What runs opencode inside `policy`, before its own command line: `sandbox-exec -f <profile>`
/// on macOS, with the profile to write, or `bwrap <arguments> --` on Linux.
fn sandbox_wrapper(plan: &Plan<'_>, policy: &Policy) -> Result<(Vec<String>, Option<String>)> {
    match Platform::current() {
        Platform::MacOs => {
            let wrapper = vec![
                tool_path(plan.path_dirs, "sandbox-exec")?,
                "-f".to_owned(),
                plan.admin.opencode_profile().display().to_string(),
            ];
            Ok((wrapper, Some(policy::seatbelt_profile(policy)?)))
        }
        Platform::Linux => {
            let mut wrapper = vec![tool_path(plan.path_dirs, "bwrap")?];
            wrapper.extend(policy::bwrap_args(policy));
            wrapper.push("--".to_owned());
            Ok((wrapper, None))
        }
        // `sandbox::check_available` has refused this system already.
        Platform::Other => bail!("opencode's sandbox runs on macOS and Linux only"),
    }
}

/// The absolute path of the sandbox tool `name` on `path_dirs`, which the check of the sandbox
/// found there.
fn tool_path(path_dirs: &[PathBuf], name: &str) -> Result<String> {
    let path =
        fs::executable_in(path_dirs, name).with_context(|| format!("{name} is not on PATH"))?;
    Ok(path.display().to_string())
}

/// Writes opencode's config directory in `admin`: our plugin, and the `.gitignore` opencode would
/// otherwise try to write there. Returns the directory, for `OPENCODE_CONFIG_DIR`.
fn write_plugin(admin: &AdminDir) -> Result<PathBuf> {
    let dir = admin.opencode_config();
    let plugin = dir.join(opencode::PLUGIN_FILE);
    fs::create_dir_all(plugin.parent().unwrap_or(&dir))?;
    fs::write_atomically(&plugin, opencode::PLUGIN.as_bytes())?;
    fs::write_atomically(
        &dir.join(opencode::GITIGNORE_FILE),
        opencode::GITIGNORE.as_bytes(),
    )?;
    Ok(dir)
}

/// Refuses, as the typed `untrusted`, extra writable paths that come from a committed
/// `orca-term.yaml` of `worktree` and that the person has not approved, before or now through
/// `approval`. Paths from the override or the global config file need no approval.
fn check_writable_approved(
    primary: &PrimaryCheckout,
    worktree: &Worktree,
    layers: &[Layer],
    approval: &mut Approval<'_>,
) -> Result<()> {
    let Some((file, paths)) = config::committed_writable(layers) else {
        return Ok(());
    };
    let unapproved = Unapproved {
        name: worktree.name.clone(),
        subject: Subject::Sandbox {
            writable: paths.to_vec(),
        },
        file: fs::canonicalize(file)?,
    };
    if is_approved(primary, &worktree.path, &unapproved, approval)? {
        Ok(())
    } else {
        Err(unapproved.refused().into())
    }
}

/// The report of a holder already alive in `worktree`, whose admin directory is `admin`, running
/// `agent`, with its pid when `agent.json` can be read; `None` when no holder is alive. Fails as
/// `busy` when the holder runs another agent, naming it.
fn already_running(worktree: &Worktree, admin: &AdminDir, agent: Agent) -> Result<Option<Started>> {
    if fs::lock_status_at(&admin.agent_lock())? != LockStatus::Held {
        return Ok(None);
    }
    let running = recorded_agent(worktree).unwrap_or(agent);
    if running != agent {
        return Err(TypedError::Busy {
            name: worktree.name.clone(),
            path: worktree.path.clone(),
            reason: BusyReason::AnotherAgent {
                running,
                wanted: agent,
            },
        }
        .into());
    }
    let pid = fs::read_optional(&admin.agent_record())?
        .and_then(|bytes| HolderRecord::parse(&bytes).ok())
        .map(|record| record.pid);
    Ok(Some(Started {
        worktree: refreshed(worktree, admin)?,
        agent,
        outcome: StartOutcome::AlreadyRunning { pid },
        writable: None,
    }))
}

/// The session `agent` resumes in the worktree `name`, whose admin directory is `admin`: none with
/// `--fresh`, nor when `status.json` records no session, one another agent recorded, or cannot be
/// used; else the recorded one, once its id is checked and, for claude, its transcript found on
/// disk (or the typed `session_gone`).
fn session_to_resume(
    name: &str,
    admin: &AdminDir,
    agent: Agent,
    mode: StartMode,
) -> Result<Option<SessionId>> {
    if mode == StartMode::Fresh {
        return Ok(None);
    }
    let status = turn::read_status(fs::read_optional(&admin.status_file())?.as_deref()).usable();
    let Some(resume) = launch::session_to_resume(name, status.as_ref(), agent)? else {
        return Ok(None);
    };
    let exists = resume.transcript.as_deref().is_some_and(fs::is_file);
    Ok(Some(launch::check_transcript(name, resume, exists)?))
}

/// Creates `refs/orca-term`, the `orca-term` directory (where the registry of running Neovims
/// lives) and every worktree's `orca-term` directory under `common` before the launch, and
/// returns the last: the sandbox denies writes to them by name, and on Linux a denied path that
/// does not exist would be stood in for by a placeholder file. Canonical paths, since `common` is.
fn prepare_admin_dirs(common: &Path) -> Result<Vec<PathBuf>> {
    fs::create_dir_all(&common.join("refs").join(ADMIN_DIR_NAME))?;
    fs::create_dir_all(&common.join(ADMIN_DIR_NAME))?;
    let mut admin_dirs = Vec::new();
    for worktree_dir in fs::list_dirs(&common.join("worktrees"))? {
        let admin_dir = worktree_dir.join(ADMIN_DIR_NAME);
        fs::create_dir_all(&admin_dir)?;
        admin_dirs.push(admin_dir);
    }
    Ok(admin_dirs)
}

/// Where the agent's hooks report: this orca-term, by its resolved absolute path, and the
/// worktree's admin directory `admin`.
fn hook_target(admin: &AdminDir) -> Result<launch::HookTarget> {
    Ok(launch::HookTarget {
        orca_term: fs::canonicalize(&env::current_exe()?)?,
        admin: admin.path().to_owned(),
    })
}

/// `worktree` with its state read again from `admin`.
pub(crate) fn refreshed(worktree: &Worktree, admin: &AdminDir) -> Result<Worktree> {
    let derived = derive_state(admin)?;
    Ok(Worktree {
        name: worktree.name.clone(),
        path: worktree.path.clone(),
        branch: worktree.branch.clone(),
        state: derived.state,
        activity: derived.activity,
        agent: derived.agent,
        holder: derived.holder,
        unseen: derived.unseen,
        notes: derived.notes,
        denials: derived.denials,
        operation: derived.operation,
        pr: derived.pr,
    })
}
