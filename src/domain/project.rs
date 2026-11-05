//! Domain: the project list (`CONTEXT.md`, **Project**, #149). A project is a repository the
//! person works on, kept as the path of its primary checkout. The list keeps the order projects
//! were added, as Orca's list of repositories does.
//!
//! It lives in `projects.json`, in orca-term's state directory (`$XDG_STATE_HOME/orca-term`, or
//! `~/.local/state/orca-term`), outside every repository:
//!
//! ```json
//! {"schema":1,"projects":[{"path":"/home/me/src/app"},{"path":"/home/me/src/api"}]}
//! ```
//!
//! The commands that change it (`project add`, `project rm`) hold `projects.lock` beside it from
//! the read to the write, and replace the file whole. Like every state file it carries `schema`:
//! a file this core cannot read, or one of a newer schema, is refused, naming the file, so it is
//! never written over.

use std::convert::Infallible;
use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::domain::error::TypedError;
use crate::domain::fleet::Fleet;

/// The newest schema of `projects.json` this core reads and writes.
pub(crate) const PROJECTS_SCHEMA: u64 = 1;

/// The project list's file name, in orca-term's state directory.
const PROJECTS_FILE: &str = "projects.json";

/// The lock taken while the project list changes, beside it.
const PROJECTS_LOCK: &str = "projects.lock";

/// The project list's file, in the state directory `state_dir`.
pub(crate) fn projects_file(state_dir: &Path) -> PathBuf {
    state_dir.join(PROJECTS_FILE)
}

/// The lock taken while the project list in `state_dir` changes.
pub(crate) fn projects_lock(state_dir: &Path) -> PathBuf {
    state_dir.join(PROJECTS_LOCK)
}

/// The project list, as `projects.json` records it: the primary checkouts' paths, in the order
/// they were added.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Projects {
    #[serde(default)]
    schema: Option<u64>,
    #[serde(default)]
    projects: Vec<Entry>,
}

/// One project on the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    /// The primary checkout's absolute path, as git lists it.
    path: PathBuf,
}

impl Projects {
    /// Every project's path, in the order they were added.
    pub(crate) fn paths(&self) -> impl Iterator<Item = &Path> {
        self.projects.iter().map(|entry| entry.path.as_path())
    }

    /// Adds the project whose primary checkout is `path` at the end of the list. Returns whether
    /// it was added: `false` when it is on the list already, which changes nothing.
    pub(crate) fn add(&mut self, path: &Path) -> bool {
        if self.paths().any(|listed| listed == path) {
            return false;
        }
        self.projects.push(Entry {
            path: path.to_owned(),
        });
        true
    }

    /// Takes the project whose path is exactly `path` off the list; the others keep their order.
    pub(crate) fn remove(&mut self, path: &Path) {
        self.projects.retain(|entry| entry.path != path);
    }

    /// The listed project named `name` (its directory's basename). Refuses a name no project has,
    /// and, as the typed `ambiguous_project` naming them, a name several projects share.
    pub(crate) fn find_by_name(&self, name: &str) -> Result<PathBuf> {
        let candidates: Vec<PathBuf> = self
            .paths()
            .filter(|path| name_of(path) == name)
            .map(Path::to_path_buf)
            .collect();
        match candidates.as_slice() {
            [] => bail!("no project named {name} is on the list; see `orca-term project ls`"),
            [only] => Ok(only.clone()),
            _ => Err(TypedError::AmbiguousProject {
                name: name.to_owned(),
                candidates,
            }
            .into()),
        }
    }

    /// The bytes written to `projects.json`, with this core's schema.
    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>> {
        let file = Self {
            schema: Some(PROJECTS_SCHEMA),
            projects: self.projects.clone(),
        };
        Ok(serde_json::to_vec(&file)?)
    }
}

/// The project list in `projects.json`, whose content is `bytes` (`None` when it does not exist:
/// no project yet). A file that is not a list, or whose schema is newer than this core supports,
/// is refused, naming the file, so it is never written over.
pub(crate) fn read(bytes: Option<&[u8]>) -> Result<Projects> {
    let Some(bytes) = bytes else {
        return Ok(Projects::default());
    };
    let projects: Projects = match serde_json::from_slice(bytes) {
        Ok(projects) => projects,
        Err(err) => bail!("could not parse {PROJECTS_FILE}: {err}; fix or remove it"),
    };
    if let Some(schema) = projects.schema
        && schema > PROJECTS_SCHEMA
    {
        bail!(
            "{PROJECTS_FILE} has schema {schema}, newer than the schema {PROJECTS_SCHEMA} this \
             orca-term supports; upgrade orca-term to use the project list"
        );
    }
    Ok(projects)
}

/// A project's name: its directory's basename, or the whole path when it has none (`/`).
pub(crate) fn name_of(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// What `project rm` is given: a project's name, or a path to it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectTarget {
    /// A project's name: its directory's basename.
    Name(String),
    /// A path to the project, relative to the current directory or absolute.
    Path(PathBuf),
}

