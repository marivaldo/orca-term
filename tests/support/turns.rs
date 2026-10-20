//! Drives a worktree's turns as claude does (seam 1): the real binary's `hook claude` run in a
//! worktree of a fixture repository, with claude's payloads on stdin (written here, or played from
//! the recordings in `tests/fixtures/claude-hooks/`), and real git to ask what the snapshots hold.
//! Shared by the suites of the snapshots and of the turn commands.

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde_json::{Value, json};

use crate::fixture::AgentRepo;

/// Where the recordings are.
pub(crate) const FIXTURES: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/claude-hooks");

/// The session of the payloads written here.
pub(crate) const SESSION: &str = "s-1";

/// A worktree of a fixture repository, whose hooks the test fires.
pub(crate) struct Snapped<'a> {
    pub(crate) fx: &'a AgentRepo,
    pub(crate) worktree: PathBuf,
    pub(crate) admin: PathBuf,
}

impl<'a> Snapped<'a> {
    /// Creates the worktree `name` with `worktree new`.
    pub(crate) fn new(fx: &'a AgentRepo, name: &str) -> Self {
        let worktree = fx.new_worktree(name);
        let admin = fx.admin_dir(&worktree);
        assert!(admin.is_dir(), "worktree new made the admin directory");
        Self {
            fx,
            worktree,
            admin,
        }
    }

    /// The id git gave the worktree: its admin directory is `.git/worktrees/<id>`.
    pub(crate) fn id(&self) -> String {
        let git_dir = self.admin.parent().unwrap();
        git_dir.file_name().unwrap().to_str().unwrap().to_owned()
    }

    /// The ref holding the worktree's chain of snapshots.
    pub(crate) fn chain_ref(&self) -> String {
        format!("refs/orca-term/worktrees/{}", self.id())
    }

    /// Runs the hook for `payload`'s event, as claude does: in the worktree, the payload on stdin,
    /// with the test's PATH. It must exit 0 and print nothing.
    pub(crate) fn fire(&self, payload: &Value) -> Output {
        self.fire_with(payload, None)
    }

    /// Like [`Self::fire`], with `PATH` set to `path` when given.
    pub(crate) fn fire_with(&self, payload: &Value, path: Option<&Path>) -> Output {
        let event = payload["hook_event_name"].as_str().unwrap();
        let admin = self.admin.to_str().unwrap();
        let mut command = self.fx.repo.command(&self.worktree);
        command.args(["hook", "claude", "--admin", admin, event]);
        if let Some(path) = path {
            command.env("PATH", path);
        }
        let out = run_with_stdin(command, payload.to_string().as_bytes());
        assert_eq!(out.status.code(), Some(0), "{event}");
        assert!(out.stdout.is_empty() && out.stderr.is_empty(), "{event}");
        out
    }

    /// Plays every hook of the recording `fixture`, its paths made the test's own.
    pub(crate) fn play(&self, fixture: &str) {
        let text = fs::read_to_string(Path::new(FIXTURES).join(fixture)).unwrap();
        let tmp = self.fx.repo.tmp.path();
        for line in text.lines().filter(|line| !line.is_empty()) {
            let (_, payload) = line.split_once(' ').unwrap();
            let payload = payload
                .replace("{{worktree}}", self.worktree.to_str().unwrap())
                .replace("{{transcripts}}", tmp.join("transcripts").to_str().unwrap())
                .replace("{{denied}}", tmp.join("denied").to_str().unwrap())
                .replace("{{tasks}}", tmp.join("tasks").to_str().unwrap());
            self.fire(&serde_json::from_str(&payload).unwrap());
        }
    }

    /// Writes `contents` to `rel` in the worktree.
    pub(crate) fn write(&self, rel: &str, contents: &str) {
        fs::write(self.worktree.join(rel), contents).unwrap();
    }

