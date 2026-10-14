//! Edge: the JSON documents `--json` prints. Domain types never derive `Serialize`: each document
//! is a view built here from them, so changing a domain type never silently changes the contract.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::domain::agent::{HolderStatus, StartOutcome, Started, StopOutcome, Stopped};
use crate::domain::branch::Branch;
use crate::domain::contract::{CONTRACT, VERSION};
use crate::domain::error;
use crate::domain::fleet::{Fleet, PrimaryCheckout, Worktree};
use crate::domain::setup::{SetupDone, SetupOutcome, SetupReport};
use crate::domain::state::State;
use crate::domain::worktree::Created;

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

/// One worktree: `agent` is its recorded agent's name or null, `holder` whether a holder is alive.
#[derive(Debug, Serialize)]
struct WorktreeView<'a> {
    name: &'a str,
    path: &'a Path,
    branch: Option<&'a str>,
    state: &'static str,
    detail: Option<String>,
    agent: Option<&'a str>,
    holder: bool,
}

/// `worktree new --json`: the worktree as `worktree ls` shows it, and what was done to make it.
#[derive(Debug, Serialize)]
struct CreatedView<'a> {
    worktree: WorktreeView<'a>,
    start: String,
    copied: usize,
    base: &'a Path,
    setup: SetupView<'a>,
}

/// `worktree setup --json`: the worktree as `worktree ls` shows it, and what its setup did.
#[derive(Debug, Serialize)]
struct SetupDoneView<'a> {
    worktree: WorktreeView<'a>,
    setup: SetupView<'a>,
}

/// What a setup did: `status` is `none` (no script configured), `succeeded` or `failed`, and
/// `log` is the setup log, `null` when no script ran.
#[derive(Debug, Serialize)]
struct SetupView<'a> {
    status: &'static str,
    log: Option<&'a Path>,
}

/// `agent start --json`: the worktree as `worktree ls` shows it, the holder and the sandbox's
/// extra writable paths with the config file that set them (`null` when nothing was launched).
#[derive(Debug, Serialize)]
struct StartedView<'a> {
    worktree: WorktreeView<'a>,
    agent: &'static str,
    holder: HolderView,
    writable: Option<WritableView<'a>>,
}

/// `status` is `started` or `already_running`; `pid` is the holder's, when known.
#[derive(Debug, Serialize)]
struct HolderView {
    status: &'static str,
    pid: Option<u32>,
}

#[derive(Debug, Serialize)]
struct WritableView<'a> {
    paths: Vec<&'a Path>,
    source: Option<&'a str>,
}

/// `agent stop --json`: `status` is `stopped` or `not_running`.
#[derive(Debug, Serialize)]
struct StoppedView<'a> {
    worktree: WorktreeView<'a>,
    status: &'static str,
}

/// Any command's failure with `--json`: `kind` is `other` unless the failure is typed.
#[derive(Debug, Serialize)]
struct ErrorView {
    error: ErrorBody,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    kind: &'static str,
    message: String,
}

/// Wraps `body` in the envelope and renders it.
fn document<T: Serialize>(body: T) -> serde_json::Result<String> {
    serde_json::to_string(&Envelope {
        contract: CONTRACT,
        version: VERSION,
        body,
    })
}

/// The fleet as one JSON document.
pub(crate) fn fleet(fleet: &Fleet) -> serde_json::Result<String> {
    document(FleetView {
        primary: primary_view(&fleet.primary),
        worktrees: fleet.worktrees.iter().map(listed_view).collect(),
    })
}

/// What `worktree new` made, as one JSON document.
pub(crate) fn created(created: &Created) -> serde_json::Result<String> {
    document(CreatedView {
        worktree: worktree_view(
            created.name.as_str(),
            &created.path,
            Some(&created.branch),
            &created.setup.state,
            created.setup.agent.as_deref(),
            HolderStatus::Gone,
        ),
        start: created.start.to_string(),
        copied: created.copied,
        base: &created.base.value,
        setup: setup_view(&created.setup),
    })
}

/// What `worktree setup` did, as one JSON document.
pub(crate) fn set_up(done: &SetupDone) -> serde_json::Result<String> {
    document(SetupDoneView {
        // A setup runs only while no holder is alive.
        worktree: worktree_view(
            &done.name,
            &done.path,
            done.branch.as_ref(),
            &done.setup.state,
            done.setup.agent.as_deref(),
            HolderStatus::Gone,
        ),
        setup: setup_view(&done.setup),
    })
}

