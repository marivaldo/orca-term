//! Domain: a worktree's state, kept as `worktree.json` in git's per-worktree admin directory
//! (ADR 0003).
//!
//! Every state file carries `schema`. Reading tolerates any schema, taking the fields it knows and
//! ignoring the rest; writing refuses a file whose schema is newer than this core supports, so an
//! older core never clobbers what a newer one wrote.
//!
//! A worktree's state is derived on read from what `worktree.json` records and from volatile
//! facts checked at that moment: whether the worktree's lock is held, and the setup log. Nothing
//! is rewritten on the read path.

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::domain::setup::{self, Exit, SetupRecord};

/// The newest schema this core reads and writes.
pub(crate) const SUPPORTED_SCHEMA: u64 = 1;

/// The worktree's state file, inside its admin directory.
const WORKTREE_FILE: &str = "worktree.json";

/// The worktree's lock file, inside its admin directory. Whoever holds its lock may write the
/// worktree's state.
const LOCK_FILE: &str = "lock";

/// The directory, inside git's per-worktree admin directory, that holds the worktree's state.
pub(crate) const ADMIN_DIR_NAME: &str = "orca-term";

/// The directory, inside git's per-worktree admin directory, that holds one worktree's state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AdminDir(PathBuf);

impl AdminDir {
    /// The admin directory git reported at `path`.
    pub(crate) fn new(path: PathBuf) -> Self {
        Self(path)
    }

    /// Where the directory is.
    pub(crate) fn path(&self) -> &Path {
        &self.0
    }

    /// The worktree's state file, `worktree.json`, inside this directory.
    pub(crate) fn state_file(&self) -> PathBuf {
        self.0.join(WORKTREE_FILE)
    }

    /// The output of the worktree's latest setup, `setup.log`, inside this directory.
    pub(crate) fn setup_log(&self) -> PathBuf {
        self.0.join(setup::LOG_FILE)
    }

    /// The worktree's lock file, `lock`, inside this directory.
    pub(crate) fn lock_file(&self) -> PathBuf {
        self.0.join(LOCK_FILE)
    }
}

/// Whether a worktree's lock is held right now, by this or another process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockStatus {
    Held,
    Free,
}

/// What a worktree is doing, as the fleet shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum State {
    /// The worktree is ready, and no agent is attached to it.
    NoAgent,
    /// The worktree's setup is running.
    SettingUp,
    /// The worktree exists but its setup did not finish, for the reason given; it stays so until
    /// `orca-term worktree setup` succeeds.
    NotReady(NotReadyReason),
    /// The worktree cannot be used, for the reason given.
    Broken(BrokenReason),
}

impl State {
    /// How a table shows the state.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            Self::NoAgent => "no agent",
            Self::SettingUp => "setting up",
            Self::NotReady(_) => "not ready",
            Self::Broken(_) => "broken",
        }
    }

    /// Why the worktree is in its state, when there is something to say.
    pub(crate) fn detail(&self) -> Option<String> {
        match self {
            Self::NoAgent | Self::SettingUp => None,
            Self::NotReady(reason) => Some(reason.to_string()),
            Self::Broken(reason) => Some(reason.to_string()),
        }
    }
}

/// Why a worktree is not ready.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NotReadyReason {
    /// Its setup is recorded as running but nobody holds the worktree's lock: the process that ran
    /// it died before recording the end.
    Interrupted,
    /// Its setup failed, as `exit` says, and `last_line` is the last line of its log, if any.
    Failed {
        exit: Exit,
        last_line: Option<String>,
    },
}

/// The reason as the fleet shows it, in the table's detail and in the JSON `detail`.
impl fmt::Display for NotReadyReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Interrupted => write!(f, "setup interrupted"),
            Self::Failed {
                exit,
                last_line: Some(line),
            } => write!(f, "{exit}: {line}"),
            Self::Failed {
                exit,
                last_line: None,
            } => write!(f, "{exit}"),
        }
    }
}

