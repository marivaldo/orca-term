//! The fleet: every worktree of one repository, derived from git itself.
//!
//! The fleet is `git worktree list` minus the primary checkout, which git always lists first. A
//! worktree is keyed by its path and named by that directory's basename.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::Serialize;

use crate::git;
use crate::state::{self, State};

/// A worktree as git reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct WorktreeEntry {
    pub(crate) path: PathBuf,
    /// The checked-out branch, without `refs/heads/`. `None` when detached or bare.
    pub(crate) branch: Option<String>,
    /// Why git marks the worktree prunable, when it does: its directory or `.git` file is gone.
    #[serde(skip)]
    pub(crate) prunable: Option<String>,
}

/// One worktree of the fleet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Worktree {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) branch: Option<String>,
    pub(crate) state: State,
    /// Why the worktree is in its state, when there is something to say.
    pub(crate) detail: Option<String>,
    /// Whether the worktree's directory is gone, so only `worktree prune` can clean it.
    #[serde(skip)]
    pub(crate) gone: bool,
}

/// The primary checkout and every worktree of its repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Fleet {
    pub(crate) primary: WorktreeEntry,
    pub(crate) worktrees: Vec<Worktree>,
}

impl Fleet {
    /// Reads the fleet of the repository containing `dir`. Never writes.
    pub(crate) fn discover(dir: &Path) -> Result<Self> {
        let entries = list_entries(dir)?;
        let prunable: Vec<Option<String>> = entries
            .iter()
            .skip(1)
            .map(|wt| wt.prunable.clone())
            .collect();
        let mut fleet = Self::from_entries(entries)?;
        for (worktree, prunable) in fleet.worktrees.iter_mut().zip(prunable) {
            worktree.read_state(prunable);
        }
        Ok(fleet)
    }

    /// The worktrees whose directory is gone: the ones `worktree prune` would clean.
    pub(crate) fn gone_worktrees(&self) -> impl Iterator<Item = &Worktree> {
        self.worktrees.iter().filter(|worktree| worktree.gone)
    }

    fn from_entries(entries: Vec<WorktreeEntry>) -> Result<Self> {
        let mut entries = entries.into_iter();
        let Some(primary) = entries.next() else {
            bail!("git listed no worktrees");
        };
        let worktrees = entries
            .map(|wt| Worktree {
                name: worktree_name(&wt.path),
                path: wt.path,
                branch: wt.branch,
                state: State::NoAgent,
                detail: None,
                gone: false,
            })
            .collect();
        Ok(Self { primary, worktrees })
    }
}

impl Worktree {
    /// Reads the worktree's state. A worktree whose directory is gone, or whose state cannot be read, is
    /// broken, and the reason goes in `detail`: reading one worktree never fails the fleet.
    fn read_state(&mut self, prunable: Option<String>) {
        if !self.path.is_dir() || prunable.is_some() {
            self.state = State::Broken;
            self.gone = true;
            self.detail = Some(if self.path.is_dir() {
                let reason = prunable.unwrap_or_default();
                format!("git marks it prunable ({reason}); run orca-term worktree prune")
            } else {
                "directory missing; run orca-term worktree prune".to_owned()
            });
            return;
        }
        match state::admin_dir(&self.path).and_then(|admin| state::read(&admin)) {
            Ok(state) => self.state = state,
            Err(err) => {
                self.state = State::Broken;
                self.detail = Some(format!("{err:#}"));
            }
        }
    }
}

/// The primary checkout of the repository containing `dir`, without reading any worktree. Never writes.
pub(crate) fn primary_checkout(dir: &Path) -> Result<WorktreeEntry> {
    match list_entries(dir)?.into_iter().next() {
        Some(primary) => Ok(primary),
        None => bail!("git listed no worktrees"),
    }
}

fn list_entries(dir: &Path) -> Result<Vec<WorktreeEntry>> {
    let porcelain = git::output(dir, &["worktree", "list", "--porcelain", "-z"])?;
    Ok(parse_porcelain(&porcelain))
}