/// What `agent start` did, as one JSON document.
pub(crate) fn started(started: &Started) -> serde_json::Result<String> {
    let (status, pid) = match started.outcome {
        StartOutcome::Started { pid } => ("started", Some(pid)),
        StartOutcome::AlreadyRunning { pid } => ("already_running", pid),
    };
    document(StartedView {
        worktree: listed_view(&started.worktree),
        agent: started.agent.name(),
        holder: HolderView { status, pid },
        writable: started.writable.as_ref().map(|writable| WritableView {
            paths: writable.value.iter().map(PathBuf::as_path).collect(),
            source: writable.source.as_deref(),
        }),
    })
}

/// What `agent stop` did, as one JSON document.
pub(crate) fn stopped(stopped: &Stopped) -> serde_json::Result<String> {
    document(StoppedView {
        worktree: listed_view(&stopped.worktree),
        status: match stopped.outcome {
            StopOutcome::Stopped => "stopped",
            StopOutcome::NotRunning => "not_running",
        },
    })
}

/// A failure as one JSON document, with the kind the client tells failures apart by.
pub(crate) fn error(err: &anyhow::Error) -> serde_json::Result<String> {
    document(ErrorView {
        error: ErrorBody {
            kind: error::kind(err),
            message: format!("{err:#}"),
        },
    })
}

fn primary_view(primary: &PrimaryCheckout) -> PrimaryView<'_> {
    PrimaryView {
        path: &primary.path,
        branch: primary.branch.as_ref().map(Branch::as_str),
    }
}

fn listed_view(worktree: &Worktree) -> WorktreeView<'_> {
    worktree_view(
        &worktree.name,
        &worktree.path,
        worktree.branch.as_ref(),
        &worktree.state,
        worktree.agent.as_deref(),
        worktree.holder,
    )
}

fn worktree_view<'a>(
    name: &'a str,
    path: &'a Path,
    branch: Option<&'a Branch>,
    state: &State,
    agent: Option<&'a str>,
    holder: HolderStatus,
) -> WorktreeView<'a> {
    WorktreeView {
        name,
        path,
        branch: branch.map(Branch::as_str),
        state: state_name(state),
        detail: state.detail(),
        agent,
        holder: holder.is_alive(),
    }
}

/// How the contract names a state.
fn state_name(state: &State) -> &'static str {
    match state {
        State::NoAgent => "no_agent",
        State::Idle => "idle",
        State::SettingUp => "setting_up",
        State::NotReady(_) => "not_ready",
        State::Broken(_) => "broken",
    }
}

