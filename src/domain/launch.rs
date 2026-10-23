//! Domain: how an agent is launched in a worktree (ADR 0007, ADR 0009). For claude: its
//! arguments, the settings file that turns its native sandbox on, keeps our state out of its reach
//! and registers the hooks that report its turns (ADR 0008: per launch, never in claude's own
//! config), and the variables kept from its environment. For opencode (#35), `launch/opencode.rs`:
//! its command line inside the sandbox orca-term wraps it in, and the plugin that reports its
//! turns. For both: the session a cold start resumes, and the guard that no launch ever bypasses
//! permissions. The versions they need are `domain/prerequisite.rs`'s.
//!
//! The forbidden flags and modes are listed here and nowhere else in the source:
//! `cargo xtask launch-guard` rejects their literals in any other file.

pub(crate) mod opencode;

use std::fmt;
use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::domain::agent::Agent;
use crate::domain::error::{GoneReason, TypedError};
use crate::domain::hook;
use crate::domain::state::ADMIN_DIR_NAME;
use crate::domain::trust::TRUST_FILE;
use crate::domain::turn::Status;

/// The permission mode every launch runs under: edits inside the worktree go through without
/// asking; the sandbox confines everything else (ADR 0007).
pub(crate) const PERMISSION_MODE: &str = "acceptEdits";

/// Variables removed from the agent's environment, so that an orca-term run from inside a Claude
/// Code session never starts the agent as a child session of it. Everything else passes through:
/// auth and config variables must.
pub(crate) const STRIPPED_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
];

/// Flags no launch may carry: claude's, and opencode's (whose TUI takes `--auto`, `--yolo` and
/// `--dangerously-skip-permissions` alike, measured in its 1.18.0 source). `--permission-mode` is
/// allowed only with [`PERMISSION_MODE`].
const FORBIDDEN_FLAGS: &[&str] = &[
    "--dangerously-skip-permissions",
    "--allow-dangerously-skip-permissions",
    "--auto",
    "--yolo",
];

/// Values `permissions.defaultMode` may never take in the settings.
const FORBIDDEN_MODES: &[&str] = &["bypassPermissions", "auto"];

/// The program every hook runs: a shell, given [`HOOK_SCRIPT`] and its arguments (exec form, so
/// no path is ever quoted into a command line).
pub(crate) const HOOK_SHELL: &str = "/bin/sh";

/// The script every hook runs through [`HOOK_SHELL`]: orca-term (`$0`, by absolute path) with the
/// worktree's admin directory (`$1`) and the event (`$2`), its output thrown away and its exit
/// status replaced by 0. claude takes a hook's exit status 2 as a refusal (it rejects the prompt,
/// or keeps working after `Stop`) and its stdout on some events as extra context, so whatever
/// orca-term does, even missing, too old or crashing, reaches claude as a silent success.
pub(crate) const HOOK_SCRIPT: &str =
    r#""$0" hook claude --admin "$1" "$2" >/dev/null 2>&1; exit 0"#;

/// How long claude lets a hook run, in seconds, before cancelling it and carrying on.
pub(crate) const HOOK_TIMEOUT_SECS: u64 = 5;

/// The flag that continues a recorded session under its own id. claude's `--fork-session`, which
/// would give it a new id, is never used: the worktree keeps one current session.
const RESUME_FLAG: &str = "--resume";

/// Every flag that names a session to continue, whose value the guard checks is a [`SessionId`]:
/// claude's `--resume`, and opencode's `-s` (also spelt `--session`).
const SESSION_FLAGS: &[&str] = &[RESUME_FLAG, "-s", "--session"];

/// The longest session id ever put on claude's command line.
const MAX_SESSION_ID_CHARS: usize = 128;

/// The command line claude is started with: its settings file and the permission mode, and,
/// when `resume` names a session, `--resume <id>` to continue it.
pub(crate) fn claude_argv(
    program: &str,
    settings_file: &Path,
    resume: Option<&SessionId>,
) -> Vec<String> {
    let mut argv = vec![
        program.to_owned(),
        "--settings".to_owned(),
        settings_file.display().to_string(),
        "--permission-mode".to_owned(),
        PERMISSION_MODE.to_owned(),
    ];
    if let Some(session) = resume {
        argv.push(RESUME_FLAG.to_owned());
        argv.push(session.as_str().to_owned());
    }
    argv
}

