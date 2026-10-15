//! Ops: `agent start`, which launches claude in a worktree inside a detached holder (ADR 0009),
//! under claude's native sandbox with everything allowed inside the worktree (ADR 0007), with
//! hooks that call this orca-term back on every turn event. Starting an agent that already runs
//! is not an error: the report says so.

use anyhow::{Context, Result};

use crate::adapters::fs;
use crate::adapters::holder::{self, HolderArgs};
use crate::adapters::{env, git, program, terminal};
use crate::domain::agent::{Agent, HolderRecord, Size, StartOutcome, Started};
use crate::domain::config;
use crate::domain::fleet::Worktree;
use crate::domain::launch;
use crate::domain::sandbox::{self, Platform};
use crate::domain::state::{ADMIN_DIR_NAME, AdminDir, LockStatus};
use crate::domain::worktree::{self, Target};
use crate::ops::{
    config_layers, derive_state, find_target_admin, lock_worktree, read_fleet, write_agent,
};

/// Starts claude in the worktree `target` names or locates in the repository containing the
/// current directory, with `rows` and `cols` as its first terminal size where given (else the
/// starter's terminal size, else 24 by 80). Fails as `busy` when the worktree's lock is held and
/// as `no_sandbox` when claude's sandbox cannot run here.
pub(crate) fn run(target: &Target, rows: Option<u16>, cols: Option<u16>) -> Result<Started> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let (worktree, admin) = find_target_admin(&fleet, &cwd, target)?;
    worktree::check_can_run_agent(worktree)?;
    let lock = lock_worktree(&worktree.name, &worktree.path)?;
    let agent = Agent::ClaudeCode;
    if fs::lock_status_at(&admin.agent_lock())? == LockStatus::Held {
        let pid = fs::read_optional(&admin.agent_record())?
            .and_then(|bytes| HolderRecord::parse(&bytes).ok())
            .map(|record| record.pid);
        return Ok(Started {
            worktree: refreshed(worktree, &admin)?,
            agent,
            outcome: StartOutcome::AlreadyRunning { pid },
            writable: None,
        });
    }

    let path_dirs = env::path_dirs();
    sandbox::check_available(Platform::current(), &|tool| {
        fs::executable_in(&path_dirs, tool).is_some()
    })?;
    let claude = fs::executable_in(&path_dirs, agent.program()).with_context(|| {
        format!(
            "{} is not on PATH; install Claude Code, or add it to PATH",
            agent.program()
        )
    })?;
    launch::check_version(&program::version(&claude)?)?;

    let config_env = env::config_env();
    let layers = config_layers(&fleet.primary, &worktree.path, &config_env)?;
    let writable = config::resolve_writable(&layers, &config_env)?;
    let common = fs::canonicalize(&git::common_dir(&fleet.primary)?)?;
    let admin_dirs = prepare_admin_dirs(&common)?;
    let admin = AdminDir::new(fs::canonicalize(admin.path())?);
    let hooks = hook_target(&admin)?;
    let settings = launch::claude_settings(&common, &admin_dirs, &writable.value, &hooks);
    let argv = launch::claude_argv(&claude.display().to_string(), &admin.claude_settings());
    // The guard: no launch bypasses permissions, whatever the code above produced.
    launch::check_launch(&argv, &settings)?;
    fs::write_atomically(
        &admin.claude_settings(),
        &launch::settings_bytes(&settings)?,
    )?;

    let size = terminal::stdin_size()
        .unwrap_or(Size::DEFAULT)
        .with(rows, cols);
    let holder_args = HolderArgs {
        worktree: fs::canonicalize(&worktree.path)?,
        admin: admin.clone(),
        size,
        argv,
    };
    let pid = holder::spawn(&holder_args, launch::STRIPPED_VARS)?;
    write_agent(&lock, agent)?;
    Ok(Started {
        worktree: refreshed(worktree, &admin)?,
        agent,
        outcome: StartOutcome::Started { pid },
        writable: Some(writable),
    })
}

/// Creates `refs/orca-term` and every worktree's `orca-term` directory under `common` before the
/// launch, and returns the latter: the sandbox denies writes to them by name, and on Linux a
/// denied path that does not exist would be stood in for by a placeholder file. Canonical paths,
/// since `common` is.
fn prepare_admin_dirs(common: &std::path::Path) -> Result<Vec<std::path::PathBuf>> {
    fs::create_dir_all(&common.join("refs").join(ADMIN_DIR_NAME))?;
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
fn refreshed(worktree: &Worktree, admin: &AdminDir) -> Result<Worktree> {
    let derived = derive_state(admin)?;
    Ok(Worktree {
        name: worktree.name.clone(),
        path: worktree.path.clone(),
        branch: worktree.branch.clone(),
        state: derived.state,
        activity: derived.activity,
        agent: derived.agent,
        holder: derived.holder,
    })
}
