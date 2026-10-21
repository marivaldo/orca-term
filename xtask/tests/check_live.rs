//! The live checks end to end, against fake agents (seam 1, #44): `cargo xtask check-live` runs in
//! a throwaway checkout holding scenarios and an older recording, with a PATH of one directory
//! holding a fake `claude` and a fake `opencode` that send known hooks and lines through the real
//! orca-term, fake sandbox tools that run what they wrap, and the real `git` and `bash`; `CARGO`
//! names a fake cargo whose suites pass or diverge as the test says. So it spends no prompt and
//! needs no agent, no sandbox and no network.
//!
//! It checks that the recordings are written normalised and named after the agents' versions,
//! beside the older one, that the summary names each scenario and suite, that a diverging suite
//! or a failed scenario makes it exit 1, and that a missing agent is refused before anything runs.

#![expect(
    clippy::unwrap_used,
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "a black-box test builds files and runs processes around the tool it drives"
)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::OnceLock;

use tempfile::TempDir;

/// The fake `claude`: answers `--version`; otherwise runs the orca-term its settings name for their
/// hooks, as claude's hooks do: for `SessionStart`, then for each line typed `UserPromptSubmit`, a
/// `PreToolUse` writing into the admin directory and `Stop`, and for `SessionEnd` on `SIGHUP`. Its
/// transcripts are said to be in `$FAKE_TRANSCRIPTS`. Bash builtins and absolute paths only.
const FAKE_CLAUDE: &str = r#"#!/bin/bash
if [ "$1" = "--version" ]; then echo "2.1.999 (Claude Code)"; exit 0; fi
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--settings" ]; then settings="$2"; fi
  shift
done
text="$(<"$settings")"
# Each handler is /bin/sh -c <script> <orca-term> <admin dir> <event>: the path after the script.
re='exit 0",[[:space:]]*"([^"]+)"'
[[ $text =~ $re ]] && exe="${BASH_REMATCH[1]}"
admin="${settings%/*}"
base="{\"session_id\":\"s-1\",\"transcript_path\":\"$FAKE_TRANSCRIPTS/s-1.jsonl\",\"cwd\":\"$PWD\""
hook() { printf '%s' "$base,$2}" | "$exe" hook claude --admin "$admin" "$1"; }
trap 'hook SessionEnd "\"hook_event_name\":\"SessionEnd\",\"reason\":\"other\""; exit 129' HUP
hook SessionStart '"hook_event_name":"SessionStart","source":"startup"'
echo "fake claude ready"
n=0
while IFS= read -r -t 60 line; do
  n=$((n + 1))
  hook UserPromptSubmit "\"prompt_id\":\"p$n\",\"hook_event_name\":\"UserPromptSubmit\",\"prompt\":\"$line\""
  hook PreToolUse "\"prompt_id\":\"p$n\",\"hook_event_name\":\"PreToolUse\",\"tool_name\":\"Bash\",\"tool_input\":{\"command\":\"touch $admin/x\"}"
  hook Stop "\"prompt_id\":\"p$n\",\"hook_event_name\":\"Stop\",\"last_assistant_message\":\"ok\""
  echo "ok"
done
"#;

/// The fake `opencode`: answers `--version`; otherwise writes the plugin's lines to the events
/// pipe: `start`, then for each line typed `prompt`, a `tool` reading a file of the worktree and
/// `idle`, and `end` on `SIGHUP`.
const FAKE_OPENCODE: &str = r#"#!/bin/bash
if [ "$1" = "--version" ]; then echo "1.18.5"; exit 0; fi
send() { printf '%s\n' "$1" >&"$ORCA_TERM_EVENTS_FD"; }
trap 'send "{\"type\":\"end\"}"; exit 129' HUP
send '{"type":"start"}'
echo "fake opencode ready"
while IFS= read -r -t 60 line; do
  send "{\"type\":\"prompt\",\"session\":\"ses_1\",\"parent\":null,\"text\":\"$line\"}"
  send "{\"type\":\"tool\",\"session\":\"ses_1\",\"parent\":null,\"tool\":\"read\",\"input\":{\"filePath\":\"$PWD/README.md\"}}"
  send "{\"type\":\"idle\",\"session\":\"ses_1\",\"parent\":null,\"text\":\"ok\",\"finish\":\"stop\",\"error\":null}"
done
"#;

/// The fake `sandbox-exec` runs what follows `-f <profile>`; the fake `bwrap` what follows `--`.
const FAKE_SANDBOX_EXEC: &str = "#!/bin/bash\nshift 2\nexec \"$@\"\n";
const FAKE_BWRAP: &str = "#!/bin/bash\nwhile [ \"$#\" -gt 0 ] && [ \"$1\" != \"--\" ]; do shift; done\nshift\nexec \"$@\"\n";

