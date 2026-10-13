//! Edge: the only module that writes to stdout and stderr. It renders the text people read; the
//! JSON documents live in `output::json`.
//!
//! Reports and JSON documents go to stdout. Errors go to stderr, or to stdout as a JSON document
//! with `--json`. A command that may run a setup first says on stderr what it starts (`creating
//! worktree fix...`), and the setup script's output follows there live, through the writer
//! [`live_echo`] hands to the command; neither appears with `--json`.

mod json;

use std::fmt::Write as _;
use std::io;
use std::path::PathBuf;
use std::process::ExitCode;

use crate::domain::error;
use crate::domain::fleet::Fleet;
use crate::domain::include;
use crate::domain::setup::{SetupDone, SetupOutcome, SetupReport};
use crate::domain::worktree::{BranchOutcome, Created, Removed, Target, WorktreeName};

/// How a command prints its result and its errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// Text for a person.
    Text,
    /// One JSON document carrying `contract` and `version`, for the client (`--json`).
    Json,
}

impl Format {
    /// The format a command's `--json` flag asks for.
    pub(crate) fn from_json_flag(json: bool) -> Self {
        if json { Self::Json } else { Self::Text }
    }
}

/// Says on stderr that `worktree new` is creating the worktree `name`, before the setup's live
/// output, which may take minutes. Nothing with `--json`.
pub(crate) fn creating_worktree(name: &WorktreeName, format: Format) {
    progress(&format!("creating worktree {name}..."), format);
}

/// Says on stderr that `worktree setup` is running the setup of `target`, before its live output.
/// Nothing with `--json`.
pub(crate) fn running_setup(target: &Target, format: Format) {
    progress(&format!("running the setup of {target}..."), format);
}

fn progress(line: &str, format: Format) {
    if format == Format::Text {
        #[expect(clippy::print_stderr, reason = "this module owns the process's output")]
        {
            eprintln!("{line}");
        }
    }
}

/// Where a setup script's output is echoed while it runs: stderr for a person, nowhere with
/// `--json`, whose stdout carries only the document.
pub(crate) fn live_echo(format: Format) -> Box<dyn io::Write> {
    match format {
        Format::Text => Box::new(io::stderr()),
        Format::Json => Box::new(io::sink()),
    }
}

/// Prints the fleet as one JSON document carrying `contract` and `version`.
pub(crate) fn fleet_json(fleet: &Fleet) -> anyhow::Result<()> {
    out(&json::fleet(fleet)?);
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
                    .as_ref()
                    .map_or_else(|| "(detached)".to_owned(), ToString::to_string),
                worktree.state.label().to_owned(),
                worktree.path.display().to_string(),
            ]
        })
        .collect();
    // The detail column appears only when some worktree has something to say.
    let details: Vec<String> = fleet
        .worktrees
        .iter()
        .map(|worktree| worktree.state.detail().unwrap_or_default())
        .collect();
    let mut lines: Vec<Vec<String>> = [header].into_iter().chain(rows).map(Vec::from).collect();
    if details.iter().any(|detail| !detail.is_empty()) {
        lines[0].push("DETAIL".to_owned());
        for (line, detail) in lines[1..].iter_mut().zip(details) {
            line.push(detail);
        }
    }
    render_columns(&lines)
}

/// Lays `lines` out in columns two spaces apart. The last column is not padded.
fn render_columns(lines: &[Vec<String>]) -> String {
    let columns = lines.first().map_or(0, Vec::len);
    let widths: Vec<usize> = (0..columns)
        .map(|col| lines.iter().map(|line| line[col].len()).max().unwrap_or(0))
        .collect();
    let mut table = String::new();
    for line in lines {
        let mut text = String::new();
        for (col, cell) in line.iter().enumerate() {
            if col + 1 < columns {
                let _ = write!(text, "{cell:width$}  ", width = widths[col]);
            } else {
                text.push_str(cell);
            }
        }
        let _ = writeln!(table, "{}", text.trim_end());
    }
    table.truncate(table.trim_end().len());
    table
}

/// Prints what `worktree new` made, with the `base` it used and where that came from, and what
/// its setup did.
pub(crate) fn worktree_created(created: &Created, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&render_worktree_created(created)),
        Format::Json => out(&json::created(created)?),
    }
    Ok(())
}

