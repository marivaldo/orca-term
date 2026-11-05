//! Ops: `project add`, which puts a repository on the project list (#149). It writes only the
//! project list, and only when the repository was not on it.

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

/// Adds the repository containing the directory `dir` to the project list, by its primary
/// checkout, and says what it did; `None`, adding nothing, when `dir` is not inside a git
/// repository. The launcher (`orca-term` with no arguments, #148) is meant to call this with the
/// current directory before it opens Neovim, so that a repository the person works in is listed.
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
