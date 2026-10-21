//! Ops: `agent swap`, which swaps a worktree's agent for another (#35): it stops the agent running
//! there, refusing while that agent is in a turn, then starts the other one on the same worktree,
//! which records it. Turns, notes and snapshots belong to the worktree and stay. A session belongs
//! to its agent, so the new agent resumes only a session it recorded itself, and starts a new one
//! otherwise. Swapping in the agent the worktree already has is `agent start` of that agent.

use anyhow::Result;

use crate::adapters::{env, fs};
use crate::domain::agent::{Agent, StartMode, Swapped};
use crate::domain::error::{BusyReason, TypedError};
use crate::domain::fleet::Worktree;
use crate::domain::sandbox::{self, Platform};
use crate::domain::state::State;
use crate::domain::worktree::{self, Target};
use crate::ops::agent_start::{self, Wanted};
use crate::ops::agent_stop::stop_holder;
use crate::ops::{Approval, find_agent, find_target_admin, lock_worktree, read_fleet};

/// Swaps the agent of the worktree `target` names or locates in the repository containing the
/// current directory for `agent`, its first terminal size `rows` and `cols` where given. Fails as
/// `busy` when the worktree's lock is held or its agent is in a turn (working, or waiting for the
/// person), and otherwise as `agent start` fails; a missing sandbox or agent program is found
/// before the running agent is stopped.
pub(crate) fn run(
    target: &Target,
    agent: Agent,
    rows: Option<u16>,
    cols: Option<u16>,
    approval: &mut Approval<'_>,
) -> Result<Swapped> {
    let cwd = env::current_dir()?;
    let fleet = read_fleet(&cwd)?;
    let (worktree, admin) = find_target_admin(&fleet, &cwd, target)?;
    worktree::check_can_run_agent(worktree)?;
    let lock = lock_worktree(&worktree.name, &worktree.path)?;
    let previous = worktree.agent.as_deref().and_then(Agent::from_name);
    let swapping = previous != Some(agent) && worktree.holder.is_alive();
    if swapping {
        check_not_in_turn(worktree, BusyReason::InTurn)?;
        check_can_launch(agent)?;
        stop_holder(&admin)?;
    }
    let wanted = Wanted {
        agent: Some(agent),
        rows,
        cols,
        mode: StartMode::Resume,
    };
    let started = agent_start::start(&fleet, worktree, &admin, &lock, &wanted, approval)?;
    Ok(Swapped {
        previous,
        stopped: swapping,
        started,
    })
}

/// Refuses, as `busy` for `reason`, a worktree whose agent is in a turn: working, or waiting for
/// the person. `agent stop` stops it anyway, for a person who means to. The session commands
/// share it (#41).
pub(crate) fn check_not_in_turn(worktree: &Worktree, reason: BusyReason) -> Result<()> {
    if matches!(worktree.state, State::Working | State::Waiting) {
        return Err(TypedError::Busy {
            name: worktree.name.clone(),
            path: worktree.path.clone(),
            reason,
        }
        .into());
    }
    Ok(())
}

/// Refuses, as `agent start` would, an `agent` whose sandbox or program is missing here, before
/// the running agent is stopped for nothing. The session commands share it (#41).
pub(crate) fn check_can_launch(agent: Agent) -> Result<()> {
    let path_dirs = env::path_dirs();
    sandbox::check_available(Platform::current(), agent, &|tool| {
        fs::executable_in(&path_dirs, tool).is_some()
    })?;
    find_agent(&path_dirs, agent)?;
    Ok(())
}