/// A session id that is safe to put on an agent's command line: 1 to 128 characters among ASCII
/// letters, digits, `_` and `-`, not starting with `-`. claude's ids are UUIDs and opencode's read
/// `ses_` and letters and digits, which pass. The rule keeps whatever `status.json` holds from
/// ever reading as a flag (an "id" such as `--dangerously-skip-permissions`) or as anything but
/// one argument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionId(String);

impl SessionId {
    /// `text` as a session id, or `None` when it breaks the rule.
    pub(crate) fn parse(text: &str) -> Option<Self> {
        let allowed = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
        let ok = !text.is_empty()
            && text.len() <= MAX_SESSION_ID_CHARS
            && !text.starts_with('-')
            && text.chars().all(allowed);
        ok.then(|| Self(text.to_owned()))
    }

    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for SessionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The session a cold start would continue: its id, and the transcript claude keeps it in
/// (`None` for opencode, which keeps its sessions in a database of its own).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Resume {
    pub(crate) session: SessionId,
    pub(crate) transcript: Option<PathBuf>,
}

/// What a cold start of `agent` in the worktree `name` does with the session `status` records
/// (`None` when `status.json` is missing or cannot be used): nothing to resume without a recorded
/// session, or with a session another agent recorded (the worktree's agent was swapped), so a new
/// session starts; else the session to resume. Fails as `session_gone` when the recorded id is not
/// a [`SessionId`] or, for claude, no absolute transcript path is recorded for it.
pub(crate) fn session_to_resume(
    name: &str,
    status: Option<&Status>,
    agent: Agent,
) -> Result<Option<Resume>, TypedError> {
    let Some(status) = status else {
        return Ok(None);
    };
    let Some(recorded) = status.session.as_deref() else {
        return Ok(None);
    };
    if status.session_agent() != agent.program() {
        return Ok(None);
    }
    let gone = |reason| TypedError::SessionGone {
        name: name.to_owned(),
        session: recorded.to_owned(),
        reason,
    };
    let session = SessionId::parse(recorded).ok_or_else(|| gone(GoneReason::InvalidId))?;
    let transcript = match agent {
        Agent::ClaudeCode => Some(
            status
                .transcript
                .clone()
                .filter(|path| path.is_absolute())
                .ok_or_else(|| gone(GoneReason::NoTranscript))?,
        ),
        Agent::OpenCode => None,
    };
    Ok(Some(Resume {
        session,
        transcript,
    }))
}

/// The session to put on the agent's command line, once the caller has looked whether its
/// transcript, when it has one (claude's), is still on disk (`transcript_exists`). Fails as
/// `session_gone` when it is not.
///
/// Why look first: claude resumes a session from its transcript, and refuses one it cannot find.
/// Measured with claude 2.1.301: `claude --resume 00000000-0000-4000-8000-000000000000 --print
/// hello` exits 1 at once, before any API call, saying on stderr that it found no session with
/// that id. Launched in a holder, such an agent would end as soon as it starts, leaving the
/// person a dead terminal instead of the reason and the fix.
pub(crate) fn check_transcript(
    name: &str,
    resume: Resume,
    transcript_exists: bool,
) -> Result<SessionId, TypedError> {
    match resume.transcript {
        Some(transcript) if !transcript_exists => Err(TypedError::SessionGone {
            name: name.to_owned(),
            session: resume.session.as_str().to_owned(),
            reason: GoneReason::TranscriptGone(transcript),
        }),
        _ => Ok(resume.session),
    }
}

