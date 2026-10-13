//! Domain: how claude is launched in a worktree (ADR 0007, ADR 0009): its arguments, the settings
//! file that turns its native sandbox on and keeps our state out of its reach, the version it
//! needs, the variables kept from its environment, and the guard that no launch ever bypasses
//! permissions.
//!
//! The forbidden flags and modes are listed here and nowhere else in the source:
//! `cargo xtask launch-guard` rejects their literals in any other file.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};
use serde_json::{Value, json};

use crate::domain::state::ADMIN_DIR_NAME;

/// The permission mode every launch runs under: edits inside the worktree go through without
/// asking; the sandbox confines everything else (ADR 0007).
pub(crate) const PERMISSION_MODE: &str = "acceptEdits";

/// The oldest claude whose sandbox honours our settings as "admin-required", so the repository's
/// own `.claude/settings*.json` cannot loosen them.
pub(crate) const MIN_CLAUDE_VERSION: (u64, u64, u64) = (2, 1, 285);

/// Variables removed from the agent's environment, so that an orca-term run from inside a Claude
/// Code session never starts the agent as a child session of it. Everything else passes through:
/// auth and config variables must.
pub(crate) const STRIPPED_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_CHILD_SESSION",
];

/// Flags no launch may carry. `--permission-mode` is allowed only with [`PERMISSION_MODE`].
const FORBIDDEN_FLAGS: &[&str] = &[
    "--dangerously-skip-permissions",
    "--allow-dangerously-skip-permissions",
    "--auto",
];

/// Values `permissions.defaultMode` may never take in the settings.
const FORBIDDEN_MODES: &[&str] = &["bypassPermissions", "auto"];

/// The command line claude is started with: its settings file and the permission mode. Nothing
/// else in this slice; `--resume` joins later.
pub(crate) fn claude_argv(program: &str, settings_file: &Path) -> Vec<String> {
    vec![
        program.to_owned(),
        "--settings".to_owned(),
        settings_file.display().to_string(),
        "--permission-mode".to_owned(),
        PERMISSION_MODE.to_owned(),
    ]
}

/// The settings passed to claude as `--settings <file>`: its sandbox on and set to fail rather
/// than run unsandboxed, the network open, and writes to every worktree's admin directory,
/// `refs/orca-term` and `orca-term.yaml` in the git common dir denied, from sandboxed commands
/// and from the Edit tool alike. `admin_dirs` are the admin directories that exist at launch:
/// Linux ignores the `*` entries, so each one is listed concretely too. `writable` are the extra
/// paths the config allows, absolute.
///
/// Every weakening key is set explicitly, since a scalar in `--settings` overrides the same key
/// in the person's own files. The Edit rules start with `//` because a single `/` would mean a
/// path relative to the settings file.
pub(crate) fn claude_settings(
    common_dir: &Path,
    admin_dirs: &[PathBuf],
    writable: &[PathBuf],
) -> Value {
    let common = common_dir.display();
    let mut deny_write: Vec<String> = admin_dirs.iter().map(|d| d.display().to_string()).collect();
    deny_write.extend([
        format!("{common}/worktrees/*/{ADMIN_DIR_NAME}"),
        format!("{common}/refs/{ADMIN_DIR_NAME}"),
        format!("{common}/orca-term.yaml"),
    ]);
    let allow_write: Vec<String> = writable.iter().map(|p| p.display().to_string()).collect();
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
            "network": {
                "allowedDomains": ["*"],
                "allowAllUnixSockets": false,
            },
        },
        "permissions": {
            "disableBypassPermissionsMode": "disable",
            "disableAutoMode": "disable",
            "deny": [
                format!("Edit(/{common}/worktrees/*/{ADMIN_DIR_NAME}/**)"),
                format!("Edit(/{common}/refs/{ADMIN_DIR_NAME}/**)"),
                format!("Edit(/{common}/orca-term.yaml)"),
            ],
        },
    })
}

/// The settings as the file's bytes, readable by a person.
pub(crate) fn settings_bytes(settings: &Value) -> Result<Vec<u8>> {
    let mut bytes = serde_json::to_vec_pretty(settings)?;
    bytes.push(b'\n');
    Ok(bytes)
}