fn render_worktree_created(created: &Created) -> String {
    let files = if created.copied == 1 { "file" } else { "files" };
    format!(
        "created worktree {}\n\
         path:   {}\n\
         branch: {} (from {})\n\
         copied: {} {files} from {}\n\
         base:   {}\n\
         {}",
        created.name,
        created.path.display(),
        created.branch,
        created.start,
        created.copied,
        include::FILE_NAME,
        created.base,
        render_setup(created.name.as_str(), &created.setup),
    )
}

/// Prints what `worktree setup` did.
pub(crate) fn worktree_set_up(done: &SetupDone, format: Format) -> anyhow::Result<()> {
    match format {
        Format::Text => out(&render_worktree_set_up(done)),
        Format::Json => out(&json::set_up(done)?),
    }
    Ok(())
}

fn render_worktree_set_up(done: &SetupDone) -> String {
    let readiness = if done.setup.outcome.failed() {
        "is not ready"
    } else {
        "is ready"
    };
    format!(
        "worktree {} {readiness}\n\
         path:   {}\n\
         {}",
        done.name,
        done.path.display(),
        render_setup(&done.name, &done.setup),
    )
}

/// The lines saying what the setup of the worktree `name` did: which script ran and from where,
/// where its log is and, when it failed, why the worktree is not ready and how to try again.
fn render_setup(name: &str, setup: &SetupReport) -> String {
    let from = setup
        .script
        .as_ref()
        .map(|script| format!(" (script {})", script.origin()))
        .unwrap_or_default();
    let log = setup
        .log
        .as_ref()
        .map(|log| format!("\nlog:    {}", log.display()))
        .unwrap_or_default();
    match setup.outcome {
        SetupOutcome::NoScript => {
            "setup:  nothing to run, no setup script is configured".to_owned()
        }
        SetupOutcome::Succeeded => format!("setup:  succeeded{from}{log}"),
        SetupOutcome::Failed(_) => format!(
            "setup:  failed{from}{log}\n\
             not ready: {}\n\
             fix the cause, then run `orca-term worktree setup {name}`",
            setup.state.detail().unwrap_or_default()
        ),
    }
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
pub(crate) fn worktrees_pruned(pruned: &[PathBuf]) {
    out(&render_worktrees_pruned(pruned));
}

fn render_worktrees_pruned(pruned: &[PathBuf]) -> String {
    if pruned.is_empty() {
        return "nothing to prune".to_owned();
    }
    pruned
        .iter()
        .map(|path| format!("pruned {}", path.display()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Prints an error, with its causes: to stderr as text, or to stdout as a JSON document carrying
/// its `kind`. Returns the exit code the error maps to: its own for a typed error, 1 otherwise.
pub fn error(err: &anyhow::Error, format: Format) -> ExitCode {
    let document = match format {
        Format::Json => json::error(err).ok(),
        Format::Text => None,
    };
    match document {
        Some(document) => out(&document),
        None => {
            #[expect(clippy::print_stderr, reason = "this module owns the process's output")]
            {
                eprintln!("orca-term: {err:#}");
            }
        }
    }
    ExitCode::from(error::exit_code(err))
}

fn out(text: &str) {
    #[expect(clippy::print_stdout, reason = "this module owns the process's output")]
    {
        println!("{text}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::branch::{Branch, StartPoint};
    use crate::domain::config::Setting;
    use crate::domain::fleet::{PrimaryCheckout, Worktree};
    use crate::domain::setup::Exit;
    use crate::domain::state::{BrokenReason, NotReadyReason, State};
    use crate::domain::worktree::KeptReason;

    fn no_setup() -> SetupReport {
        SetupReport {
            outcome: SetupOutcome::NoScript,
            script: None,
            log: None,
            state: State::NoAgent,
        }
    }

    fn failed_setup() -> SetupReport {
        SetupReport {
            outcome: SetupOutcome::Failed(Exit::Code(1)),
            script: Some(Setting {
                value: "make".to_owned(),
                source: Some("orca-term.yaml".to_owned()),
                overridden: Vec::new(),
            }),
            log: Some(PathBuf::from("/a/setup.log")),
            state: State::NotReady(NotReadyReason::Failed {
                exit: Exit::Code(1),
                last_line: Some("boom".to_owned()),
            }),
        }
    }

    fn fleet(worktrees: Vec<Worktree>) -> Fleet {
        Fleet {
            primary: PrimaryCheckout {
                path: PathBuf::from("/repo"),
                branch: Some(Branch::new("main")),
            },
            worktrees,
        }
    }

    #[test]
    fn an_empty_fleet_says_so() {
        assert_eq!(render_fleet_table(&fleet(vec![])), "no worktrees in /repo");
    }

    #[test]
    fn the_detail_column_appears_only_when_a_worktree_has_one() {
        let worktree = |name: &str, state| Worktree {
            name: name.to_owned(),
            path: PathBuf::from(format!("/l/{name}")),
            branch: Some(Branch::new(name)),
            state,
        };
        assert_eq!(
            render_fleet_table(&fleet(vec![worktree("a", State::NoAgent)])),
            "NAME  BRANCH  STATE     PATH\n\
             a     a       no agent  /l/a"
        );
        assert_eq!(
            render_fleet_table(&fleet(vec![
                worktree("a", State::SettingUp),
                worktree("bb", failed_setup().state),
            ])),
            "NAME  BRANCH  STATE       PATH   DETAIL\n\
             a     a       setting up  /l/a\n\
             bb    bb      not ready   /l/bb  setup failed (exit 1): boom"
        );
    }

    #[test]
    fn columns_align() {
        let table = render_fleet_table(&fleet(vec![
            Worktree {
                name: "fix-login".to_owned(),
                path: PathBuf::from("/l/fix-login"),
                branch: Some(Branch::new("fix-login")),
                state: State::NoAgent,
            },
            Worktree {
                name: "docs".to_owned(),
                path: PathBuf::from("/l/docs"),
                branch: None,
                state: State::Broken(BrokenReason::DirectoryMissing),
            },
        ]));
        assert_eq!(
            table,
            "NAME       BRANCH      STATE     PATH          DETAIL\n\
             fix-login  fix-login   no agent  /l/fix-login\n\
             docs       (detached)  broken    /l/docs       directory missing; run orca-term worktree prune"
        );
    }

    #[test]
    fn worktree_new_names_the_base_and_its_source() {
        let text = render_worktree_created(&Created {
            name: "fix".parse().unwrap(),
            path: PathBuf::from("/l/repo/fix"),
            branch: Branch::new("fix"),
            start: StartPoint::Local(Branch::new("main")),
            copied: 1,
            base: Setting {
                value: PathBuf::from("/l"),
                source: Some(".git/orca-term.yaml".to_owned()),
                overridden: vec!["orca-term.yaml".to_owned()],
            },
            setup: no_setup(),
        });
        assert_eq!(
            text,
            "created worktree fix\n\
             path:   /l/repo/fix\n\
             branch: fix (from main)\n\
             copied: 1 file from .worktreeinclude\n\
             base:   /l (from .git/orca-term.yaml, overriding orca-term.yaml)\n\
             setup:  nothing to run, no setup script is configured"
        );
    }

    #[test]
    fn a_failed_setup_says_where_the_log_is_and_how_to_try_again() {
        assert_eq!(
            render_setup("fix", &failed_setup()),
            "setup:  failed (script from orca-term.yaml)\n\
             log:    /a/setup.log\n\
             not ready: setup failed (exit 1): boom\n\
             fix the cause, then run `orca-term worktree setup fix`"
        );
        let done = SetupDone {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
            branch: None,
            setup: failed_setup(),
        };
        assert!(
            render_worktree_set_up(&done)
                .starts_with("worktree fix is not ready\npath:   /l/fix\n")
        );
    }

    #[test]
    fn a_successful_setup_names_its_script_and_its_log() {
        let setup = SetupReport {
            outcome: SetupOutcome::Succeeded,
            log: Some(PathBuf::from("/a/setup.log")),
            state: State::NoAgent,
            ..failed_setup()
        };
        assert_eq!(
            render_setup("fix", &setup),
            "setup:  succeeded (script from orca-term.yaml)\nlog:    /a/setup.log"
        );
    }

    #[test]
    fn an_error_maps_to_its_exit_code() {
        use crate::domain::error::TypedError;
        let busy = anyhow::Error::new(TypedError::Busy {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
        });
        assert_eq!(error(&busy, Format::Json), ExitCode::from(3));
        assert_eq!(
            error(&anyhow::anyhow!("x"), Format::Text),
            ExitCode::FAILURE
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
                branch: Branch::new("fix"),
                reason: KeptReason::NotMerged {
                    into: StartPoint::Local(Branch::new("main")),
                },
            })),
            "removed worktree fix\npath:   /l/repo/fix\nkept branch fix: not merged into main"
        );
        assert_eq!(
            render_worktree_removed(&removed(BranchOutcome::DeletedMerged {
                branch: Branch::new("fix"),
                into: StartPoint::Local(Branch::new("main")),
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
