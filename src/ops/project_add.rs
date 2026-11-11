//! Ops: `project add`, which puts a repository on the project list (#149), and the same step the
//! application takes as it opens (#193). It writes only the project list, and only when the
//! repository was not on it.

use std::path::Path;

use anyhow::{Result, bail};

use crate::adapters::{env, fs, git};
use crate::domain::error::TypedError;
use crate::domain::project::ProjectAdded;
use crate::ops::change_projects;

/// Adds the repository containing `path` (the current directory when `None`), by its primary
/// checkout, even from a linked worktree or a subdirectory. Adding it twice changes nothing.
/// Refuses a path that is not a directory, and, as the typed `not_a_repository`, one outside every
/// git repository.
pub(crate) fn run(path: Option<&Path>) -> Result<ProjectAdded> {
    let cwd = env::current_dir()?;
    let dir = path.map_or_else(|| cwd.clone(), |path| cwd.join(path));
    if !fs::is_dir(&dir) {
        bail!("{} is not a directory", dir.display());
    }
    match add_repository_of(&dir)? {
        Some(added) => Ok(added),
        None => Err(TypedError::NotARepository { path: dir }.into()),
    }
}

/// Adds the repository containing the current directory to the project list, as `project add`
/// does, so that the repository the person opens the application in is listed (#152, #193). The
/// application calls it as it opens, from `orca-term app` and from `orca-term` with no arguments.
/// It says nothing, whether it added it, it was already there or the directory is outside every
/// repository. A project list that cannot be changed (a broken `projects.json`, a newer schema,
/// its lock held, git missing) never stops the application: nothing is added, and the application
/// shows the problem when it reads the same list through `fleets`.
pub(crate) fn add_current_repository() {
    let Ok(dir) = env::current_dir() else {
        return;
    };
    // Whatever happened, the application opens: the result is ignored on purpose.
    let _ = add_repository_of(&dir);
}

/// Adds the repository containing the directory `dir` to the project list, by its primary
/// checkout, and says what it did; `None`, adding nothing, when `dir` is not inside a git
/// repository.
pub(crate) fn add_repository_of(dir: &Path) -> Result<Option<ProjectAdded>> {
    let Some(primary) = git::repository_of(dir)? else {
        return Ok(None);
    };
    let added = change_projects(|projects| Ok(projects.add(&primary.path)))?;
    Ok(Some(ProjectAdded {
        path: primary.path,
        added,
    }))
}