/// The fake cargo: logs its arguments, and fails the suite `$FAKE_DIVERGE` names as cargo lists a
/// failing test; every other suite passes.
const FAKE_CARGO: &str = r#"#!/bin/bash
echo "$*" >> "$FAKE_CARGO_LOG"
prev=""
for arg in "$@"; do
  if [ "$prev" = "--test" ]; then suite="$arg"; fi
  prev="$arg"
done
if [ -n "$FAKE_DIVERGE" ] && [ "$suite" = "$FAKE_DIVERGE" ]; then
  printf 'running 2 tests\n\nfailures:\n    replays_the_new_recording\n\ntest result: FAILED. 1 passed; 1 failed\n'
  exit 101
fi
echo "test result: ok. 2 passed"
"#;

const FAKE_HELLO: &str = "\
# The fake claude answers a prompt.
agent claude-code
recording fake-hello
timeout 30
expect finished

wait-for SessionStart
type reply ok
press enter
wait-for Stop
";

const FAKE_OC: &str = "\
agent opencode
recording fake-oc
timeout 30
expect finished

wait-for start
type look
press enter
wait-for idle
";

const FAKE_STUCK: &str = "\
agent claude-code
recording fake-stuck
timeout 3
expect finished

wait-for PermissionRequest
";

const FAKE_BY_HAND: &str = "\
agent claude-code
recording fake-by-hand
manual
timeout 30
expect finished
";

/// An older recording, which a new version must leave alone.
const OLDER: &str = "SessionStart {\"session_id\":\"old\"}\n";

/// The orca-term the checks drive, built once.
fn orca_term() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| {
        let status = Command::new(env!("CARGO"))
            .args(["build", "--locked", "--quiet", "--package", "orca-term"])
            .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/.."))
            .status()
            .unwrap();
        assert!(status.success(), "cargo build of orca-term failed");
        let debug = Path::new(env!("CARGO_BIN_EXE_xtask")).parent().unwrap();
        debug.join("orca-term")
    })
}

/// A throwaway checkout with the scenarios and an older recording, the isolated PATH and fakes.
struct Live {
    tmp: TempDir,
    checkout: PathBuf,
    bin: PathBuf,
}

