//! The only module that writes to stdout and stderr.

use std::fmt::Write as _;

use serde::Serialize;

use crate::contract::{CONTRACT, VERSION};
use crate::fleet::Fleet;
use crate::include;
use crate::worktree::{BranchOutcome, Created, Removed};

#[derive(Debug, Serialize)]
struct Envelope<'a, T: Serialize> {
    contract: u32,
    version: &'a str,
    #[serde(flatten)]
    body: &'a T,
}

/// Prints `body` as one JSON document carrying `contract` and `version`.
pub(crate) fn json<T: Serialize>(body: &T) -> anyhow::Result<()> {
    let doc = serde_json::to_string(&Envelope {
        contract: CONTRACT,
        version: VERSION,
        body,
    })?;
    out(&doc);
    Ok(())
}

/// Prints the fleet as an aligned table, one worktree per line.
pub(crate) fn fleet_table(fleet: &Fleet) {
    out(&render_fleet_table(fleet));
}

fn render_fleet_table(fleet: &Fleet) -> String {
    if fleet.worktrees.is_empty() {
        return format!("no worktrees in {}", fleet.primary.path.display());
    }
    let header = ["NAME", "BRANCH", "STATE", "PATH"].map(str::to_owned);
    let rows: Vec<[String; 4]> = fleet
        .worktrees
        .iter()
        .map(|worktree| {
            [
                worktree.name.clone(),
                worktree
                    .branch
                    .clone()
                    .unwrap_or_else(|| "(detached)".to_owned()),
                worktree.state.label().to_owned(),
                worktree.path.display().to_string(),
            ]
        })
        .collect();
    let width = |col: usize| {
        rows.iter()
            .chain([&header])
            .map(|r| r[col].len())
            .max()
            .unwrap_or(0)
    };
    let (name_w, branch_w, state_w) = (width(0), width(1), width(2));
    let mut table = String::new();
    for row in [&header].into_iter().chain(&rows) {
        let _ = writeln!(
            table,
            "{:name_w$}  {:branch_w$}  {:state_w$}  {}",
            row[0], row[1], row[2], row[3]
        );
    }
    table.truncate(table.trim_end().len());
    table
}

/// Prints what `worktree new` made, ending with the `base` it used and where that came from.
pub(crate) fn worktree_created(created: &Created) {
    out(&render_worktree_created(created));
}

fn render_worktree_created(created: &Created) -> String {
    let files = if created.copied == 1 { "file" } else { "files" };
    format!(
        "created worktree {}\n\
         path:   {}\n\
         branch: {} (from {})\n\
         copied: {} {files} from {}\n\
         base:   {}",
        created.name,
        created.path.display(),
        created.branch,
        created.start,
        created.copied,
        include::FILE_NAME,
        created.base,
    )
}

/// Prints what `worktree rm` removed and what happened to the worktree's branch.
pub(crate) fn worktree_removed(removed: &Removed) {
    out(&render_worktree_removed(removed));
}

fn render_worktree_removed(removed: &Removed) -> String {
    let branch = match &removed.branch {
        BranchOutcome::DeletedMerged { branch, into } => {
            format!("deleted branch {branch} (merged into {into})")
        }
        BranchOutcome::DeletedForced { branch } => format!("deleted branch {branch} (--force)"),
        BranchOutcome::Kept { branch, reason } => format!("kept branch {branch}: {reason}"),
        BranchOutcome::Detached => "no branch to delete: the worktree was detached".to_owned(),
    };
    format!(
        "removed worktree {}\n\
         path:   {}\n\
         {branch}",
        removed.name,
        removed.path.display(),
    )
}

/// Prints the worktrees `worktree prune` cleaned, one path per line, or that there was nothing to prune.
pub(crate) fn worktrees_pruned(pruned: &[std::path::PathBuf]) {
    out(&render_worktrees_pruned(pruned));
}

fn render_worktrees_pruned(pruned: &[std::path::PathBuf]) -> String {
    if pruned.is_empty() {
        return "nothing to prune".to_owned();
    }
    pruned
        .iter()
        .map(|path| format!("pruned {}", path.display()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Prints an error, with its causes, to stderr.
pub fn error(err: &anyhow::Error) {
    #[expect(clippy::print_stderr, reason = "this module owns the process's output")]
    {
        eprintln!("orca-term: {err:#}");
    }
}

fn out(text: &str) {
    #[expect(clippy::print_stdout, reason = "this module owns the process's output")]
    {
        println!("{text}");
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::config::Setting;
    use crate::fleet::{Worktree, WorktreeEntry};
    use crate::state::State;

    fn fleet(worktrees: Vec<Worktree>) -> Fleet {
        Fleet {
            primary: WorktreeEntry {
                path: PathBuf::from("/repo"),
                branch: Some("main".to_owned()),
                prunable: None,
            },
            worktrees,
        }
    }

    #[test]
    fn an_empty_fleet_says_so() {
        assert_eq!(render_fleet_table(&fleet(vec![])), "no worktrees in /repo");
    }

    #[test]
    fn columns_align() {
        let table = render_fleet_table(&fleet(vec![
            Worktree {
                name: "fix-login".to_owned(),
                path: PathBuf::from("/l/fix-login"),
                branch: Some("fix-login".to_owned()),
                state: State::NoAgent,
                detail: None,
                gone: false,
            },
            Worktree {
                name: "docs".to_owned(),
                path: PathBuf::from("/l/docs"),
                branch: None,
                state: State::Broken,
                detail: Some("directory missing; run orca-term worktree prune".to_owned()),
                gone: true,
            },
        ]));
        assert_eq!(
            table,
            "NAME       BRANCH      STATE     PATH\n\
             fix-login  fix-login   no agent  /l/fix-login\n\
             docs       (detached)  broken    /l/docs"
        );
    }

    #[test]
    fn worktree_new_names_the_base_and_its_source() {
        let text = render_worktree_created(&Created {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/repo/fix"),
            branch: "fix".to_owned(),
            start: "main".to_owned(),
            copied: 1,
            base: Setting {
                value: PathBuf::from("/l"),
                source: Some(".git/orca-term.yaml".to_owned()),
                overridden: vec!["orca-term.yaml".to_owned()],
            },
        });
        assert_eq!(
            text,
            "created worktree fix\n\
             path:   /l/repo/fix\n\
             branch: fix (from main)\n\
             copied: 1 file from .worktreeinclude\n\
             base:   /l (from .git/orca-term.yaml, overriding orca-term.yaml)"
        );
    }

    #[test]
    fn worktree_rm_says_what_happened_to_the_branch() {
        let removed = |branch| Removed {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/repo/fix"),
            branch,
        };
        assert_eq!(
            render_worktree_removed(&removed(BranchOutcome::Kept {
                branch: "fix".to_owned(),
                reason: "not merged into main".to_owned(),
            })),
            "removed worktree fix\npath:   /l/repo/fix\nkept branch fix: not merged into main"
        );
        assert_eq!(
            render_worktree_removed(&removed(BranchOutcome::DeletedMerged {
                branch: "fix".to_owned(),
                into: "main".to_owned(),
            })),
            "removed worktree fix\npath:   /l/repo/fix\ndeleted branch fix (merged into main)"
        );
    }

    #[test]
    fn worktree_prune_lists_paths_or_says_nothing() {
        assert_eq!(render_worktrees_pruned(&[]), "nothing to prune");
        assert_eq!(
            render_worktrees_pruned(&[PathBuf::from("/a"), PathBuf::from("/b")]),
            "pruned /a\npruned /b"
        );
    }
}