fn worktree_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.to_string_lossy(), OsStr::to_string_lossy)
        .into_owned()
}

/// Parses `git worktree list --porcelain -z`: attribute lines end in NUL, and an empty attribute
/// line ends each record.
fn parse_porcelain(raw: &[u8]) -> Vec<WorktreeEntry> {
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for field in raw.split(|&b| b == 0) {
        if field.is_empty() {
            entries.extend(current.take());
        } else if let Some(path) = field.strip_prefix(b"worktree ") {
            entries.extend(current.take());
            current = Some(WorktreeEntry {
                path: PathBuf::from(OsStr::from_bytes(path)),
                branch: None,
                prunable: None,
            });
        } else if let (Some(branch), Some(wt)) = (field.strip_prefix(b"branch "), current.as_mut())
        {
            let branch = branch.strip_prefix(b"refs/heads/").unwrap_or(branch);
            wt.branch = Some(String::from_utf8_lossy(branch).into_owned());
        } else if let (Some(rest), Some(wt)) = (field.strip_prefix(b"prunable"), current.as_mut()) {
            let reason = rest.strip_prefix(b" ").unwrap_or(rest);
            wt.prunable = Some(String::from_utf8_lossy(reason).into_owned());
        }
    }
    entries.extend(current);
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(path: &str, branch: Option<&str>) -> WorktreeEntry {
        WorktreeEntry {
            path: PathBuf::from(path),
            branch: branch.map(str::to_owned),
            prunable: None,
        }
    }

    #[test]
    fn parses_records_and_strips_refs_heads() {
        let raw = b"worktree /repo\0HEAD abc\0branch refs/heads/main\0\0\
worktree /worktrees/repo/fix\0HEAD def\0branch refs/heads/fix\0\0\
worktree /worktrees/repo/look\0HEAD 123\0detached\0\0";
        assert_eq!(
            parse_porcelain(raw),
            vec![
                wt("/repo", Some("main")),
                wt("/worktrees/repo/fix", Some("fix")),
                wt("/worktrees/repo/look", None),
            ]
        );
    }

    #[test]
    fn keeps_locked_and_prunable_worktrees() {
        let raw = b"worktree /repo\0HEAD abc\0branch refs/heads/main\0\0\
worktree /gone\0HEAD def\0branch refs/heads/gone\0prunable gitdir file points to non-existent location\0\0\
worktree /held\0HEAD 456\0branch refs/heads/held\0locked\0\0";
        let parsed = parse_porcelain(raw);
        assert_eq!(parsed.len(), 3);
        assert_eq!(parsed[1].path, PathBuf::from("/gone"));
        assert_eq!(parsed[1].branch.as_deref(), Some("gone"));
        assert_eq!(
            parsed[1].prunable.as_deref(),
            Some("gitdir file points to non-existent location")
        );
        assert_eq!(parsed[2], wt("/held", Some("held")));
    }

    #[test]
    fn the_primary_checkout_is_never_a_worktree() {
        let fleet = Fleet::from_entries(vec![
            wt("/repo", Some("main")),
            wt("/worktrees/repo/fix", Some("fix")),
        ])
        .unwrap();
        assert_eq!(fleet.primary, wt("/repo", Some("main")));
        assert_eq!(fleet.worktrees.len(), 1);
        assert_eq!(fleet.worktrees[0].name, "fix");
    }

    #[test]
    fn worktrees_sharing_a_basename_stay_distinct_by_path() {
        let fleet = Fleet::from_entries(vec![
            wt("/repo", Some("main")),
            wt("/a/api", Some("api")),
            wt("/b/api", Some("api-2")),
        ])
        .unwrap();
        assert_eq!(fleet.worktrees[0].name, fleet.worktrees[1].name);
        assert_ne!(fleet.worktrees[0].path, fleet.worktrees[1].path);
    }
}
