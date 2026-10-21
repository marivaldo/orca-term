//! Ops: `session new`, which discards a worktree's current session (#41): it moves the
//! current-session pointer only, so the next start of its agent begins a new session. When the
//! agent runs, it is restarted at once on a new session, on the same worktree; that is refused as
//! `busy` while the agent is in a turn. Every session keeps its turns, and `session resume` brings
//! the discarded one back.

use anyhow::Result;

use crate::adapters::env;
use crate::domain::agent::{Agent, StartMode};
use crate::domain::error::BusyReason;
use crate::domain::session::{self, Discarded};
use crate::domain::worktree::{self, Target};
use crate::ops::agent_start::{self, Wanted, refreshed};
use crate::ops::agent_stop::stop_holder;
use crate::ops::agent_swap::{check_can_launch, check_not_in_turn};
use crate::ops::{
    Approval, change_status, find_target_admin, lock_worktree, notify_changed, read_fleet,
    read_status,
};

/// Discards the current session of the worktree `target` names or locates in the repository
/// containing the current directory, restarting its agent on a new session when it runs, the
/// agent's first terminal size `rows` and `cols` where given.
pub(crate) fn run(
    target: &Target,
    rows: Option<u16>,
    cols: Option<u16>,
    approval: &mut Approval<'_>,
) -> Result<Discarded> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let (worktree, admin) = find_target_admin(&fleet, &cwd, target)?;
    let lock = lock_worktree(&worktree.name, &worktree.path)?;
    let discarded = read_status(&admin).and_then(|status| status.session);
    let running = worktree.holder.is_alive();
    // A holder is alive only for an agent the worktree recorded.
    let agent = worktree
        .agent
        .as_deref()
        .and_then(Agent::from_name)
        .unwrap_or(Agent::ClaudeCode);
    if running {
        worktree::check_can_run_agent(worktree)?;
        check_not_in_turn(worktree, BusyReason::SessionInTurn)?;
        check_can_launch(agent)?;
        stop_holder(&admin)?;
    }
    if discarded.is_some() {
        change_status(&admin, &worktree.name, session::discard)?;
        notify_changed(&admin);
    }
    let started = if running {
        let wanted = Wanted {
            agent: Some(agent),
            rows,
            cols,
            mode: StartMode::Fresh,
        };
        Some(agent_start::start(
            &fleet, worktree, &admin, &lock, &wanted, approval,
        )?)
    } else {
        None
    };
    Ok(Discarded {
        worktree: refreshed(worktree, &admin)?,
        session: discarded,
        started,
    })
}
