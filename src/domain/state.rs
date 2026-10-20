//! Domain: a worktree's state, kept as `worktree.json` in git's per-worktree admin directory
//! (ADR 0003).
//!
//! Every state file carries `schema`. Reading tolerates any schema, taking the fields it knows and
//! ignoring the rest; writing refuses a file whose schema is newer than this core supports, so an
//! older core never clobbers what a newer one wrote.
//!
//! A worktree's state is derived on read from what `worktree.json` records, the live status the
//! agent's hooks keep (`status.json`, `domain/turn.rs`), and volatile facts checked at that
//! moment: whether the worktree's lock and the holder's lock are held, and the setup log. Nothing
//! is rewritten on the read path.

use std::fmt;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::domain::agent::HolderStatus;
use crate::domain::setup::{self, Exit, SetupRecord};
use crate::domain::turn::{Outcome, Phase, Status};

/// The newest schema this core reads and writes.
pub(crate) const SUPPORTED_SCHEMA: u64 = 1;

/// The worktree's state file, inside its admin directory.
const WORKTREE_FILE: &str = "worktree.json";

/// The worktree's lock file, inside its admin directory. Whoever holds its lock may write the
/// worktree's state.
const LOCK_FILE: &str = "lock";

/// The directory, inside git's per-worktree admin directory, that holds the worktree's state.
pub(crate) const ADMIN_DIR_NAME: &str = "orca-term";

/// The lock the holder keeps for its whole life: held means the agent runs (ADR 0009).
const AGENT_LOCK_FILE: &str = "agent.lock";

/// The holder's pid and socket, written by the holder; trusted only while `agent.lock` is held.
const AGENT_RECORD_FILE: &str = "agent.json";

/// The unix socket clients attach through. The holder binds it, and clients connect to it, by
/// this bare name from inside the admin directory, since the absolute path can exceed what a unix
/// socket address holds.
pub(crate) const AGENT_SOCKET_FILE: &str = "agent.sock";

/// The settings claude is launched with, rebuilt on every start.
const CLAUDE_SETTINGS_FILE: &str = "claude-settings.json";

/// The live status the agent's hooks keep (`domain/turn.rs`).
const STATUS_FILE: &str = "status.json";

/// The turn log the agent's hooks append to (`domain/turn.rs`).
const TURN_LOG_FILE: &str = "turns.jsonl";

/// The lock each hook run takes, waiting briefly, while it changes the status and the turn log.
/// Commands never take it.
const HOOK_LOCK_FILE: &str = "hook.lock";

/// What went wrong in the hooks, one line each, kept small.
const HOOK_LOG_FILE: &str = "hook.log";

/// What the person has seen of the worktree (`domain/seen.rs`). Only the person's commands write
/// it; the hooks never do.
const SEEN_FILE: &str = "seen.json";

/// The worktree's pending notes (`domain/note.rs`). Only the note commands write it.
const NOTES_FILE: &str = "notes.json";

/// The lock the note commands take, waiting briefly, while they change `notes.json`.
const NOTES_LOCK_FILE: &str = "notes.lock";

