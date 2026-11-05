//! Ops: `project ls`, which lists the project list (#149). Never writes, and runs no git.

use anyhow::Result;

use crate::adapters::fs;
use crate::domain::project::ListedProject;
use crate::ops::read_projects;

/// Every project, in the order they were added. A project whose directory is gone stays on the
/// list, marked `gone`.
pub(crate) fn run() -> Result<Vec<ListedProject>> {
    let projects = read_projects()?;
    Ok(projects
        .paths()
        .map(|path| ListedProject {
            path: path.to_owned(),
            gone: !fs::is_dir(path),
        })
        .collect())
}