    /// Appends `line` and a newline to `rel` in the worktree.
    pub(crate) fn append(&self, rel: &str, line: &str) {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.worktree.join(rel))
            .unwrap();
        writeln!(file, "{line}").unwrap();
    }

    /// Runs git in the worktree and returns its stdout, trimmed.
    pub(crate) fn git(&self, args: &[&str]) -> String {
        let mut full = vec!["-C", self.worktree.to_str().unwrap()];
        full.extend_from_slice(args);
        self.fx.repo.git(&full).trim_end().to_owned()
    }

    /// Whether the ref `name` exists.
    pub(crate) fn has_ref(&self, name: &str) -> bool {
        !self
            .git(&["for-each-ref", "--format=%(refname)", name])
            .is_empty()
    }

    /// The turn log, one value per line.
    pub(crate) fn turns(&self) -> Vec<Value> {
        let text = fs::read_to_string(self.admin.join("turns.jsonl")).unwrap_or_default();
        text.lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// The turn log's line for `kind` (`start` or `end`) of turn `turn`.
    pub(crate) fn line(&self, turn: u64, kind: &str) -> Value {
        self.turns()
            .into_iter()
            .find(|line| line["turn"] == turn && line["kind"] == kind)
            .unwrap_or_else(|| panic!("no {kind} line for turn {turn}"))
    }

    /// The snapshot `field` (`begin` or `end`) of turn `turn`, from its end line (`begin` from
    /// its start line while it is open).
    pub(crate) fn snap(&self, turn: u64, field: &str) -> String {
        let kind = if field == "end" { "end" } else { "start" };
        self.line(turn, kind)[field].as_str().unwrap().to_owned()
    }

    /// `status.json`, parsed.
    pub(crate) fn status(&self) -> Value {
        serde_json::from_slice(&fs::read(self.admin.join("status.json")).unwrap()).unwrap()
    }

    /// `hook.log`, empty when missing.
    pub(crate) fn hook_log(&self) -> String {
        fs::read_to_string(self.admin.join("hook.log")).unwrap_or_default()
    }

    /// The worktree's real index, byte for byte.
    pub(crate) fn index_bytes(&self) -> Vec<u8> {
        let path = self.git(&["rev-parse", "--path-format=absolute", "--git-path", "index"]);
        fs::read(path).unwrap()
    }

    /// `git status --porcelain`, without letting git refresh the index.
    pub(crate) fn porcelain(&self) -> String {
        self.git(&["--no-optional-locks", "status", "--porcelain"])
    }

    /// The author of `commit`, as `name <email>`.
    pub(crate) fn author(&self, commit: &str) -> String {
        self.git(&["log", "-1", "--format=%an <%ae>", commit])
    }

    /// The parents of `commit`, space-separated, empty for none.
    pub(crate) fn parents(&self, commit: &str) -> String {
        let line = self.git(&["rev-list", "--parents", "-n", "1", commit]);
        line.split_once(' ')
            .map(|(_, parents)| parents.to_owned())
            .unwrap_or_default()
    }
}

/// Runs `command` with `stdin` written to its standard input, collecting its output.
pub(crate) fn run_with_stdin(mut command: Command, stdin: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let _ = child.stdin.take().unwrap().write_all(stdin);
    child.wait_with_output().unwrap()
}

/// `UserPromptSubmit` of the prompt `prompt_id`.
pub(crate) fn submit(prompt_id: &str) -> Value {
    json!({
        "hook_event_name": "UserPromptSubmit",
        "session_id": SESSION,
        "prompt_id": prompt_id,
        "prompt": format!("do {prompt_id}"),
    })
}

/// `Stop` of the turn of `prompt_id`.
pub(crate) fn stop(prompt_id: &str) -> Value {
    json!({
        "hook_event_name": "Stop",
        "session_id": SESSION,
        "prompt_id": prompt_id,
        "last_assistant_message": "done",
    })
}

/// `SessionEnd` of the session.
pub(crate) fn session_end() -> Value {
    json!({"hook_event_name": "SessionEnd", "session_id": SESSION, "reason": "other"})
}
