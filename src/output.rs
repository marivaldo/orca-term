//! The only module that writes to stdout and stderr.

use std::fmt::Write as _;

use serde::Serialize;

use crate::contract::{CONTRACT, VERSION};
use crate::fleet::Fleet;

#[derive(Debug, Serialize)]
struct Envelope<'a, T: Serialize> {
    contract: u32,
    version: &'a str,
    #[serde(flatten)]
    body: &'a T,
}

/// Prints `body` as one JSON document carrying `contract` and `version`.
pub fn json<T: Serialize>(body: &T) -> anyhow::Result<()> {
    let doc = serde_json::to_string(&Envelope {
        contract: CONTRACT,
        version: VERSION,
        body,
    })?;
    out(&doc);
    Ok(())
}

/// Prints the fleet as an aligned table, one lane per line.
pub fn fleet_table(fleet: &Fleet) {
    out(&render_fleet_table(fleet));
}

fn render_fleet_table(fleet: &Fleet) -> String {
    if fleet.lanes.is_empty() {
        return format!("no lanes in {}", fleet.primary.path.display());
    }
    let rows: Vec<[String; 3]> = fleet
        .lanes
        .iter()
        .map(|lane| {
            [
                lane.name.clone(),
                lane.branch
                    .clone()
                    .unwrap_or_else(|| "(detached)".to_owned()),
                lane.path.display().to_string(),
            ]
        })
        .collect();
    let header = ["NAME".to_owned(), "BRANCH".to_owned(), "PATH".to_owned()];
    let name_w = rows
        .iter()
        .chain([&header])
        .map(|r| r[0].len())
        .max()
        .unwrap_or(0);
    let branch_w = rows
        .iter()
        .chain([&header])
        .map(|r| r[1].len())
        .max()
        .unwrap_or(0);
    let mut table = String::new();
    for row in [&header].into_iter().chain(&rows) {
        let _ = writeln!(
            table,
            "{:name_w$}  {:branch_w$}  {}",
            row[0], row[1], row[2]
        );
    }
    table.truncate(table.trim_end().len());
    table
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
    use crate::fleet::{Lane, Worktree};

    fn fleet(lanes: Vec<Lane>) -> Fleet {
        Fleet {
            primary: Worktree {
                path: PathBuf::from("/repo"),
                branch: Some("main".to_owned()),
            },
            lanes,
        }
    }

    #[test]
    fn an_empty_fleet_says_so() {
        assert_eq!(render_fleet_table(&fleet(vec![])), "no lanes in /repo");
    }

    #[test]
    fn columns_align() {
        let table = render_fleet_table(&fleet(vec![
            Lane {
                name: "fix-login".to_owned(),
                path: PathBuf::from("/l/fix-login"),
                branch: Some("fix-login".to_owned()),
            },
            Lane {
                name: "docs".to_owned(),
                path: PathBuf::from("/l/docs"),
                branch: None,
            },
        ]));
        assert_eq!(
            table,
            "NAME       BRANCH      PATH\n\
             fix-login  fix-login   /l/fix-login\n\
             docs       (detached)  /l/docs"
        );
    }
}
