//! Ops: `fleets`, which reads the fleet of every project at once (#149), for the sidebar that
//! groups worktrees by project. Never writes.

use std::path::PathBuf;

use anyhow::{Result, anyhow};

use crate::adapters::fs;
use crate::domain::project::ProjectFleet;
use crate::ops::read_projects;
use crate::ops::worktree_ls::read_listed;

/// Every project's fleet, as `worktree ls` reads it, in the list's order. A project whose
/// directory is gone, or whose fleet cannot be read, carries why instead, and the others are
/// still read. Each project is read on a thread of its own, all at once.
pub(crate) fn run() -> Result<Vec<ProjectFleet>> {
    let projects = read_projects()?;
    let paths: Vec<PathBuf> = projects.paths().map(PathBuf::from).collect();
    std::thread::scope(|scope| {
        let readers: Vec<_> = paths
            .into_iter()
            .map(|path| scope.spawn(move || read_project(path)))
            .collect();
        readers
            .into_iter()
            .map(|reader| {
                reader
                    .join()
                    .map_err(|_| anyhow!("reading a project's fleet panicked"))
            })
            .collect()
    })
}

/// The fleet of the project whose primary checkout is `path`, or why it cannot be read.
fn read_project(path: PathBuf) -> ProjectFleet {
    if !fs::is_dir(&path) {
        let fleet = Err(anyhow!(
            "the project's directory {} is gone; take it off the list with `orca-term project \
             rm {}`",
            path.display(),
            path.display()
        ));
        return ProjectFleet {
            path,
            gone: true,
            fleet,
        };
    }
    let fleet = read_listed(&path);
    ProjectFleet {
        path,
        gone: false,
        fleet,
    }
}