/// The settings passed to claude as `--settings <file>`: its sandbox on and set to fail rather
/// than run unsandboxed, the network open, and writes to every worktree's admin directory,
/// `refs/orca-term`, `orca-term.yaml`, `orca-term-trust.json` (the approved setup scripts) and the
/// `orca-term` directory (the registry of running Neovims, which the core connects to) in the git
/// common dir denied, from sandboxed commands
/// and from the Edit tool alike. `admin_dirs` are the admin directories that exist at launch:
/// Linux ignores the `*` entries, so each one is listed concretely too. `writable` are the extra
/// paths the config allows, absolute. `unix_sockets` are the sockets the agent may connect to: the
/// person's signing agent's, when `sandbox.agent_socket` is on (#105), listed as
/// `allowUnixSockets`, which is absent when there are none; `allowAllUnixSockets` stays off
/// whatever they are (claude's Linux sandbox honours only that one, so there the setting does
/// nothing). `hooks` reports the turns to the worktree whose admin directory it names
/// ([`HookTarget`]).
///
/// Every weakening key is set explicitly, since a scalar in `--settings` overrides the same key
/// in the person's own files. The Edit rules start with `//` because a single `/` would mean a
/// path relative to the settings file.
pub(crate) fn claude_settings(
    common_dir: &Path,
    admin_dirs: &[PathBuf],
    writable: &[PathBuf],
    unix_sockets: &[PathBuf],
    hooks: &HookTarget,
) -> Value {
    let common = common_dir.display();
    let mut deny_write: Vec<String> = admin_dirs.iter().map(|d| d.display().to_string()).collect();
    deny_write.extend([
        format!("{common}/worktrees/*/{ADMIN_DIR_NAME}"),
        format!("{common}/refs/{ADMIN_DIR_NAME}"),
        format!("{common}/orca-term.yaml"),
        format!("{common}/{TRUST_FILE}"),
        format!("{common}/{ADMIN_DIR_NAME}"),
    ]);
    let allow_write: Vec<String> = writable.iter().map(|p| p.display().to_string()).collect();
    let mut network = json!({
        "allowedDomains": ["*"],
        "allowAllUnixSockets": false,
    });
    if !unix_sockets.is_empty() {
        let sockets: Vec<String> = unix_sockets
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        network["allowUnixSockets"] = json!(sockets);
    }
    json!({
        "sandbox": {
            "enabled": true,
            "failIfUnavailable": true,
            "autoAllowBashIfSandboxed": true,
            "allowUnsandboxedCommands": false,
            "enableWeakerNestedSandbox": false,
            "allowAppleEvents": false,
            "filesystem": {
                "disabled": false,
                "allowWrite": allow_write,
                "denyWrite": deny_write,
            },
            "network": network,
        },
        "permissions": {
            "disableBypassPermissionsMode": "disable",
            "disableAutoMode": "disable",
            "deny": [
                format!("Edit(/{common}/worktrees/*/{ADMIN_DIR_NAME}/**)"),
                format!("Edit(/{common}/refs/{ADMIN_DIR_NAME}/**)"),
                format!("Edit(/{common}/orca-term.yaml)"),
                format!("Edit(/{common}/{TRUST_FILE})"),
                format!("Edit(/{common}/{ADMIN_DIR_NAME}/**)"),
            ],
        },
        "hooks": hooks_settings(hooks),
    })
}

/// Where the hooks report to: the running orca-term and the worktree's admin directory, both
/// absolute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HookTarget {
    pub(crate) orca_term: PathBuf,
    pub(crate) admin: PathBuf,
}

/// The `hooks` section of the settings: for each event in [`hook::REGISTERED`], with its matcher,
/// one synchronous command handler running [`HOOK_SCRIPT`] for that event. Not `async`: the
/// events must reach the core in the order claude sends them.
fn hooks_settings(target: &HookTarget) -> Value {
    let mut hooks = serde_json::Map::new();
    for (event, matcher) in hook::REGISTERED {
        let handler = json!({
            "type": "command",
            "command": HOOK_SHELL,
            "args": [
                "-c",
                HOOK_SCRIPT,
                target.orca_term.display().to_string(),
                target.admin.display().to_string(),
                event,
            ],
            "timeout": HOOK_TIMEOUT_SECS,
        });
        let entry = match matcher {
            Some(matcher) => json!({"matcher": matcher, "hooks": [handler]}),
            None => json!({"hooks": [handler]}),
        };
        hooks.insert((*event).to_owned(), json!([entry]));
    }
    Value::Object(hooks)
}