/// The directory of the file contents kept for notes on lines, one file per git blob id.
const NOTE_COPIES_DIR: &str = "notes";

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

    /// The holder's lock, `agent.lock`.
    pub(crate) fn agent_lock(&self) -> PathBuf {
        self.0.join(AGENT_LOCK_FILE)
    }

    /// The holder's record, `agent.json`.
    pub(crate) fn agent_record(&self) -> PathBuf {
        self.0.join(AGENT_RECORD_FILE)
    }

    /// The holder's socket, `agent.sock`, for display and removal.
    pub(crate) fn agent_socket(&self) -> PathBuf {
        self.0.join(AGENT_SOCKET_FILE)
    }

    /// claude's settings file, `claude-settings.json`.
    pub(crate) fn claude_settings(&self) -> PathBuf {
        self.0.join(CLAUDE_SETTINGS_FILE)
    }

    /// The hooks' live status, `status.json`.
    pub(crate) fn status_file(&self) -> PathBuf {
        self.0.join(STATUS_FILE)
    }

    /// The hooks' turn log, `turns.jsonl`.
    pub(crate) fn turn_log(&self) -> PathBuf {
        self.0.join(TURN_LOG_FILE)
    }

    /// The hooks' lock, `hook.lock`.
    pub(crate) fn hook_lock(&self) -> PathBuf {
        self.0.join(HOOK_LOCK_FILE)
    }

    /// The hooks' error log, `hook.log`.
    pub(crate) fn hook_log(&self) -> PathBuf {
        self.0.join(HOOK_LOG_FILE)
    }

    /// What the person has seen, `seen.json`.
    pub(crate) fn seen_file(&self) -> PathBuf {
        self.0.join(SEEN_FILE)
    }

    /// The pending notes, `notes.json`.
    pub(crate) fn notes_file(&self) -> PathBuf {
        self.0.join(NOTES_FILE)
    }

    /// The note commands' lock, `notes.lock`.
    pub(crate) fn notes_lock(&self) -> PathBuf {
        self.0.join(NOTES_LOCK_FILE)
    }

    /// The directory of the contents kept for notes on lines, `notes/`.
    pub(crate) fn note_copies(&self) -> PathBuf {
        self.0.join(NOTE_COPIES_DIR)
    }

    /// git's `commondir` file beside this directory, in git's per-worktree admin directory
    /// (`<common>/worktrees/<id>`): it names the git common dir.
    pub(crate) fn commondir_file(&self) -> Option<PathBuf> {
        self.0.parent().map(|git_dir| git_dir.join(COMMONDIR_FILE))
    }

    /// git's `gitdir` file beside this directory: it names the worktree's `.git` file.
    pub(crate) fn gitdir_file(&self) -> Option<PathBuf> {
        self.0.parent().map(|git_dir| git_dir.join(GITDIR_FILE))
    }
}

/// git's file, in a worktree's git admin directory, naming the git common dir.
const COMMONDIR_FILE: &str = "commondir";

/// git's file, in a worktree's git admin directory, naming the worktree's `.git` file.
const GITDIR_FILE: &str = "gitdir";

/// The git common dir named by the `commondir` file at `file`, whose text is `text`: a path
/// relative to the file's directory (git writes `../..`), or absolute. `None` when it names
/// nothing.
pub(crate) fn common_dir_from(file: &Path, text: &str) -> Option<PathBuf> {
    path_in_git_file(file, text)
}

/// The worktree's path named by the `gitdir` file at `file`, whose text is `text`: the path of
/// the worktree's `.git` file, absolute (or relative to the file's directory, when git is set to
/// write relative paths). The worktree is that path without its `/.git`, which is the path
/// `git worktree list`, and so `worktree ls`, prints. `None` when it names nothing.
pub(crate) fn worktree_path_from(file: &Path, text: &str) -> Option<PathBuf> {
    let dot_git = path_in_git_file(file, text)?;
    match dot_git.file_name() {
        Some(name) if name == ".git" => dot_git.parent().map(Path::to_path_buf),
        _ => Some(dot_git),
    }
}

