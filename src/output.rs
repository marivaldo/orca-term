//! The only module that writes to stdout and stderr.

use std::fmt::Write as _;

use serde::Serialize;

use crate::contract::{CONTRACT, VERSION};
use crate::fleet::Fleet;
use crate::include;
use crate::lane::Created;

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
    let header = ["NAME", "BRANCH", "STATE", "PATH"].map(str::to_owned);
    let rows: Vec<[String; 4]> = fleet
        .lanes
        .iter()
        .map(|lane| {
            [
                lane.name.clone(),
                lane.branch
                    .clone()
                    .unwrap_or_else(|| "(detached)".to_owned()),
                lane.state.label().to_owned(),
                lane.path.display().to_string(),
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

/// Prints what `lane new` made, ending with the `base` it used and where that came from.
pub fn lane_created(created: &Created) {
    out(&render_lane_created(created));
}

fn render_lane_created(created: &Created) -> String {
    let files = if created.copied == 1 { "file" } else { "files" };
    format!(
        "created lane {}\n\
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
    use crate::fleet::{Lane, Worktree};
    use crate::state::State;

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
                state: State::NoAgent,
            },
            Lane {
                name: "docs".to_owned(),
                path: PathBuf::from("/l/docs"),
                branch: None,
                state: State::NoAgent,
            },
        ]));
        assert_eq!(
            table,
            "NAME       BRANCH      STATE     PATH\n\
             fix-login  fix-login   no agent  /l/fix-login\n\
             docs       (detached)  no agent  /l/docs"
        );
    }

    #[test]
    fn lane_new_names_the_base_and_its_source() {
        let text = render_lane_created(&Created {
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
            "created lane fix\n\
             path:   /l/repo/fix\n\
             branch: fix (from main)\n\
             copied: 1 file from .worktreeinclude\n\
             base:   /l (from .git/orca-term.yaml, overriding orca-term.yaml)"
        );
    }
}