/// The settings as the file's bytes, readable by a person.
pub(crate) fn settings_bytes(settings: &Value) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(settings)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Refuses a launch that bypasses permissions: an argument among [`FORBIDDEN_FLAGS`], a
/// `--permission-mode` other than [`PERMISSION_MODE`] (as two arguments or as `--permission-mode=`),
/// a `--resume` whose value is not a [`SessionId`], or settings whose `permissions.defaultMode`
/// is one of [`FORBIDDEN_MODES`]. Arguments are compared whole, never as substrings: the settings
/// legitimately name `disableBypassPermissionsMode`. Every argument is checked, the values of
/// flags included, so a forbidden flag hidden as a value is still refused.
pub(crate) fn check_launch(argv: &[String], settings: &Value) -> Result<()> {
    let rest = argv.get(1..).unwrap_or_default();
    for (at, arg) in rest.iter().enumerate() {
        let value = rest.get(at + 1).map(String::as_str);
        if FORBIDDEN_FLAGS.contains(&arg.as_str()) {
            bail!("refusing to launch the agent with `{arg}`, which bypasses permissions");
        }
        check_permission_mode(arg, value)?;
        check_resume(arg, value)?;
    }
    if let Some(mode) = settings
        .pointer("/permissions/defaultMode")
        .and_then(Value::as_str)
        && FORBIDDEN_MODES.contains(&mode)
    {
        bail!(
            "refusing to launch the agent with settings whose `permissions.defaultMode` is `{mode}`"
        );
    }
    Ok(())
}

/// Refuses `arg` when it is `--permission-mode` (followed by `value`) or `--permission-mode=...`
/// with any mode but [`PERMISSION_MODE`].
fn check_permission_mode(arg: &str, value: Option<&str>) -> Result<()> {
    let mode = if arg == "--permission-mode" {
        Some(value.unwrap_or_default())
    } else {
        arg.strip_prefix("--permission-mode=")
    };
    if let Some(mode) = mode
        && mode != PERMISSION_MODE
    {
        bail!(
            "refusing to launch the agent with `--permission-mode {mode}`; only \
             `{PERMISSION_MODE}` is allowed"
        );
    }
    Ok(())
}