/// The path written on the first line of the git file at `file`, resolved against the file's
/// directory, with `.` and `..` taken out by hand (no symlink is resolved).
fn path_in_git_file(file: &Path, text: &str) -> Option<PathBuf> {
    let line = text.lines().next()?.trim_end_matches('\r');
    if line.is_empty() {
        return None;
    }
    let joined = file.parent()?.join(line);
    let mut resolved = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                resolved.pop();
            }
            other => resolved.push(other),
        }
    }
    Some(resolved)
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
    /// The worktree is ready and has an agent, which is not in a turn: no turn yet, or the
    /// latest one belongs to an earlier session.
    Idle,
    /// The agent is in a turn, working.
    Working,
    /// The agent is in a turn, waiting for the person: a permission dialog or a question.
    Waiting,
    /// The agent's latest turn of the current session finished.
    Finished,
    /// The agent's latest turn of the current session ended on an API error.
    Failed,
    /// The agent's latest turn ended without its end being seen: the person pressed Esc and sent
    /// another prompt, the session ended, or the agent died mid-turn.
    Interrupted,
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
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Waiting => "waiting",
            Self::Finished => "finished",
            Self::Failed => "failed",
            Self::Interrupted => "interrupted",
            Self::SettingUp => "setting up",
            Self::NotReady(_) => "not ready",
            Self::Broken(_) => "broken",
        }
    }

    /// Why the worktree is in its state, when there is something to say.
    pub(crate) fn detail(&self) -> Option<String> {
        match self {
            Self::NoAgent
            | Self::Idle
            | Self::Working
            | Self::Waiting
            | Self::Finished
            | Self::Failed
            | Self::Interrupted
            | Self::SettingUp => None,
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

/// The state of a ready worktree whose agent is `agent` (`None`: no agent), with the activity
/// shown beside it. With an agent, the state comes from the hooks' live status (`None` before any
/// hook reported, or when `status.json` cannot be used) and whether its holder is alive:
///
/// - a turn is open: `working` or `waiting` while the holder lives, showing what the agent does;
///   `interrupted` once it is gone (the agent died mid-turn), showing the turn's prompt;
/// - no turn is open: the latest turn's outcome when it belongs to the current session, showing
///   what the agent said last (an interrupted turn shows its prompt); otherwise `idle`, as a
///   fresh session reads.
pub(crate) fn ready(
    agent: Option<&str>,
    status: Option<&Status>,
    holder: HolderStatus,
) -> (State, Option<String>) {
    if agent.is_none() {
        return (State::NoAgent, None);
    }
    let Some(status) = status else {
        return (State::Idle, None);
    };
    if let Some(turn) = &status.turn {
        return match (holder, turn.phase) {
            (HolderStatus::Gone, _) => (State::Interrupted, shown(&turn.prompt)),
            (HolderStatus::Alive, Phase::Waiting) => (State::Waiting, shown(&turn.activity)),
            (HolderStatus::Alive, Phase::Working) => (State::Working, shown(&turn.activity)),
        };
    }
    match &status.latest {
        Some(closed) if closed.session == status.session => {
            let activity = closed.detail.as_deref().unwrap_or(&closed.prompt);
            (outcome_state(closed.outcome), shown(activity))
        }
        _ => (State::Idle, None),
    }
}

/// The state a closed turn's outcome shows as.
fn outcome_state(outcome: Outcome) -> State {
    match outcome {
        Outcome::Finished => State::Finished,
        Outcome::Failed => State::Failed,
        Outcome::Interrupted => State::Interrupted,
    }
}

/// An activity to show, `None` when there is no text.
fn shown(text: &str) -> Option<String> {
    (!text.is_empty()).then(|| text.to_owned())
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
/// worktree with no setup recorded. `agent` is the recorded agent's name (`claude-code`) or null.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct WorktreeFile {
    schema: u64,
    agent: Option<String>,
    setup: Option<SetupFile>,
}

impl WorktreeFile {
    /// The state of a worktree whose agent is `agent` and whose setup is recorded as `setup`.
    /// Every write passes both, so neither is lost when the other changes.
    pub(crate) fn new(agent: Option<&str>, setup: Option<SetupRecord>) -> Self {
        Self {
            schema: SUPPORTED_SCHEMA,
            agent: agent.map(str::to_owned),
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
    agent: Option<String>,
    #[serde(default)]
    setup: Option<SetupFile>,
}

/// What a worktree's state file records that its state is derived from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Recorded {
    /// The worktree's agent, by name, or `None` when it has none.
    pub(crate) agent: Option<String>,
    /// The worktree's latest setup, or `None` when no setup script ran.
    pub(crate) setup: Option<SetupRecord>,
}

/// What the state file at `path`, whose content is `bytes`, records. A file that does not exist
/// records nothing: a worktree with no agent and no setup.
pub(crate) fn read(path: &Path, bytes: Option<&[u8]>) -> Result<Recorded> {
    let Some(bytes) = bytes else {
        return Ok(Recorded {
            agent: None,
            setup: None,
        });
    };
    let seen = parse(path, bytes)?;
    let setup = match seen.setup {
        Some(file) => Some(
            file.to_record()
                .with_context(|| format!("could not parse {}", path.display()))?,
        ),
        None => None,
    };
    Ok(Recorded {
        agent: seen.agent,
        setup,
    })
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
    use crate::domain::turn::{ClosedTurn, OpenTurn};

    const PATH: &str = "/admin/orca-term/worktree.json";

    fn recorded(bytes: &[u8]) -> Option<SetupRecord> {
        read(Path::new(PATH), Some(bytes)).unwrap().setup
    }

    #[test]
    fn writes_schema_1_with_no_agent_and_no_setup() {
        let bytes = WorktreeFile::new(None, None).to_bytes().unwrap();
        assert_eq!(bytes, br#"{"schema":1,"agent":null,"setup":null}"#);
        assert_eq!(recorded(&bytes), None);
    }

    #[test]
    fn the_agent_is_written_by_name_and_read_back() {
        let bytes = WorktreeFile::new(Some("claude-code"), Some(SetupRecord::Succeeded))
            .to_bytes()
            .unwrap();
        assert_eq!(
            bytes,
            br#"{"schema":1,"agent":"claude-code","setup":{"status":"succeeded"}}"#
        );
        let seen = read(Path::new(PATH), Some(&bytes)).unwrap();
        assert_eq!(seen.agent.as_deref(), Some("claude-code"));
        assert_eq!(seen.setup, Some(SetupRecord::Succeeded));
    }

    #[test]
    fn a_ready_worktree_is_idle_only_with_an_agent() {
        let agent = Some("claude-code");
        assert_eq!(ready(agent, None, HolderStatus::Alive), (State::Idle, None));
        assert_eq!(ready(agent, None, HolderStatus::Gone), (State::Idle, None));
        let any = Status::default();
        assert_eq!(
            ready(None, Some(&any), HolderStatus::Alive),
            (State::NoAgent, None)
        );
        assert_eq!(
            ready(agent, Some(&any), HolderStatus::Alive),
            (State::Idle, None)
        );
        assert_eq!(State::Idle.label(), "idle");
        assert_eq!(State::Idle.detail(), None);
    }

    fn open_turn(phase: Phase) -> Status {
        Status {
            session: Some("s1".to_owned()),
            turn: Some(OpenTurn {
                number: 2,
                session: Some("s1".to_owned()),
                prompt_id: Some("p2".to_owned()),
                prompt: "fix the login".to_owned(),
                started_at: 1,
                phase,
                activity: "Bash: npm test".to_owned(),
                denials: Vec::new(),
            }),
            ..Status::default()
        }
    }

    fn closed_turn(outcome: Outcome, session: &str, detail: Option<&str>) -> Status {
        Status {
            session: Some("s1".to_owned()),
            latest: Some(ClosedTurn {
                number: 1,
                session: Some(session.to_owned()),
                prompt_id: Some("p1".to_owned()),
                prompt: "fix the login".to_owned(),
                outcome,
                detail: detail.map(str::to_owned),
                error: None,
                ended_at: 2,
                denials: Vec::new(),
                message: None,
            }),
            ..Status::default()
        }
    }

    #[test]
    fn an_open_turn_is_working_or_waiting_while_the_holder_lives() {
        let agent = Some("claude-code");
        let activity = Some("Bash: npm test".to_owned());
        assert_eq!(
            ready(agent, Some(&open_turn(Phase::Working)), HolderStatus::Alive),
            (State::Working, activity.clone())
        );
        assert_eq!(
            ready(agent, Some(&open_turn(Phase::Waiting)), HolderStatus::Alive),
            (State::Waiting, activity)
        );
    }

    #[test]
    fn an_open_turn_whose_holder_is_gone_is_interrupted_showing_its_prompt() {
        for phase in [Phase::Working, Phase::Waiting] {
            assert_eq!(
                ready(
                    Some("claude-code"),
                    Some(&open_turn(phase)),
                    HolderStatus::Gone
                ),
                (State::Interrupted, Some("fix the login".to_owned()))
            );
        }
    }

    #[test]
    fn a_closed_turn_of_the_current_session_shows_its_outcome() {
        let agent = Some("claude-code");
        for (outcome, state) in [
            (Outcome::Finished, State::Finished),
            (Outcome::Failed, State::Failed),
            (Outcome::Interrupted, State::Interrupted),
        ] {
            for holder in [HolderStatus::Alive, HolderStatus::Gone] {
                let status = closed_turn(outcome, "s1", Some("All done."));
                assert_eq!(
                    ready(agent, Some(&status), holder),
                    (state.clone(), Some("All done.".to_owned()))
                );
            }
        }
        let interrupted = closed_turn(Outcome::Interrupted, "s1", None);
        assert_eq!(
            ready(agent, Some(&interrupted), HolderStatus::Alive),
            (State::Interrupted, Some("fix the login".to_owned())),
            "with no detail, the turn's prompt"
        );
    }

    #[test]
    fn a_closed_turn_of_an_earlier_session_reads_idle() {
        let status = closed_turn(Outcome::Finished, "s0", Some("All done."));
        assert_eq!(
            ready(Some("claude-code"), Some(&status), HolderStatus::Alive),
            (State::Idle, None)
        );
    }

    #[test]
    fn the_agent_states_have_labels_and_no_detail() {
        for (state, label) in [
            (State::Working, "working"),
            (State::Waiting, "waiting"),
            (State::Finished, "finished"),
            (State::Failed, "failed"),
            (State::Interrupted, "interrupted"),
        ] {
            assert_eq!(state.label(), label);
            assert_eq!(state.detail(), None);
        }
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
            let bytes = WorktreeFile::new(None, Some(record)).to_bytes().unwrap();
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
            Recorded {
                agent: None,
                setup: None
            }
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
        assert_eq!(
            admin.agent_lock(),
            PathBuf::from("/repo/.git/worktrees/fix/orca-term/agent.lock")
        );
        assert_eq!(
            admin.agent_record(),
            PathBuf::from("/repo/.git/worktrees/fix/orca-term/agent.json")
        );
        assert_eq!(
            admin.agent_socket(),
            PathBuf::from("/repo/.git/worktrees/fix/orca-term/agent.sock")
        );
        assert_eq!(AGENT_SOCKET_FILE, "agent.sock");
        assert_eq!(
            admin.claude_settings(),
            PathBuf::from("/repo/.git/worktrees/fix/orca-term/claude-settings.json")
        );
        assert_eq!(
            admin.status_file(),
            PathBuf::from("/repo/.git/worktrees/fix/orca-term/status.json")
        );
        assert_eq!(
            admin.turn_log(),
            PathBuf::from("/repo/.git/worktrees/fix/orca-term/turns.jsonl")
        );
        assert_eq!(
            admin.hook_lock(),
            PathBuf::from("/repo/.git/worktrees/fix/orca-term/hook.lock")
        );
        assert_eq!(
            admin.hook_log(),
            PathBuf::from("/repo/.git/worktrees/fix/orca-term/hook.log")
        );
        assert_eq!(
            admin.seen_file(),
            PathBuf::from("/repo/.git/worktrees/fix/orca-term/seen.json")
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

    #[test]
    fn gits_files_beside_the_admin_dir_name_the_common_dir_and_the_worktree() {
        let admin = AdminDir::new(PathBuf::from("/r/.git/worktrees/fix/orca-term"));
        let commondir = admin.commondir_file().unwrap();
        let gitdir = admin.gitdir_file().unwrap();
        assert_eq!(commondir, Path::new("/r/.git/worktrees/fix/commondir"));
        assert_eq!(gitdir, Path::new("/r/.git/worktrees/fix/gitdir"));

        assert_eq!(
            common_dir_from(&commondir, "../..\n"),
            Some(PathBuf::from("/r/.git"))
        );
        assert_eq!(
            common_dir_from(&commondir, "/elsewhere/.git\r\n"),
            Some(PathBuf::from("/elsewhere/.git"))
        );
        assert_eq!(common_dir_from(&commondir, ""), None);
        assert_eq!(common_dir_from(&commondir, "\n"), None);

        assert_eq!(
            worktree_path_from(&gitdir, "/w/repo/fix/.git\n"),
            Some(PathBuf::from("/w/repo/fix"))
        );
        assert_eq!(
            worktree_path_from(&gitdir, "../../../../w/fix/.git\n"),
            Some(PathBuf::from("/w/fix")),
            "relative to the file's directory, with worktree.useRelativePaths"
        );
        assert_eq!(
            worktree_path_from(&gitdir, "/w/odd\n"),
            Some(PathBuf::from("/w/odd"))
        );
        assert_eq!(worktree_path_from(&gitdir, ""), None);
    }
}
