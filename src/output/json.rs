//! Edge: the JSON documents `--json` prints. Domain types never derive `Serialize`: each document
//! is a view built here from them, so changing a domain type never silently changes the contract.

use std::path::Path;

use serde::Serialize;

use crate::domain::contract::{CONTRACT, VERSION};
use crate::domain::fleet::{Fleet, Worktree, WorktreeEntry};
use crate::domain::state::State;

/// Every document carries `contract` and `version` next to its own fields.
#[derive(Debug, Serialize)]
struct Envelope<'a, T: Serialize> {
    contract: u32,
    version: &'a str,
    #[serde(flatten)]
    body: T,
}

/// `worktree ls --json`.
#[derive(Debug, Serialize)]
struct FleetView<'a> {
    primary: PrimaryView<'a>,
    worktrees: Vec<WorktreeView<'a>>,
}

#[derive(Debug, Serialize)]
struct PrimaryView<'a> {
    path: &'a Path,
    branch: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct WorktreeView<'a> {
    name: &'a str,
    path: &'a Path,
    branch: Option<&'a str>,
    state: &'static str,
    detail: Option<&'a str>,
}

/// The fleet as one JSON document.
pub(crate) fn fleet(fleet: &Fleet) -> serde_json::Result<String> {
    serde_json::to_string(&Envelope {
        contract: CONTRACT,
        version: VERSION,
        body: FleetView {
            primary: primary_view(&fleet.primary),
            worktrees: fleet.worktrees.iter().map(worktree_view).collect(),
        },
    })
}

fn primary_view(entry: &WorktreeEntry) -> PrimaryView<'_> {
    PrimaryView {
        path: &entry.path,
        branch: entry.branch.as_deref(),
    }
}

fn worktree_view(worktree: &Worktree) -> WorktreeView<'_> {
    WorktreeView {
        name: &worktree.name,
        path: &worktree.path,
        branch: worktree.branch.as_deref(),
        state: state_name(worktree.state),
        detail: worktree.detail.as_deref(),
    }
}

/// How the contract names a state.
fn state_name(state: State) -> &'static str {
    match state {
        State::NoAgent => "no_agent",
        State::Broken => "broken",
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn the_fleet_document_keeps_its_shape() {
        let mut listed = Fleet::from_entries(vec![
            WorktreeEntry {
                path: PathBuf::from("/repo"),
                branch: Some("main".to_owned()),
                prunable: None,
            },
            WorktreeEntry {
                path: PathBuf::from("/l/docs"),
                branch: None,
                prunable: Some("gone".to_owned()),
            },
        ])
        .unwrap();
        listed.worktrees[0].mark_gone("directory missing".to_owned());
        assert_eq!(
            fleet(&listed).unwrap(),
            format!(
                r#"{{"contract":1,"version":"{VERSION}","primary":{{"path":"/repo","branch":"main"}},"worktrees":[{{"name":"docs","path":"/l/docs","branch":null,"state":"broken","detail":"directory missing"}}]}}"#
            )
        );
    }
}