/// The target as the person wrote it.
impl fmt::Display for ProjectTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Name(name) => f.write_str(name),
            Self::Path(path) => write!(f, "{}", path.display()),
        }
    }
}

/// A target is a path when it contains `/` or is `.` or `..`, and a name otherwise, as a worktree
/// target is.
impl FromStr for ProjectTarget {
    type Err = Infallible;

    fn from_str(target: &str) -> Result<Self, Self::Err> {
        if target.contains('/') || target == "." || target == ".." {
            Ok(Self::Path(PathBuf::from(target)))
        } else {
            Ok(Self::Name(target.to_owned()))
        }
    }
}

/// What `project add` did: the project's primary checkout, and whether it was `added` (`false`
/// when it was on the list already).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectAdded {
    pub(crate) path: PathBuf,
    pub(crate) added: bool,
}

/// What `project rm` took off the list: the project's primary checkout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProjectRemoved {
    pub(crate) path: PathBuf,
}

/// A project as `project ls` lists it: its path, and whether its directory is `gone`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ListedProject {
    pub(crate) path: PathBuf,
    pub(crate) gone: bool,
}

/// A project's fleet as `fleets` reads it: its path, whether its directory is `gone`, and its
/// fleet as `worktree ls` reads it, or why it could not be read.
#[derive(Debug)]
pub(crate) struct ProjectFleet {
    pub(crate) path: PathBuf,
    pub(crate) gone: bool,
    pub(crate) fleet: Result<Fleet>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::error;

    fn listed(paths: &[&str]) -> Projects {
        let mut projects = Projects::default();
        for path in paths {
            projects.add(Path::new(path));
        }
        projects
    }

    fn paths(projects: &Projects) -> Vec<&Path> {
        projects.paths().collect()
    }

    #[test]
    fn projects_keep_the_order_they_were_added_and_adding_twice_changes_nothing() {
        let mut projects = listed(&["/src/b", "/src/a"]);
        assert!(!projects.add(Path::new("/src/b")));
        assert!(projects.add(Path::new("/src/c")));
        assert_eq!(
            paths(&projects),
            [
                Path::new("/src/b"),
                Path::new("/src/a"),
                Path::new("/src/c")
            ]
        );
    }

    #[test]
    fn removing_a_project_keeps_the_others_in_order() {
        let mut projects = listed(&["/src/a", "/src/b", "/src/c"]);
        projects.remove(Path::new("/src/b"));
        assert_eq!(paths(&projects), [Path::new("/src/a"), Path::new("/src/c")]);
    }

    #[test]
    fn a_name_finds_its_one_project_and_an_ambiguous_one_names_the_candidates() {
        let projects = listed(&["/a/app", "/src/api", "/b/app"]);
        assert_eq!(
            projects.find_by_name("api").unwrap(),
            PathBuf::from("/src/api")
        );
        let err = projects.find_by_name("app").unwrap_err();
        assert_eq!(error::kind(&err), "ambiguous_project");
        assert!(err.to_string().contains("/a/app, /b/app"), "{err}");
        let err = projects.find_by_name("web").unwrap_err();
        assert_eq!(error::kind(&err), "other");
        assert!(err.to_string().starts_with("no project named web"), "{err}");
    }

    #[test]
    fn the_file_reads_back_as_written_with_its_schema() {
        let projects = listed(&["/src/a", "/src/b"]);
        let bytes = projects.to_bytes().unwrap();
        assert_eq!(
            String::from_utf8(bytes.clone()).unwrap(),
            r#"{"schema":1,"projects":[{"path":"/src/a"},{"path":"/src/b"}]}"#
        );
        assert_eq!(paths(&read(Some(&bytes)).unwrap()), paths(&projects));
        assert_eq!(read(None).unwrap(), Projects::default());
    }

    #[test]
    fn a_newer_schema_or_a_broken_file_is_refused() {
        let err = read(Some(br#"{"schema":2,"projects":[]}"#)).unwrap_err();
        assert!(err.to_string().contains("newer than the schema 1"), "{err}");
        let err = read(Some(b"not json")).unwrap_err();
        assert!(
            err.to_string().starts_with("could not parse projects.json"),
            "{err}"
        );
    }

    #[test]
    fn a_target_is_a_path_when_it_holds_a_slash_or_is_a_dot() {
        let target = |text: &str| text.parse::<ProjectTarget>().unwrap();
        assert_eq!(target("app"), ProjectTarget::Name("app".to_owned()));
        assert_eq!(target("."), ProjectTarget::Path(PathBuf::from(".")));
        assert_eq!(
            target("../app"),
            ProjectTarget::Path(PathBuf::from("../app"))
        );
    }

    #[test]
    fn a_project_is_named_after_its_directory() {
        assert_eq!(name_of(Path::new("/src/app")), "app");
        assert_eq!(name_of(Path::new("/")), "/");
    }
}
