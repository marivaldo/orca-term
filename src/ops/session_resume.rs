//! Ops: `session resume`, which makes one of a worktree's sessions its current one and restarts
//! its agent in its holder on it (#41): `claude --resume <id>` or `opencode -s <id>`, starting the
//! holder if none runs. A session of the other agent swaps the worktree's agent, with `agent
//! swap`'s checks: refused as `busy` while the agent is in a turn, the sandbox and the program
//! checked before anything is stopped, the extra writable paths of a committed `orca-term.yaml`
//! approved before the start. A session that cannot be resumed (claude's transcript gone, or
//! opencode no longer listing it) fails as `session_gone` before anything is stopped or
//! launched. The current session, its agent running, is only reported.

use std::path::Path;

use anyhow::Result;

use crate::adapters::{env, fs, program};
use crate::domain::agent::{Agent, StartMode, Swapped};
use crate::domain::error::{BusyReason, GoneReason, TypedError};
use crate::domain::launch::{self, SessionId, opencode};
use crate::domain::session::{self, Resumed, Session};
use crate::domain::worktree::{self, Target};
use crate::ops::agent_start::{self, Wanted};
use crate::ops::agent_stop::stop_holder;
use crate::ops::agent_swap::{check_can_launch, check_not_in_turn};
use crate::ops::{
    Approval, change_status, find_agent, find_target_admin, lock_worktree, notify_changed,
    read_fleet, read_sessions,
};

/// Makes the session `id` of the worktree `target` names or locates in the repository containing
/// the current directory its current session, and starts its agent on it, the agent's first
/// terminal size `rows` and `cols` where given. An id the worktree never held is refused.
pub(crate) fn run(
    target: &Target,
    id: &str,
    rows: Option<u16>,
    cols: Option<u16>,
    approval: &mut Approval<'_>,
) -> Result<Resumed> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let (worktree, admin) = find_target_admin(&fleet, &cwd, target)?;
    worktree::check_can_run_agent(worktree)?;
    let lock = lock_worktree(&worktree.name, &worktree.path)?;
    let sessions = read_sessions(&admin)?;
    let chosen = session::find(&worktree.name, &sessions, id)?;
    let agent = session::agent_of(&worktree.name, chosen)?;
    let previous = worktree.agent.as_deref().and_then(Agent::from_name);
    let running = worktree.holder.is_alive();
    let wanted = Wanted {
        agent: Some(agent),
        rows,
        cols,
        mode: StartMode::Resume,
    };

    // The current session, its own agent running: nothing to do but say so.
    let stopped = if chosen.current && running && previous == Some(agent) {
        false
    } else {
        check_resumable(&worktree.name, &worktree.path, chosen)?;
        if running {
            check_not_in_turn(worktree, BusyReason::SessionInTurn)?;
            check_can_launch(agent)?;
            stop_holder(&admin)?;
        }
        change_status(&admin, &worktree.name, |status| {
            session::make_current(status, chosen)
        })?;
        notify_changed(&admin);
        running
    };
    let started = agent_start::start(&fleet, worktree, &admin, &lock, &wanted, approval)?;
    Ok(Resumed {
        session: chosen.id.clone(),
        swapped: Swapped {
            previous,
            stopped,
            started,
        },
    })
}

/// Refuses, as `session_gone`, a session of the worktree `name` at `path` that cannot be resumed:
/// its id is not one an agent makes; claude's transcript of it is not recorded or gone from disk;
/// opencode's own list of its sessions does not hold it.
fn check_resumable(name: &str, path: &Path, session: &Session) -> Result<()> {
    let resume = session::to_resume(name, session)?;
    let transcript_exists = resume.transcript.as_deref().is_some_and(fs::is_file);
    let id = launch::check_transcript(name, resume, transcript_exists)?;
    if session.agent() == Some(Agent::OpenCode) {
        check_opencode_lists(name, path, &id)?;
    }
    Ok(())
}

/// Refuses, as `session_gone`, an opencode session that opencode's own list of the sessions of the
/// worktree at `path` does not hold (`opencode session list`, which only reads its database).
/// When the list cannot be had or read, nobody can tell, and the session is tried: opencode then
/// says itself that it cannot open it.
fn check_opencode_lists(name: &str, path: &Path, id: &SessionId) -> Result<()> {
    let program = find_agent(&env::path_dirs(), Agent::OpenCode)?;
    let Ok(listed) = program::listed_by(&program, opencode::SESSION_LIST_ARGS, path) else {
        return Ok(());
    };
    if opencode::lists_session(&listed, id.as_str()) == Some(false) {
        return Err(TypedError::SessionGone {
            name: name.to_owned(),
            session: id.as_str().to_owned(),
            reason: GoneReason::NotListed,
        }
        .into());
    }
    Ok(())
}