impl Live {
    fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();
        for dir in [
            "tmp",
            "transcripts",
            "xdg/data",
            "xdg/cache",
            "xdg/config",
            "xdg/state",
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
        }
        fs::write(
            root.join("gitconfig"),
            "[user]\n\tname = Fixture\n\temail = fixture@example.invalid\n[init]\n\tdefaultBranch = main\n",
        )
        .unwrap();
        let bin = root.join("bin");
        fs::create_dir(&bin).unwrap();
        for tool in ["git", "bash"] {
            std::os::unix::fs::symlink(which(tool), bin.join(tool)).unwrap();
        }
        let live = Self {
            checkout: root.join("checkout"),
            bin,
            tmp,
        };
        live.write_tool("claude", FAKE_CLAUDE);
        live.write_tool("opencode", FAKE_OPENCODE);
        live.write_tool("sandbox-exec", FAKE_SANDBOX_EXEC);
        live.write_tool("bwrap", FAKE_BWRAP);
        live.write_tool("socat", "#!/bin/sh\nexit 0\n");
        live.write_checkout();
        live
    }

    fn root(&self) -> PathBuf {
        self.tmp.path().canonicalize().unwrap()
    }

    fn write_tool(&self, name: &str, script: &str) {
        let path = self.bin.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    }

    /// The checkout: `tests/live/` with the scenarios, `tests/fixtures/claude-hooks/` with the
    /// older recording, all committed.
    fn write_checkout(&self) {
        let live = self.checkout.join("tests/live");
        let hooks = self.checkout.join("tests/fixtures/claude-hooks");
        fs::create_dir_all(&live).unwrap();
        fs::create_dir_all(&hooks).unwrap();
        for (name, text) in [
            ("fake-hello", FAKE_HELLO),
            ("fake-oc", FAKE_OC),
            ("fake-stuck", FAKE_STUCK),
            ("fake-by-hand", FAKE_BY_HAND),
        ] {
            fs::write(live.join(format!("{name}.scenario")), text).unwrap();
        }
        fs::write(hooks.join("fake-hello-2.1.300.log"), OLDER).unwrap();
        self.git(&["init", "--quiet"]);
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "-m", "init"]);
    }

    fn git(&self, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(&self.checkout)
            .env("GIT_CONFIG_GLOBAL", self.root().join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdout(Stdio::null())
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?}");
    }

    /// Runs `cargo xtask check-live <args>` in the checkout, the suite `diverge` failing when
    /// given.
    fn check_live(&self, args: &[&str], diverge: Option<&str>) -> Output {
        self.xtask("check-live", args)
            .env("FAKE_DIVERGE", diverge.unwrap_or_default())
            .output()
            .unwrap()
    }

    /// Runs `cargo xtask record-live <scenario>` in the checkout. Its input ends at once, which
    /// detaches the attach it hands its terminal to, as a person detaching would.
    fn record_live(&self, scenario: &str) -> Output {
        self.xtask("record-live", &[scenario]).output().unwrap()
    }

    /// `cargo xtask <command> --orca-term <orca-term> <args>`, in the checkout, with the isolated
    /// PATH, the fake cargo, and the temporary and XDG directories inside the test's own.
    fn xtask(&self, command: &str, args: &[&str]) -> Command {
        let root = self.root();
        let mut xtask = Command::new(env!("CARGO_BIN_EXE_xtask"));
        xtask
            .arg(command)
            .arg("--orca-term")
            .arg(orca_term())
            .args(args)
            .current_dir(&self.checkout)
            .env("PATH", &self.bin)
            .env("CARGO", self.fake_cargo())
            .env("FAKE_CARGO_LOG", root.join("cargo.log"))
            .env("FAKE_TRANSCRIPTS", root.join("transcripts"))
            .env("TMPDIR", root.join("tmp"))
            .env("XDG_DATA_HOME", root.join("xdg/data"))
            .env("XDG_CACHE_HOME", root.join("xdg/cache"))
            .env("XDG_CONFIG_HOME", root.join("xdg/config"))
            .env("XDG_STATE_HOME", root.join("xdg/state"))
            .env("GIT_CONFIG_GLOBAL", root.join("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null());
        xtask
    }

    /// The fake cargo, outside PATH: only `CARGO` names it.
    fn fake_cargo(&self) -> PathBuf {
        let path = self.root().join("cargo");
        if !path.exists() {
            fs::write(&path, FAKE_CARGO).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        path
    }

    /// The recording `name` in the checkout's `tests/fixtures/<dir>/`, if it was written.
    fn recording(&self, dir: &str, name: &str) -> Option<String> {
        fs::read_to_string(self.checkout.join("tests/fixtures").join(dir).join(name)).ok()
    }

    /// What the fake cargo was asked, one line per run.
    fn cargo_runs(&self) -> Vec<String> {
        fs::read_to_string(self.root().join("cargo.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }
}

/// The real `name`, found on the test's own PATH.
fn which(name: &str) -> PathBuf {
    let path = std::env::var_os("PATH").unwrap();
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .unwrap_or_else(|| panic!("no {name} on PATH"))
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

#[test]
fn recordings_are_written_normalised_and_versioned_and_a_diverging_suite_is_named() {
    let live = Live::new();
    let out = live.check_live(&[], Some("hooks"));
    let (text, errors) = (stdout(&out), stderr(&out));
    assert_eq!(out.status.code(), Some(1), "{text}{errors}");

    let session = "\"session_id\":\"s-1\",\"transcript_path\":\"{{transcripts}}/s-1.jsonl\",\"cwd\":\"{{worktree}}\"";
    assert_eq!(
        live.recording("claude-hooks", "fake-hello-2.1.999.log")
            .unwrap(),
        format!(
            "SessionStart {{{session},\"hook_event_name\":\"SessionStart\",\"source\":\"startup\"}}\n\
             UserPromptSubmit {{{session},\"prompt_id\":\"p1\",\"hook_event_name\":\"UserPromptSubmit\",\"prompt\":\"reply ok\"}}\n\
             PreToolUse {{{session},\"prompt_id\":\"p1\",\"hook_event_name\":\"PreToolUse\",\"tool_name\":\"Bash\",\"tool_input\":{{\"command\":\"touch {{{{denied}}}}/x\"}}}}\n\
             Stop {{{session},\"prompt_id\":\"p1\",\"hook_event_name\":\"Stop\",\"last_assistant_message\":\"ok\"}}\n\
             SessionEnd {{{session},\"hook_event_name\":\"SessionEnd\",\"reason\":\"other\"}}\n"
        ),
        "claude's hooks, their paths made placeholders, named after its version"
    );
    assert_eq!(
        live.recording("claude-hooks", "fake-hello-2.1.300.log")
            .unwrap(),
        OLDER,
        "the older version is left beside it"
    );
    assert_eq!(
        live.recording("opencode-events", "fake-oc-1.18.5.log")
            .unwrap(),
        "{\"type\":\"start\"}\n\
         {\"type\":\"prompt\",\"session\":\"ses_1\",\"parent\":null,\"text\":\"look\"}\n\
         {\"type\":\"tool\",\"session\":\"ses_1\",\"parent\":null,\"tool\":\"read\",\"input\":{\"filePath\":\"{{worktree}}/README.md\"}}\n\
         {\"type\":\"idle\",\"session\":\"ses_1\",\"parent\":null,\"text\":\"ok\",\"finish\":\"stop\",\"error\":null}\n\
         {\"type\":\"end\"}\n",
        "opencode's lines, in a directory of their own"
    );
    assert!(
        live.recording("claude-hooks", "fake-stuck-2.1.999.log")
            .is_none(),
        "a failed scenario writes nothing"
    );

    check_summary(&live, &text);
}

/// The summary of the run with every scenario, the suite `hooks` diverging.
fn check_summary(live: &Live, text: &str) {
    assert!(
        text.starts_with("check-live: claude 2.1.999, opencode 1.18.5\n"),
        "{text}"
    );
    for line in [
        "  manual    fake-by-hand  recorded by hand: just record-live fake-by-hand\n",
        "  recorded  fake-hello    tests/fixtures/claude-hooks/fake-hello-2.1.999.log\n",
        "  recorded  fake-oc       tests/fixtures/opencode-events/fake-oc-1.18.5.log\n",
        "  failed    fake-stuck    timed out after 3 s waiting for PermissionRequest (kept ",
        "  passed    recordings\n",
        "  diverged  hooks         replays_the_new_recording\n",
        "  passed    doctor\n",
        "fixtures (git status --short):\n  ?? tests/fixtures/claude-hooks/fake-hello-2.1.999.log\n  ?? tests/fixtures/opencode-events/\n",
    ] {
        assert!(text.contains(line), "no `{line}` in:\n{text}");
    }
    assert!(
        text.ends_with("check-live: diverged: hooks; failed: fake-stuck\n"),
        "{text}"
    );
    assert_eq!(
        live.cargo_runs(),
        ["recordings", "hooks", "snapshots", "opencode", "doctor"]
            .map(|suite| format!("test --locked --quiet --test {suite}")),
        "every suite that replays the recordings, with cargo test"
    );
}

#[test]
fn every_scenario_recorded_and_every_suite_passed_exits_0() {
    let live = Live::new();
    let out = live.check_live(&["fake-hello"], None);
    let text = stdout(&out);
    assert_eq!(out.status.code(), Some(0), "{text}{}", stderr(&out));
    assert!(
        text.starts_with("check-live: claude 2.1.999\n"),
        "only the agent driven: {text}"
    );
    assert!(!text.contains("fake-oc"), "only the scenario named: {text}");
    assert!(
        text.ends_with("check-live: every scenario recorded, every suite passed\n"),
        "{text}"
    );
}

#[test]
fn a_missing_agent_is_refused_before_anything_runs() {
    let live = Live::new();
    fs::remove_file(live.bin.join("opencode")).unwrap();
    let out = live.check_live(&[], None);
    assert_eq!(out.status.code(), Some(2));
    assert!(out.stdout.is_empty());
    assert_eq!(
        stderr(&out),
        "check-live: refused (orca-term doctor tells more):\n  \
         opencode is not installed, and the chosen scenarios need it\n"
    );
    assert!(
        live.recording("claude-hooks", "fake-hello-2.1.999.log")
            .is_none()
    );
    assert!(live.cargo_runs().is_empty(), "no suite ran");

    let out = live.check_live(&["fake-hello"], None);
    assert_eq!(
        out.status.code(),
        Some(0),
        "claude's scenarios alone need no opencode"
    );
}

#[test]
fn a_scenario_recorded_by_hand_shows_its_procedure_and_is_recorded_once_the_person_detaches() {
    let live = Live::new();
    let out = live.record_live("fake-by-hand");
    let errors = stderr(&out);
    assert_eq!(out.status.code(), Some(0), "{errors}");
    assert!(
        errors.starts_with("record-live: fake-by-hand (claude-code), in a throwaway repository:\n"),
        "{errors}"
    );
    let text = stdout(&out);
    assert!(
        text.ends_with(
            "record-live: recorded tests/fixtures/claude-hooks/fake-by-hand-2.1.999.log\n\
             review it with git diff, then run just check-live\n"
        ),
        "after what the attach showed of the agent: {text}"
    );
    let recording = live
        .recording("claude-hooks", "fake-by-hand-2.1.999.log")
        .unwrap();
    let kinds: Vec<&str> = recording
        .lines()
        .map(|line| line.split(' ').next().unwrap())
        .collect();
    assert_eq!(kinds, ["SessionStart", "SessionEnd"], "{recording}");
    assert!(
        live.cargo_runs().is_empty(),
        "no suite runs after a recording by hand"
    );
}
