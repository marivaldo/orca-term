//! App: projects and worktrees made up for the application's tests, as the core would read them.

use std::path::PathBuf;

use anyhow::anyhow;

use crate::domain::agent::HolderStatus;
use crate::domain::branch::Branch;
use crate::domain::fleet::{Fleet, PrimaryCheckout, Worktree};
use crate::domain::project::ProjectFleet;
use crate::domain::state::State;

/// A worktree at `path`, on a branch named after its directory, in `state`, with nothing else
/// known about it.
pub(crate) fn worktree(path: &str, state: State) -> Worktree {
    let path = PathBuf::from(path);
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    Worktree {
        branch: Some(Branch::new(&name)),
        name,
        path,
        primary: false,
        state,
        activity: None,
        agent: None,
        holder: HolderStatus::Gone,
        unseen: false,
        notes: 0,
        denials: 0,
        operation: None,
        pr: None,
        last_activity: None,
        changed_files: None,
    }
}

/// The project whose primary checkout is `path`, with `worktrees`.
pub(crate) fn project(path: &str, worktrees: Vec<Worktree>) -> ProjectFleet {
    ProjectFleet {
        path: PathBuf::from(path),
        gone: false,
        fleet: Ok(Fleet {
            primary: PrimaryCheckout {
                path: PathBuf::from(path),
                branch: Some(Branch::new("main")),
            },
            worktrees,
            remote_base: None,
            has_origin: false,
        }),
    }
}

/// The project whose directory at `path` is gone, as `fleets` reports it.
pub(crate) fn gone(path: &str) -> ProjectFleet {
    ProjectFleet {
        path: PathBuf::from(path),
        gone: true,
        fleet: Err(anyhow!(
            "the project's directory {path} is gone; take it off the list with `orca-term \
             project rm {path}`"
        )),
    }
}
