//! App: what the side menu says about the projects and their worktrees, read from every project's
//! fleet (ADR 0011): the order the worktrees come in, the counts the status bar shows, and each
//! worktree's words (its branch, its pull request, its last message and how long ago).

use std::path::{Path, PathBuf};

use crate::domain::fleet::Worktree;
use crate::domain::pr::{Checks, Found};
use crate::domain::project::ProjectFleet;
use crate::domain::state::State;

/// Every worktree's path, in the side menu's order: the projects in the list's order, each one's
/// worktrees as its fleet lists them. A project that cannot be read has none.
pub(crate) fn order(fleets: &[ProjectFleet]) -> Vec<PathBuf> {
    fleets
        .iter()
        .filter_map(|project| project.fleet.as_ref().ok())
        .flat_map(|fleet| fleet.worktrees.iter().map(|worktree| worktree.path.clone()))
        .collect()
}

/// The worktree at `path` and the project it belongs to.
pub(crate) fn find<'a>(
    fleets: &'a [ProjectFleet],
    path: &Path,
) -> Option<(&'a ProjectFleet, &'a Worktree)> {
    fleets.iter().find_map(|project| {
        let fleet = project.fleet.as_ref().ok()?;
        let worktree = fleet
            .worktrees
            .iter()
            .find(|worktree| worktree.path == path)?;
        Some((project, worktree))
    })
}

/// The worktree `by` places after (or, negative, before) the one at `from` in `order`, wrapping
/// at either end; the first one when `from` is not in it.
pub(crate) fn step(order: &[PathBuf], from: Option<&Path>, by: isize) -> Option<PathBuf> {
    let at = from.and_then(|from| order.iter().position(|path| path == from));
    let Some(at) = at else {
        return order.first().cloned();
    };
    let len = isize::try_from(order.len()).ok()?;
    let at = isize::try_from(at).ok()?;
    let next = usize::try_from((at + by).rem_euclid(len)).ok()?;
    order.get(next).cloned()
}

/// A project's name: its primary checkout's directory name.
pub(crate) fn project_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

/// How many worktrees, over every project, need the person, failed, are running, or are quiet
/// (everything else).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Counts {
    pub(crate) needs_you: usize,
    pub(crate) failed: usize,
    pub(crate) running: usize,
    pub(crate) quiet: usize,
}

pub(crate) fn counts(fleets: &[ProjectFleet]) -> Counts {
    let mut counts = Counts::default();
    for worktree in worktrees(fleets) {
        match worktree.state {
            State::Waiting => counts.needs_you += 1,
            State::Failed => counts.failed += 1,
            State::Working => counts.running += 1,
            _ => counts.quiet += 1,
        }
    }
    counts
}

/// Whether any worktree is working, so the spinner turns.
pub(crate) fn any_working(fleets: &[ProjectFleet]) -> bool {
    worktrees(fleets).any(|worktree| worktree.state == State::Working)
}

fn worktrees(fleets: &[ProjectFleet]) -> impl Iterator<Item = &Worktree> {
    fleets
        .iter()
        .filter_map(|project| project.fleet.as_ref().ok())
        .flat_map(|fleet| fleet.worktrees.iter())
}

/// The number of the worktree's pull request, when `pr refresh` found one.
pub(crate) fn pr_number(worktree: &Worktree) -> Option<u64> {
    match &worktree.pr.as_ref()?.found {
        Found::Pr(pr) => Some(pr.number),
        Found::NoPr | Found::Unavailable(_) => None,
    }
}

/// The checks of the worktree's pull request, when it has one.
pub(crate) fn checks(worktree: &Worktree) -> Option<Checks> {
    match &worktree.pr.as_ref()?.found {
        Found::Pr(pr) => Some(pr.checks),
        Found::NoPr | Found::Unavailable(_) => None,
    }
}

/// The name an agent goes by on screen: `claude` for claude-code, any other by its own name.
pub(crate) fn agent_label(agent: &str) -> &str {
    if agent == "claude-code" {
        "claude"
    } else {
        agent
    }
}

/// What the worktree's agent said or is doing last, or else its state in words.
pub(crate) fn message(worktree: &Worktree) -> String {
    worktree
        .activity
        .clone()
        .unwrap_or_else(|| worktree.state.label().to_owned())
}

/// How long ago `then_ms` was at `now_ms` (both in milliseconds since the Unix epoch), short:
/// `now` under a minute, then `9m`, `3h`, `2d`, as the Neovim client's sidebar says it.
pub(crate) fn age(then_ms: u64, now_ms: u64) -> String {
    let minutes = now_ms.saturating_sub(then_ms) / 60_000;
    match minutes {
        0 => "now".to_owned(),
        1..60 => format!("{minutes}m"),
        60..1440 => format!("{}h", minutes / 60),
        _ => format!("{}d", minutes / 1440),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_age_reads_in_the_largest_unit() {
        let now = 10 * 86_400_000;
        assert_eq!(age(now - 59_000, now), "now");
        assert_eq!(age(now - 9 * 60_000, now), "9m");
        assert_eq!(age(now - 3 * 3_600_000, now), "3h");
        assert_eq!(age(now - 2 * 86_400_000, now), "2d");
        assert_eq!(age(now + 5_000, now), "now", "a clock that went back");
    }

    #[test]
    fn stepping_wraps_at_either_end() {
        let order = [
            PathBuf::from("/a"),
            PathBuf::from("/b"),
            PathBuf::from("/c"),
        ];
        let at = |path: &str| Some(PathBuf::from(path));
        assert_eq!(step(&order, Some(Path::new("/a")), 1), at("/b"));
        assert_eq!(step(&order, Some(Path::new("/c")), 1), at("/a"));
        assert_eq!(step(&order, Some(Path::new("/a")), -1), at("/c"));
        assert_eq!(step(&order, Some(Path::new("/x")), 1), at("/a"));
        assert_eq!(step(&order, None, -1), at("/a"));
        assert_eq!(step(&[], None, 1), None);
    }

    #[test]
    fn claude_code_goes_by_claude() {
        assert_eq!(agent_label("claude-code"), "claude");
        assert_eq!(agent_label("opencode"), "opencode");
    }
}