fn setup_view(setup: &SetupReport) -> SetupView<'_> {
    SetupView {
        status: match setup.outcome {
            SetupOutcome::NoScript => "none",
            SetupOutcome::Succeeded => "succeeded",
            SetupOutcome::Failed(_) => "failed",
        },
        log: setup.log.as_deref(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::domain::agent::Agent;
    use crate::domain::config::Setting;
    use crate::domain::error::{BusyReason, TypedError};
    use crate::domain::fleet::WorktreeEntry;
    use crate::domain::setup::Exit;
    use crate::domain::state::{BrokenReason, NotReadyReason};

    #[test]
    fn the_fleet_document_keeps_its_shape() {
        let mut listed = Fleet::from_entries(vec![
            WorktreeEntry {
                path: PathBuf::from("/repo"),
                branch: Some(Branch::new("main")),
                prunable: None,
            },
            WorktreeEntry {
                path: PathBuf::from("/l/docs"),
                branch: None,
                prunable: Some("gone".to_owned()),
            },
        ])
        .unwrap();
        listed.worktrees[0].state = State::Broken(BrokenReason::DirectoryMissing);
        assert_eq!(
            fleet(&listed).unwrap(),
            format!(
                r#"{{"contract":1,"version":"{VERSION}","primary":{{"path":"/repo","branch":"main"}},"worktrees":[{{"name":"docs","path":"/l/docs","branch":null,"state":"broken","detail":"directory missing; run orca-term worktree prune","agent":null,"holder":false}}]}}"#
            )
        );
    }

    #[test]
    fn setup_states_are_named_in_snake_case_with_their_detail() {
        let failed = State::NotReady(NotReadyReason::Failed {
            exit: Exit::Code(1),
            last_line: Some("boom".to_owned()),
        });
        let view = worktree_view(
            "fix",
            Path::new("/l/fix"),
            None,
            &failed,
            None,
            HolderStatus::Gone,
        );
        assert_eq!(view.state, "not_ready");
        assert_eq!(view.detail.as_deref(), Some("setup failed (exit 1): boom"));
        assert_eq!(state_name(&State::SettingUp), "setting_up");
        assert_eq!(state_name(&State::Idle), "idle");
    }

    fn idle_worktree() -> Worktree {
        Worktree {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
            branch: Some(Branch::new("fix")),
            state: State::Idle,
            agent: Some("claude-code".to_owned()),
            holder: HolderStatus::Alive,
        }
    }

    #[test]
    fn the_start_document_keeps_its_shape() {
        let doc = started(&Started {
            worktree: idle_worktree(),
            agent: Agent::ClaudeCode,
            outcome: StartOutcome::Started { pid: 4242 },
            writable: Some(Setting {
                value: vec![PathBuf::from("/home/me/.cache")],
                source: Some(".git/orca-term.yaml".to_owned()),
                overridden: Vec::new(),
            }),
        })
        .unwrap();
        assert_eq!(
            doc,
            format!(
                r#"{{"contract":1,"version":"{VERSION}","worktree":{{"name":"fix","path":"/l/fix","branch":"fix","state":"idle","detail":null,"agent":"claude-code","holder":true}},"agent":"claude-code","holder":{{"status":"started","pid":4242}},"writable":{{"paths":["/home/me/.cache"],"source":".git/orca-term.yaml"}}}}"#
            )
        );
        let doc: serde_json::Value = serde_json::from_str(
            &started(&Started {
                worktree: idle_worktree(),
                agent: Agent::ClaudeCode,
                outcome: StartOutcome::AlreadyRunning { pid: None },
                writable: None,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(doc["holder"]["status"], "already_running");
        assert_eq!(doc["holder"]["pid"], serde_json::Value::Null);
        assert_eq!(doc["writable"], serde_json::Value::Null);
    }

    #[test]
    fn the_stop_document_keeps_its_shape() {
        let doc: serde_json::Value = serde_json::from_str(
            &stopped(&Stopped {
                worktree: idle_worktree(),
                outcome: StopOutcome::NotRunning,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(doc["status"], "not_running");
        assert_eq!(doc["worktree"]["state"], "idle");
        assert_eq!(doc["worktree"]["agent"], "claude-code");
    }

    #[test]
    fn the_setup_document_keeps_its_shape() {
        let done = SetupDone {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
            branch: Some(Branch::new("fix")),
            setup: SetupReport {
                outcome: SetupOutcome::Succeeded,
                script: None,
                log: Some(PathBuf::from("/a/setup.log")),
                state: State::NoAgent,
                agent: None,
            },
        };
        assert_eq!(
            set_up(&done).unwrap(),
            format!(
                r#"{{"contract":1,"version":"{VERSION}","worktree":{{"name":"fix","path":"/l/fix","branch":"fix","state":"no_agent","detail":null,"agent":null,"holder":false}},"setup":{{"status":"succeeded","log":"/a/setup.log"}}}}"#
            )
        );
    }

    #[test]
    fn an_error_carries_its_kind_and_message() {
        let busy = anyhow::Error::new(TypedError::Busy {
            name: "fix".to_owned(),
            path: PathBuf::from("/l/fix"),
            reason: BusyReason::Locked,
        });
        let doc: serde_json::Value = serde_json::from_str(&error(&busy).unwrap()).unwrap();
        assert_eq!(doc["contract"], 1);
        assert_eq!(doc["error"]["kind"], "busy");
        assert!(
            doc["error"]["message"]
                .as_str()
                .unwrap()
                .contains("is busy")
        );
        let other: serde_json::Value =
            serde_json::from_str(&error(&anyhow::anyhow!("git failed")).unwrap()).unwrap();
        assert_eq!(other["error"]["kind"], "other");
        assert_eq!(other["error"]["message"], "git failed");
    }
}