/// The state of a worktree whose setup is recorded as running, given whether its lock is held
/// right now. A held lock means the setup process is alive; a free one means it died.
pub(crate) fn setup_running(lock: LockStatus) -> State {
    match lock {
        LockStatus::Held => State::SettingUp,
        LockStatus::Free => State::NotReady(NotReadyReason::Interrupted),
    }
}

/// The state of a worktree whose setup failed as `exit`, with `log` as its setup log.
pub(crate) fn setup_failed(exit: Exit, log: &[u8]) -> State {
    State::NotReady(NotReadyReason::Failed {
        exit,
        last_line: setup::last_line(log),
    })
}

/// Why a worktree is broken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum BrokenReason {
    /// Its directory is missing.
    DirectoryMissing,
    /// Git marks it prunable, for the reason git gives (its `.git` file is gone, say).
    Prunable(String),
    /// Its state file cannot be read, for the reason given.
    StateUnreadable(String),
}

impl BrokenReason {
    /// Whether the worktree is gone from disk, so that only `worktree prune` can clean it.
    pub(crate) fn is_gone(&self) -> bool {
        match self {
            Self::DirectoryMissing | Self::Prunable(_) => true,
            Self::StateUnreadable(_) => false,
        }
    }
}

/// The reason as the fleet shows it, in the table's detail and in the JSON `detail`.
impl fmt::Display for BrokenReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DirectoryMissing => {
                write!(f, "directory missing; run orca-term worktree prune")
            }
            Self::Prunable(reason) => write!(
                f,
                "git marks it prunable ({reason}); run orca-term worktree prune"
            ),
            Self::StateUnreadable(reason) => write!(f, "{reason}"),
        }
    }
}

/// `worktree.json` as this core writes it.
///
/// `setup` joined schema 1 before any release, so it stays schema 1: a file without it reads as a
/// worktree with no setup recorded.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct WorktreeFile {
    schema: u64,
    agent: Option<String>,
    setup: Option<SetupFile>,
}

impl WorktreeFile {
    /// The state of a worktree with no agent, whose setup is recorded as `setup`.
    pub(crate) fn with_setup(setup: Option<SetupRecord>) -> Self {
        Self {
            schema: SUPPORTED_SCHEMA,
            agent: None,
            setup: setup.map(SetupFile::from_record),
        }
    }

    /// The bytes written to `worktree.json`.
    pub(crate) fn to_bytes(&self) -> Result<Vec<u8>> {
        Ok(serde_json::to_vec(self)?)
    }
}

/// The `setup` object of `worktree.json`: `{"status": "running" | "succeeded" | "failed"}`, with
/// `exit_code` or `signal` when it failed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SetupFile {
    status: SetupStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    signal: Option<i32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SetupStatus {
    Running,
    Succeeded,
    Failed,
}

impl SetupFile {
    fn from_record(record: SetupRecord) -> Self {
        let (status, exit_code, signal) = match record {
            SetupRecord::Running => (SetupStatus::Running, None, None),
            SetupRecord::Succeeded => (SetupStatus::Succeeded, None, None),
            SetupRecord::Failed(Exit::Code(code)) => (SetupStatus::Failed, Some(code), None),
            SetupRecord::Failed(Exit::Signal(signal)) => (SetupStatus::Failed, None, Some(signal)),
        };
        Self {
            status,
            exit_code,
            signal,
        }
    }

    fn to_record(&self) -> Result<SetupRecord> {
        Ok(match (self.status, self.exit_code, self.signal) {
            (SetupStatus::Running, ..) => SetupRecord::Running,
            (SetupStatus::Succeeded, ..) => SetupRecord::Succeeded,
            (SetupStatus::Failed, Some(code), _) => SetupRecord::Failed(Exit::Code(code)),
            (SetupStatus::Failed, None, Some(signal)) => SetupRecord::Failed(Exit::Signal(signal)),
            (SetupStatus::Failed, None, None) => {
                bail!("a failed setup records neither `exit_code` nor `signal`")
            }
        })
    }
}