/// Refuses a launch that bypasses permissions: an argument among [`FORBIDDEN_FLAGS`], a
/// `--permission-mode` other than [`PERMISSION_MODE`] (as two arguments or as `--permission-mode=`),
/// or settings whose `permissions.defaultMode` is one of [`FORBIDDEN_MODES`]. Arguments are
/// compared whole, never as substrings: the settings legitimately name `disableBypassPermissionsMode`.
pub(crate) fn check_launch(argv: &[String], settings: &Value) -> Result<()> {
    let mut rest = argv.iter().skip(1);
    while let Some(arg) = rest.next() {
        if FORBIDDEN_FLAGS.contains(&arg.as_str()) {
            bail!("refusing to launch the agent with `{arg}`, which bypasses permissions");
        }
        let mode = if arg == "--permission-mode" {
            rest.next().map(String::as_str)
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

/// Checks what `claude --version` printed (`2.1.296 (Claude Code)`) against
/// [`MIN_CLAUDE_VERSION`].
pub(crate) fn check_version(output: &str) -> Result<()> {
    let text = output.trim();
    let Some(version) = parse_version(text) else {
        bail!("could not read claude's version from `claude --version` (it printed `{text}`)");
    };
    if version < MIN_CLAUDE_VERSION {
        let (a, b, c) = MIN_CLAUDE_VERSION;
        bail!(
            "claude {}.{}.{} is too old: orca-term needs {a}.{b}.{c} or newer, whose sandbox \
             honours the settings orca-term passes; upgrade claude",
            version.0,
            version.1,
            version.2
        );
    }
    Ok(())
}

/// The leading `major.minor.patch` of `text`, if it starts with one.
fn parse_version(text: &str) -> Option<(u64, u64, u64)> {
    let first = text.split_whitespace().next()?;
    let mut parts = first.split('.').map(|p| p.parse::<u64>().ok());
    Some((parts.next()??, parts.next()??, parts.next()??))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        std::iter::once("claude")
            .chain(args.iter().copied())
            .map(str::to_owned)
            .collect()
    }

    #[test]
    fn the_argv_passes_the_settings_file_and_accept_edits() {
        assert_eq!(
            claude_argv("claude", Path::new("/a/claude-settings.json")),
            [
                "claude",
                "--settings",
                "/a/claude-settings.json",
                "--permission-mode",
                "acceptEdits"
            ]
        );
        let settings = claude_settings(Path::new("/r/.git"), &[], &[]);
        check_launch(
            &claude_argv("claude", Path::new("/a/claude-settings.json")),
            &settings,
        )
        .unwrap();
    }

    #[test]
    fn the_settings_turn_the_sandbox_on_and_deny_our_state() {
        let common = Path::new("/r/.git");
        let admin = [PathBuf::from("/r/.git/worktrees/fix/orca-term")];
        let writable = [PathBuf::from("/home/me/.cache/pnpm")];
        let settings = claude_settings(common, &admin, &writable);
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
                "/r/.git/orca-term.yaml"
            ])
        );
        assert_eq!(at("/sandbox/network/allowedDomains"), json!(["*"]));
        assert_eq!(at("/sandbox/network/allowAllUnixSockets"), json!(false));
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
                "Edit(//r/.git/orca-term.yaml)"
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
    fn a_launch_bypassing_permissions_is_refused_by_whole_argument() {
        let settings = claude_settings(Path::new("/r/.git"), &[], &[]);
        for flag in [
            "--dangerously-skip-permissions",
            "--allow-dangerously-skip-permissions",
            "--auto",
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
    fn settings_defaulting_to_a_bypass_mode_are_refused() {
        let good = claude_settings(Path::new("/r/.git"), &[], &[]);
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
    fn claude_older_than_the_floor_is_refused_with_the_fix() {
        check_version("2.1.296 (Claude Code)\n").unwrap();
        check_version("2.1.285").unwrap();
        let err = check_version("2.1.284 (Claude Code)")
            .unwrap_err()
            .to_string();
        assert!(err.contains("2.1.284 is too old"), "{err}");
        assert!(err.contains("2.1.285 or newer"), "{err}");
        let err = check_version("Claude Code").unwrap_err().to_string();
        assert!(err.contains("could not read claude's version"), "{err}");
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
