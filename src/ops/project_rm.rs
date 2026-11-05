//! Ops: `project rm`, which takes a project off the project list (#149). It writes only the
//! project list: never the project's directory, its worktrees or its branches.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::adapters::{env, fs, git};
use crate::domain::project::{ProjectRemoved, ProjectTarget, Projects};
use crate::ops::change_projects;

/// Takes the project `target` names off the list: by name (its directory's basename), refusing a
/// name several projects share as the typed `ambiguous_project`, or by a path to it. Refuses a
/// target that names no listed project.
pub(crate) fn run(target: &ProjectTarget) -> Result<ProjectRemoved> {
    let cwd = env::current_dir()?;
    change_projects(|projects| {
        let path = match target {
            ProjectTarget::Name(name) => projects.find_by_name(name)?,
            ProjectTarget::Path(path) => find_by_path(projects, &tidy(&cwd.join(path)))?,
        };
        projects.remove(&path);
        Ok(ProjectRemoved { path })
    })
}

/// The listed project at `wanted`: the same path, literally or through symlinks, else the primary
/// checkout of the repository containing `wanted` (a subdirectory, or a linked worktree). A gone
/// project matches only its path as listed.
fn find_by_path(projects: &Projects, wanted: &Path) -> Result<PathBuf> {
    if let Some(listed) = listed_as(projects, wanted) {
        return Ok(listed);
    }
    let primary = git::repository_of(wanted).ok().flatten();
    if let Some(listed) = primary.and_then(|primary| listed_as(projects, &primary.path)) {
        return Ok(listed);
    }
    bail!(
        "{} is not on the project list; see `orca-term project ls`",
        wanted.display()
    )
}

/// The listed path that is `path`, literally or once both are resolved through symlinks.
fn listed_as(projects: &Projects, path: &Path) -> Option<PathBuf> {
    projects
        .paths()
        .find(|listed| fs::same_path(listed, path))
        .map(Path::to_path_buf)
}

/// `path` without its `.` parts, so `/src/./app` matches a gone project listed as `/src/app`.
fn tidy(path: &Path) -> PathBuf {
    path.components().collect()
}