/// `worktree.json` as this core reads it, from any schema.
#[derive(Debug, Default, Deserialize)]
struct Seen {
    #[serde(default)]
    schema: Option<u64>,
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "no agent can be attached yet; a recorded agent gains its states in a later slice"
    )]
    agent: serde_json::Value,
    #[serde(default)]
    setup: Option<SetupFile>,
}

/// What a worktree's state file records that its state is derived from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Recorded {
    /// The worktree's latest setup, or `None` when no setup script ran.
    pub(crate) setup: Option<SetupRecord>,
}

/// What the state file at `path`, whose content is `bytes`, records. A file that does not exist
/// records nothing: a worktree with no agent and no setup.
pub(crate) fn read(path: &Path, bytes: Option<&[u8]>) -> Result<Recorded> {
    let Some(bytes) = bytes else {
        return Ok(Recorded { setup: None });
    };
    let seen = parse(path, bytes)?;
    let setup = match seen.setup {
        Some(file) => Some(
            file.to_record()
                .with_context(|| format!("could not parse {}", path.display()))?,
        ),
        None => None,
    };
    Ok(Recorded { setup })
}

/// Refuses to overwrite the state file at `path`, whose content is `existing` (`None` when it does
/// not exist), when it is corrupt or carries a schema newer than this core supports.
pub(crate) fn check_overwrite(path: &Path, existing: Option<&[u8]>) -> Result<()> {
    let Some(bytes) = existing else {
        return Ok(());
    };
    if let Some(schema) = parse(path, bytes)?.schema
        && schema > SUPPORTED_SCHEMA
    {
        bail!(
            "{} has schema {schema}, newer than the schema {SUPPORTED_SCHEMA} this orca-term \
             supports; upgrade orca-term to change this worktree",
            path.display()
        );
    }
    Ok(())
}