/// Refuses `arg` when it is a flag of [`SESSION_FLAGS`] (followed by `value`), or one of its
/// long forms written `--flag=...`, with a value that is not a [`SessionId`].
fn check_resume(arg: &str, value: Option<&str>) -> Result<()> {
    for flag in SESSION_FLAGS {
        let session = if arg == *flag {
            Some(value.unwrap_or_default())
        } else if flag.starts_with("--") {
            arg.strip_prefix(*flag)
                .and_then(|rest| rest.strip_prefix('='))
        } else {
            None
        };
        if let Some(session) = session
            && SessionId::parse(session).is_none()
        {
            bail!("refusing to launch the agent with `{flag} {session}`, not a session id");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target() -> HookTarget {
        HookTarget {
            orca_term: PathBuf::from("/opt/orca term/bin/orca-term"),
            admin: PathBuf::from("/r/.git/worktrees/fix/orca-term"),
        }
    }

    fn argv(args: &[&str]) -> Vec<String> {
        std::iter::once("claude")
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn the_argv_passes_the_settings_file_and_accept_edits() {
        assert_eq!(
            claude_argv("claude", Path::new("/a/claude-settings.json"), None),
            [
                "claude",
                "--settings",
                "/a/claude-settings.json",
                "--permission-mode",
                "acceptEdits"
            ]
        );
        let settings = claude_settings(Path::new("/r/.git"), &[], &[], &[], &target());
        check_launch(
            &claude_argv("claude", Path::new("/a/claude-settings.json"), None),
            &settings,
        )
        .unwrap();
    }

    const UUID: &str = "0b983a87-4074-466e-b864-422ce6f063c8";

    #[test]
    fn a_resumed_launch_adds_resume_and_its_session_and_passes_the_guard() {
        let session = SessionId::parse(UUID).unwrap();
        let argv = claude_argv("claude", Path::new("/a/s.json"), Some(&session));
        assert_eq!(
            argv,
            [
                "claude",
                "--settings",
                "/a/s.json",
                "--permission-mode",
                "acceptEdits",
                "--resume",
                UUID
            ]
        );
        let settings = claude_settings(Path::new("/r/.git"), &[], &[], &[], &target());
        check_launch(&argv, &settings).unwrap();
    }

    #[test]
    fn a_session_id_is_letters_digits_underscore_and_dash_not_leading() {
        assert_eq!(SessionId::parse(UUID).unwrap().as_str(), UUID);
        assert_eq!(SessionId::parse(UUID).unwrap().to_string(), UUID);
        assert!(SessionId::parse("a_b-C9").is_some());
        assert!(SessionId::parse(&"a".repeat(128)).is_some());
        for bad in [
            "",
            "-x",
            "--dangerously-skip-permissions",
            "a b",
            "a/b",
            "../x",
            "a\nb",
            "é",
            "a=b",
        ] {
            assert_eq!(SessionId::parse(bad), None, "{bad:?}");
        }
        assert_eq!(SessionId::parse(&"a".repeat(129)), None, "too long");
    }

    #[test]
    fn the_guard_refuses_bypasses_beside_resume_and_a_resume_that_is_not_an_id() {
        let settings = claude_settings(Path::new("/r/.git"), &[], &[], &[], &target());
        let refused = |args: &[&str]| check_launch(&argv(args), &settings).is_err();
        assert!(refused(&["--resume", "--dangerously-skip-permissions"]));
        assert!(refused(&[
            "--resume",
            "--allow-dangerously-skip-permissions"
        ]));
        assert!(refused(&["--resume", "--auto"]));
        assert!(refused(&["--resume=--auto"]));
        assert!(refused(&[
            "--resume",
            UUID,
            "--dangerously-skip-permissions"
        ]));
        assert!(refused(&["--resume", UUID, "--permission-mode", "plan"]));
        assert!(refused(&[
            "--resume",
            UUID,
            "--permission-mode=bypassPermissions"
        ]));
        assert!(refused(&["--resume", "a b"]));
        assert!(refused(&["--resume"]), "a resume without its id");
        check_launch(&argv(&["--resume", UUID]), &settings).unwrap();
        check_launch(&argv(&[&format!("--resume={UUID}")]), &settings).unwrap();
        let err = check_launch(&argv(&["--resume", "-x"]), &settings)
            .unwrap_err()
            .to_string();
        assert!(err.contains("`--resume -x`, not a session id"), "{err}");
    }

    fn recorded(session: &str, transcript: Option<&str>) -> Status {
        Status {
            session: Some(session.to_owned()),
            transcript: transcript.map(PathBuf::from),
            ..Status::default()
        }
    }

    fn gone_reason(result: Result<Option<Resume>, TypedError>) -> GoneReason {
        match result {
            Err(TypedError::SessionGone { reason, .. }) => reason,
            other => panic!("expected session_gone, got {other:?}"),
        }
    }

    const CLAUDE: Agent = Agent::ClaudeCode;

    #[test]
    fn a_cold_start_resumes_the_recorded_session_or_starts_a_new_one() {
        assert_eq!(session_to_resume("fix", None, CLAUDE), Ok(None));
        assert_eq!(
            session_to_resume("fix", Some(&Status::default()), CLAUDE),
            Ok(None),
            "no session recorded"
        );
        let status = recorded(UUID, Some("/h/t/s.jsonl"));
        assert_eq!(
            session_to_resume("fix", Some(&status), CLAUDE),
            Ok(Some(Resume {
                session: SessionId::parse(UUID).unwrap(),
                transcript: Some(PathBuf::from("/h/t/s.jsonl")),
            }))
        );
    }

    #[test]
    fn a_session_is_resumed_only_by_the_agent_that_recorded_it() {
        let claudes = recorded(UUID, Some("/h/t/s.jsonl"));
        assert_eq!(
            session_to_resume("fix", Some(&claudes), Agent::OpenCode),
            Ok(None),
            "a session recorded before the agent was named is claude's"
        );
        let opencodes = Status {
            agent: Some("opencode".to_owned()),
            ..recorded("ses_2Ab9", None)
        };
        assert_eq!(
            session_to_resume("fix", Some(&opencodes), Agent::OpenCode),
            Ok(Some(Resume {
                session: SessionId::parse("ses_2Ab9").unwrap(),
                transcript: None,
            })),
            "opencode keeps no transcript to look at"
        );
        assert_eq!(session_to_resume("fix", Some(&opencodes), CLAUDE), Ok(None));
        let flag = Status {
            agent: Some("opencode".to_owned()),
            ..recorded("-x", None)
        };
        assert_eq!(
            gone_reason(session_to_resume("fix", Some(&flag), Agent::OpenCode)),
            GoneReason::InvalidId
        );
        let resume = session_to_resume("fix", Some(&opencodes), Agent::OpenCode)
            .unwrap()
            .unwrap();
        assert_eq!(
            check_transcript("fix", resume, false),
            Ok(SessionId::parse("ses_2Ab9").unwrap()),
            "with no transcript there is nothing to find gone"
        );
    }

    #[test]
    fn a_session_with_a_bad_id_or_no_transcript_cannot_be_resumed() {
        let flag = recorded("--dangerously-skip-permissions", Some("/h/t/s.jsonl"));
        assert_eq!(
            gone_reason(session_to_resume("fix", Some(&flag), CLAUDE)),
            GoneReason::InvalidId
        );
        assert_eq!(
            gone_reason(session_to_resume(
                "fix",
                Some(&recorded(UUID, None)),
                CLAUDE
            )),
            GoneReason::NoTranscript
        );
        assert_eq!(
            gone_reason(session_to_resume(
                "fix",
                Some(&recorded(UUID, Some("t/s.jsonl"))),
                CLAUDE
            )),
            GoneReason::NoTranscript,
            "a relative path is not trusted"
        );
    }

    #[test]
    fn a_session_whose_transcript_is_gone_cannot_be_resumed() {
        let resume = Resume {
            session: SessionId::parse(UUID).unwrap(),
            transcript: Some(PathBuf::from("/h/t/s.jsonl")),
        };
        assert_eq!(
            check_transcript("fix", resume.clone(), true),
            Ok(SessionId::parse(UUID).unwrap())
        );
        assert_eq!(
            check_transcript("fix", resume, false),
            Err(TypedError::SessionGone {
                name: "fix".to_owned(),
                session: UUID.to_owned(),
                reason: GoneReason::TranscriptGone(PathBuf::from("/h/t/s.jsonl")),
            })
        );
    }

    #[test]
    fn the_settings_turn_the_sandbox_on_and_deny_our_state() {
        let common = Path::new("/r/.git");
        let admin = [PathBuf::from("/r/.git/worktrees/fix/orca-term")];
        let writable = [PathBuf::from("/home/me/.cache/pnpm")];
        let settings = claude_settings(common, &admin, &writable, &[], &target());
        let at = |pointer: &str| settings.pointer(pointer).cloned().unwrap_or(Value::Null);
        assert_eq!(at("/sandbox/enabled"), json!(true));
        assert_eq!(at("/sandbox/failIfUnavailable"), json!(true));
        assert_eq!(at("/sandbox/autoAllowBashIfSandboxed"), json!(true));
        assert_eq!(at("/sandbox/allowUnsandboxedCommands"), json!(false));
        assert_eq!(at("/sandbox/enableWeakerNestedSandbox"), json!(false));
        assert_eq!(at("/sandbox/allowAppleEvents"), json!(false));
        assert_eq!(at("/sandbox/filesystem/disabled"), json!(false));
        assert_eq!(
            at("/sandbox/filesystem/allowWrite"),
            json!(["/home/me/.cache/pnpm"])
        );
        assert_eq!(
            at("/sandbox/filesystem/denyWrite"),
            json!([
                "/r/.git/worktrees/fix/orca-term",
                "/r/.git/worktrees/*/orca-term",
                "/r/.git/refs/orca-term",
                "/r/.git/orca-term.yaml",
                "/r/.git/orca-term-trust.json",
                "/r/.git/orca-term"
            ])
        );
        assert_eq!(at("/sandbox/network/allowedDomains"), json!(["*"]));
        assert_eq!(at("/sandbox/network/allowAllUnixSockets"), json!(false));
        assert_eq!(
            at("/sandbox/network/allowUnixSockets"),
            Value::Null,
            "no socket unless the signing agent's is given"
        );
        assert_eq!(
            at("/permissions/disableBypassPermissionsMode"),
            json!("disable")
        );
        assert_eq!(at("/permissions/disableAutoMode"), json!("disable"));
        assert_eq!(
            at("/permissions/deny"),
            json!([
                "Edit(//r/.git/worktrees/*/orca-term/**)",
                "Edit(//r/.git/refs/orca-term/**)",
                "Edit(//r/.git/orca-term.yaml)",
                "Edit(//r/.git/orca-term-trust.json)",
                "Edit(//r/.git/orca-term/**)"
            ])
        );
        assert_eq!(
            at("/permissions/defaultMode"),
            Value::Null,
            "the flag decides the mode"
        );
        let text = String::from_utf8(settings_bytes(&settings).unwrap()).unwrap();
        assert!(text.ends_with("}\n"));
    }

    #[test]
    fn the_settings_allow_the_signing_agents_socket_only_when_given_and_only_that_path() {
        let socket = [PathBuf::from("/private/var/folders/x/T/agent.sock")];
        let with = claude_settings(Path::new("/r/.git"), &[], &[], &socket, &target());
        assert_eq!(
            with["sandbox"]["network"],
            json!({
                "allowedDomains": ["*"],
                "allowAllUnixSockets": false,
                "allowUnixSockets": ["/private/var/folders/x/T/agent.sock"],
            })
        );
        let mut without = claude_settings(Path::new("/r/.git"), &[], &[], &[], &target());
        without["sandbox"]["network"]["allowUnixSockets"] =
            json!(["/private/var/folders/x/T/agent.sock"]);
        assert_eq!(with, without, "nothing else changes");
    }

    #[test]
    fn the_settings_register_a_silent_hook_for_every_turn_event() {
        let settings = claude_settings(Path::new("/r/.git"), &[], &[], &[], &target());
        let hooks = settings["hooks"].as_object().unwrap();
        let events: Vec<&str> = hooks.keys().map(String::as_str).collect();
        let mut expected = vec![
            "PermissionRequest",
            "PostToolBatch",
            "PostToolUse",
            "PostToolUseFailure",
            "PreToolUse",
            "SessionEnd",
            "SessionStart",
            "Stop",
            "StopFailure",
            "UserPromptSubmit",
        ];
        expected.sort_unstable();
        assert_eq!(events, expected, "exactly these events");
        for (event, entries) in hooks {
            let entries = entries.as_array().unwrap();
            assert_eq!(entries.len(), 1, "{event}");
            let entry = &entries[0];
            let tool_event = [
                "PreToolUse",
                "PermissionRequest",
                "PostToolUse",
                "PostToolUseFailure",
            ]
            .contains(&event.as_str());
            if tool_event {
                assert_eq!(entry["matcher"], "*", "{event} matches every tool");
            } else {
                assert_eq!(entry.get("matcher"), None, "{event} has no matcher");
            }
            let handlers = entry["hooks"].as_array().unwrap();
            assert_eq!(handlers.len(), 1, "{event}");
            check_handler(event, &handlers[0]);
        }
        assert_eq!(
            settings.get("disableAllHooks"),
            None,
            "a person or repository that disabled hooks is respected"
        );
    }

    /// One hook handler: `/bin/sh` in exec form with absolute paths, its output discarded, exit 0
    /// always, a timeout of at most 5 s, synchronous.
    fn check_handler(event: &str, handler: &Value) {
        assert_eq!(handler["type"], "command");
        assert_eq!(handler["command"], "/bin/sh", "exec form through sh");
        assert_eq!(
            handler["args"],
            json!([
                "-c",
                "\"$0\" hook claude --admin \"$1\" \"$2\" >/dev/null 2>&1; exit 0",
                "/opt/orca term/bin/orca-term",
                "/r/.git/worktrees/fix/orca-term",
                event
            ]),
            "{event}: absolute paths, output discarded, always exit 0"
        );
        let script = handler["args"][1].as_str().unwrap();
        assert!(script.ends_with("; exit 0"), "{script}");
        assert!(handler["args"][2].as_str().unwrap().starts_with('/'));
        assert!(handler["args"][3].as_str().unwrap().starts_with('/'));
        assert!(handler["timeout"].as_u64().unwrap() <= 5, "{event}");
        assert_eq!(handler.get("async"), None, "{event} runs synchronously");
        let keys: Vec<&str> = handler
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, ["args", "command", "timeout", "type"], "{event}");
    }

    #[test]
    fn a_launch_bypassing_permissions_is_refused_by_whole_argument() {
        let settings = claude_settings(Path::new("/r/.git"), &[], &[], &[], &target());
        for flag in [
            "--dangerously-skip-permissions",
            "--allow-dangerously-skip-permissions",
            "--auto",
            "--yolo",
        ] {
            let err = check_launch(&argv(&[flag]), &settings).unwrap_err();
            assert!(err.to_string().contains(flag), "{err}");
        }
        assert!(
            check_launch(
                &argv(&["--permission-mode", "bypassPermissions"]),
                &settings
            )
            .is_err()
        );
        assert!(check_launch(&argv(&["--permission-mode=auto"]), &settings).is_err());
        assert!(check_launch(&argv(&["--permission-mode", "plan"]), &settings).is_err());
        check_launch(&argv(&["--permission-mode", "acceptEdits"]), &settings).unwrap();
        check_launch(&argv(&["--permission-mode=acceptEdits"]), &settings).unwrap();
        // Compared as whole arguments: a longer argument containing a flag is not that flag.
        check_launch(&argv(&["--autocomplete"]), &settings).unwrap();
    }

    #[test]
    fn opencodes_session_flag_takes_only_a_session_id_and_its_bypasses_are_refused() {
        let opencode = |args: &[&str]| -> Vec<String> {
            ["sandbox-exec", "-f", "/a/opencode-sandbox.sb", "opencode"]
                .iter()
                .chain(args)
                .map(|arg| (*arg).to_owned())
                .collect()
        };
        check_launch(&opencode(&[]), &Value::Null).unwrap();
        check_launch(&opencode(&["-s", "ses_2Ab9"]), &Value::Null).unwrap();
        check_launch(&opencode(&["--session=ses_2Ab9"]), &Value::Null).unwrap();
        for bad in [
            &["-s", "--yolo"][..],
            &["-s"],
            &["--session", "a b"],
            &["--session=--auto"],
            &["-s", "ses_2Ab9", "--yolo"],
            &["--dangerously-skip-permissions"],
            &["--auto"],
        ] {
            assert!(
                check_launch(&opencode(bad), &Value::Null).is_err(),
                "{bad:?}"
            );
        }
        let err = check_launch(&opencode(&["-s", "-x"]), &Value::Null)
            .unwrap_err()
            .to_string();
        assert!(err.contains("`-s -x`, not a session id"), "{err}");
    }

    #[test]
    fn settings_defaulting_to_a_bypass_mode_are_refused() {
        let good = claude_settings(Path::new("/r/.git"), &[], &[], &[], &target());
        for mode in ["bypassPermissions", "auto"] {
            let bad = json!({"permissions": {"defaultMode": mode}});
            let err = check_launch(&argv(&[]), &bad).unwrap_err();
            assert!(err.to_string().contains(mode), "{err}");
        }
        check_launch(
            &argv(&[]),
            &json!({"permissions": {"defaultMode": "acceptEdits"}}),
        )
        .unwrap();
        check_launch(&argv(&[]), &good).unwrap();
    }

    #[test]
    fn only_the_session_variables_are_stripped() {
        assert_eq!(
            STRIPPED_VARS,
            [
                "CLAUDECODE",
                "CLAUDE_CODE_ENTRYPOINT",
                "CLAUDE_CODE_SESSION_ID",
                "CLAUDE_CODE_CHILD_SESSION"
            ]
        );
    }
}
