//! The fleet: every lane of one repository, derived from git itself.
//!
//! The fleet is `git worktree list` minus the primary checkout, which git always lists first. A
//! lane is keyed by its worktree path and named by that directory's basename.

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde::Serialize;

use crate::git;
use crate::state::{self, State};

/// A worktree as git reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Worktree {
    pub path: PathBuf,
    /// The checked-out branch, without `refs/heads/`. `None` when detached or bare.
    pub branch: Option<String>,
    /// Why git marks the worktree prunable, when it does: its directory or `.git` file is gone.
    #[serde(skip)]
    pub prunable: Option<String>,
}

/// One lane of the fleet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Lane {
    pub name: String,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub state: State,
    /// Why the lane is in its state, when there is something to say.
    pub detail: Option<String>,
    /// Whether the lane's worktree is gone, so only `lane prune` can clean it.
    #[serde(skip)]
    pub gone: bool,
}

/// The primary checkout and every lane of its repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Fleet {
    pub primary: Worktree,
    pub lanes: Vec<Lane>,
}

impl Fleet {
    /// Reads the fleet of the repository containing `dir`. Never writes.
    pub fn discover(dir: &Path) -> Result<Self> {
        let worktrees = list_worktrees(dir)?;
        let prunable: Vec<Option<String>> = worktrees
            .iter()
            .skip(1)
            .map(|wt| wt.prunable.clone())
            .collect();
        let mut fleet = Self::from_worktrees(worktrees)?;
        for (lane, prunable) in fleet.lanes.iter_mut().zip(prunable) {
            lane.read_state(prunable);
        }
        Ok(fleet)
    }

    /// The lanes whose worktree is gone: the ones `lane prune` would clean.
    pub fn gone_lanes(&self) -> impl Iterator<Item = &Lane> {
        self.lanes.iter().filter(|lane| lane.gone)
    }

    fn from_worktrees(worktrees: Vec<Worktree>) -> Result<Self> {
        let mut worktrees = worktrees.into_iter();
        let Some(primary) = worktrees.next() else {
            bail!("git listed no worktrees");
        };
        let lanes = worktrees
            .map(|wt| Lane {
                name: lane_name(&wt.path),
                path: wt.path,
                branch: wt.branch,
                state: State::NoAgent,
                detail: None,
                gone: false,
            })
            .collect();
        Ok(Self { primary, lanes })
    }
}

impl Lane {
    /// Reads the lane's state. A lane whose directory is gone, or whose state cannot be read, is
    /// broken, and the reason goes in `detail`: reading one lane never fails the fleet.
    fn read_state(&mut self, prunable: Option<String>) {
        if !self.path.is_dir() || prunable.is_some() {
            self.state = State::Broken;
            self.gone = true;
            self.detail = Some(if self.path.is_dir() {
                let reason = prunable.unwrap_or_default();
                format!("git marks it prunable ({reason}); run orca-term lane prune")
            } else {
                "directory missing; run orca-term lane prune".to_owned()
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

/// The primary checkout of the repository containing `dir`, without reading any lane. Never writes.
pub fn primary_checkout(dir: &Path) -> Result<Worktree> {
    match list_worktrees(dir)?.into_iter().next() {
        Some(primary) => Ok(primary),
        None => bail!("git listed no worktrees"),
    }
}

fn list_worktrees(dir: &Path) -> Result<Vec<Worktree>> {
    let porcelain = git::output(dir, &["worktree", "list", "--porcelain", "-z"])?;
    Ok(parse_porcelain(&porcelain))
}

fn lane_name(path: &Path) -> String {
    path.file_name()
        .map_or_else(|| path.to_string_lossy(), OsStr::to_string_lossy)
        .into_owned()
}

/// Parses `git worktree list --porcelain -z`: attribute lines end in NUL, and an empty attribute
/// line ends each record.
fn parse_porcelain(raw: &[u8]) -> Vec<Worktree> {
    let mut worktrees = Vec::new();
    let mut current: Option<Worktree> = None;
    for field in raw.split(|&b| b == 0) {
        if field.is_empty() {
            worktrees.extend(current.take());
        } else if let Some(path) = field.strip_prefix(b"worktree ") {
            worktrees.extend(current.take());
            current = Some(Worktree {
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
    worktrees.extend(current);
    worktrees
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wt(path: &str, branch: Option<&str>) -> Worktree {
        Worktree {
            path: PathBuf::from(path),
            branch: branch.map(str::to_owned),
            prunable: None,
        }
    }

    #[test]
    fn parses_records_and_strips_refs_heads() {
        let raw = b"worktree /repo\0HEAD abc\0branch refs/heads/main\0\0\
worktree /lanes/repo/fix\0HEAD def\0branch refs/heads/fix\0\0\
worktree /lanes/repo/look\0HEAD 123\0detached\0\0";
        assert_eq!(
            parse_porcelain(raw),
            vec![
                wt("/repo", Some("main")),
                wt("/lanes/repo/fix", Some("fix")),
                wt("/lanes/repo/look", None),
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
    fn the_primary_checkout_is_never_a_lane() {
        let fleet = Fleet::from_worktrees(vec![
            wt("/repo", Some("main")),
            wt("/lanes/repo/fix", Some("fix")),
        ])
        .unwrap();
        assert_eq!(fleet.primary, wt("/repo", Some("main")));
        assert_eq!(fleet.lanes.len(), 1);
        assert_eq!(fleet.lanes[0].name, "fix");
    }

    #[test]
    fn lanes_sharing_a_basename_stay_distinct_by_path() {
        let fleet = Fleet::from_worktrees(vec![
            wt("/repo", Some("main")),
            wt("/a/api", Some("api")),
            wt("/b/api", Some("api-2")),
        ])
        .unwrap();
        assert_eq!(fleet.lanes[0].name, fleet.lanes[1].name);
        assert_ne!(fleet.lanes[0].path, fleet.lanes[1].path);
    }
}