fn parse(path: &Path, bytes: &[u8]) -> Result<Seen> {
    serde_json::from_slice(bytes).with_context(|| format!("could not parse {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATH: &str = "/admin/orca-term/worktree.json";

    fn recorded(bytes: &[u8]) -> Option<SetupRecord> {
        read(Path::new(PATH), Some(bytes)).unwrap().setup
    }

    #[test]
    fn writes_schema_1_with_no_agent_and_no_setup() {
        let bytes = WorktreeFile::with_setup(None).to_bytes().unwrap();
        assert_eq!(bytes, br#"{"schema":1,"agent":null,"setup":null}"#);
        assert_eq!(recorded(&bytes), None);
    }

    #[test]
    fn the_setup_is_written_and_read_back_unchanged() {
        for (record, json) in [
            (SetupRecord::Running, r#""setup":{"status":"running"}"#),
            (SetupRecord::Succeeded, r#""setup":{"status":"succeeded"}"#),
            (
                SetupRecord::Failed(Exit::Code(1)),
                r#""setup":{"status":"failed","exit_code":1}"#,
            ),
            (
                SetupRecord::Failed(Exit::Signal(9)),
                r#""setup":{"status":"failed","signal":9}"#,
            ),
        ] {
            let bytes = WorktreeFile::with_setup(Some(record)).to_bytes().unwrap();
            let text = String::from_utf8(bytes.clone()).unwrap();
            assert!(text.contains(json), "{text}");
            assert_eq!(recorded(&bytes), Some(record));
        }
    }

    #[test]
    fn a_file_without_setup_records_none() {
        assert_eq!(recorded(br#"{"schema":1,"agent":null}"#), None);
    }

    #[test]
    fn a_failed_setup_without_its_exit_is_reported_with_its_path() {
        let err = read(
            Path::new(PATH),
            Some(br#"{"schema":1,"setup":{"status":"failed"}}"#),
        )
        .unwrap_err();
        assert!(format!("{err:#}").starts_with("could not parse /admin/orca-term/worktree.json: "));
    }

    #[test]
    fn a_running_setup_is_setting_up_only_while_the_lock_is_held() {
        assert_eq!(setup_running(LockStatus::Held), State::SettingUp);
        assert_eq!(
            setup_running(LockStatus::Free),
            State::NotReady(NotReadyReason::Interrupted)
        );
        assert_eq!(
            State::NotReady(NotReadyReason::Interrupted).detail(),
            Some("setup interrupted".to_owned())
        );
    }

    #[test]
    fn a_failed_setup_is_not_ready_with_the_exit_and_the_last_log_line() {
        let state = setup_failed(Exit::Code(1), b"installing\nerror: no lockfile\n\n");
        assert_eq!(state.label(), "not ready");
        assert_eq!(
            state.detail(),
            Some("setup failed (exit 1): error: no lockfile".to_owned())
        );
        assert_eq!(
            setup_failed(Exit::Signal(9), b"").detail(),
            Some("setup killed by signal 9".to_owned())
        );
    }

    #[test]
    fn every_state_has_a_label_and_only_some_a_detail() {
        assert_eq!(State::NoAgent.label(), "no agent");
        assert_eq!(State::SettingUp.label(), "setting up");
        assert_eq!(State::NoAgent.detail(), None);
        assert_eq!(State::SettingUp.detail(), None);
        assert_eq!(
            State::Broken(BrokenReason::DirectoryMissing).detail(),
            Some("directory missing; run orca-term worktree prune".to_owned())
        );
    }

    #[test]
    fn refuses_to_write_over_a_newer_schema() {
        let newer = br#"{"schema":2,"agent":null,"holder":{"pid":1}}"#;
        let err = check_overwrite(Path::new(PATH), Some(newer)).unwrap_err();
        assert!(err.to_string().contains("upgrade orca-term"), "{err}");
    }

    #[test]
    fn overwrites_a_file_of_the_supported_schema_or_no_file() {
        check_overwrite(Path::new(PATH), Some(br#"{"schema":1,"agent":null}"#)).unwrap();
        check_overwrite(Path::new(PATH), None).unwrap();
    }

    #[test]
    fn reads_any_schema_and_ignores_unknown_fields() {
        let bytes = br#"{"schema":7,"agent":null,"future":[1,2]}"#;
        assert_eq!(recorded(bytes), None);
    }

    #[test]
    fn a_missing_file_records_no_setup() {
        assert_eq!(
            read(Path::new(PATH), None).unwrap(),
            Recorded { setup: None }
        );
    }

    #[test]
    fn the_state_file_the_log_and_the_lock_live_in_the_admin_dir() {
        let admin = AdminDir::new(PathBuf::from("/repo/.git/worktrees/fix/orca-term"));
        assert_eq!(
            admin.state_file(),
            PathBuf::from("/repo/.git/worktrees/fix/orca-term/worktree.json")
        );
        assert_eq!(
            admin.setup_log(),
            PathBuf::from("/repo/.git/worktrees/fix/orca-term/setup.log")
        );
        assert_eq!(
            admin.lock_file(),
            PathBuf::from("/repo/.git/worktrees/fix/orca-term/lock")
        );
    }

    #[test]
    fn a_broken_reason_says_what_to_do() {
        assert_eq!(
            BrokenReason::DirectoryMissing.to_string(),
            "directory missing; run orca-term worktree prune"
        );
        assert_eq!(
            BrokenReason::Prunable("gitdir file points to non-existent location".to_owned())
                .to_string(),
            "git marks it prunable (gitdir file points to non-existent location); run orca-term \
             worktree prune"
        );
        assert_eq!(
            BrokenReason::StateUnreadable("could not parse x".to_owned()).to_string(),
            "could not parse x"
        );
    }

    #[test]
    fn only_a_worktree_gone_from_disk_is_left_to_prune() {
        assert!(BrokenReason::DirectoryMissing.is_gone());
        assert!(BrokenReason::Prunable("gone".to_owned()).is_gone());
        assert!(!BrokenReason::StateUnreadable("corrupt".to_owned()).is_gone());
    }

    #[test]
    fn a_corrupt_file_is_reported_with_its_path() {
        let err = read(Path::new(PATH), Some(b"{not json")).unwrap_err();
        assert!(format!("{err:#}").starts_with("could not parse /admin/orca-term/worktree.json: "));
    }
}
