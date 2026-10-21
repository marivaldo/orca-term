//! Domain: how opencode is launched in a worktree (#35, ADR 0007, ADR 0009): `opencode [-s <id>]`
//! in the worktree, on its holder's PTY, inside the sandbox orca-term wraps it in
//! (`domain/sandbox/policy.rs`): `sandbox-exec -f <profile>` on macOS, `bwrap <arguments> --` on
//! Linux. Its turns are reported by a plugin orca-term ships, embedded in the binary and written
//! before each launch into a config directory of the worktree's own, inside its admin directory,
//! which `OPENCODE_CONFIG_DIR` names (ADR 0008: never in opencode's global config).
//!
//! The plugin runs inside the sandbox, which denies writes to the admin directory, so it cannot
//! run `orca-term hook` itself: it writes each event as one line to a pipe the holder hands it as
//! a file descriptor, named by [`EVENTS_FD_VAR`](crate::domain::hook::EVENTS_FD_VAR), and the
//! holder runs the hook outside the sandbox.
//!
//! opencode writes to its own directories, found the way it finds them (the `xdg-basedir` package
//! and Node's `os.tmpdir()`), which the sandbox therefore allows: `$XDG_DATA_HOME/opencode`
//! (`~/.local/share/opencode`), the same under `$XDG_CACHE_HOME` (`~/.cache`),
//! `$XDG_CONFIG_HOME` (`~/.config`) and `$XDG_STATE_HOME` (`~/.local/state`), and `~/.opencode`,
//! another directory it reads its config from and writes a `.gitignore` into.

use std::path::{Path, PathBuf};

use crate::domain::launch::SessionId;

/// The variable that names opencode's extra config directory, the one holding our plugin.
pub(crate) const CONFIG_DIR_VAR: &str = "OPENCODE_CONFIG_DIR";

/// The plugin, a plain JavaScript file importing only Node's built-in `fs`.
pub(crate) const PLUGIN: &str = include_str!("opencode/orca-term.js");

/// Where the plugin goes in the config directory: opencode loads every `plugins/*.js` there.
pub(crate) const PLUGIN_FILE: &str = "plugins/orca-term.js";

/// The `.gitignore` opencode writes into every config directory that has none, written by us
/// first so that opencode, which cannot write there inside the sandbox, has nothing to write.
pub(crate) const GITIGNORE: &str =
    "node_modules\npackage.json\npackage-lock.json\nbun.lock\n.gitignore";

/// The `.gitignore` file of a config directory.
pub(crate) const GITIGNORE_FILE: &str = ".gitignore";

/// The flag that opens opencode on a recorded session.
const SESSION_FLAG: &str = "-s";

/// The name opencode gives its directories under each base directory.
const APP: &str = "opencode";

/// opencode's command line: the sandbox's `wrapper` (its program and arguments, everything before
/// opencode's own), then `program` (opencode, by its absolute path), then `-s <id>` when `resume`
/// names a session.
pub(crate) fn opencode_argv(
    wrapper: Vec<String>,
    program: &Path,
    resume: Option<&SessionId>,
) -> Vec<String> {
    let mut argv = wrapper;
    argv.push(program.display().to_string());
    if let Some(session) = resume {
        argv.push(SESSION_FLAG.to_owned());
        argv.push(session.as_str().to_owned());
    }
    argv
}

/// The variables opencode finds its directories by, as they are set, if at all.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct DirsEnv {
    pub(crate) home: Option<String>,
    pub(crate) xdg_data_home: Option<String>,
    pub(crate) xdg_cache_home: Option<String>,
    pub(crate) xdg_config_home: Option<String>,
    pub(crate) xdg_state_home: Option<String>,
    pub(crate) tmpdir: Option<String>,
}

/// opencode's own directories, the ones it creates: `<base>/opencode` for its data, cache, config
/// and state bases, each base its XDG variable when set to an absolute path, else under `HOME`.
/// None without a usable `HOME` and variable.
pub(crate) fn own_dirs(env: &DirsEnv) -> Vec<PathBuf> {
    let home = absolute(env.home.as_deref());
    // A base directory: its variable when set to an absolute path, else `under_home` in HOME.
    let base = |variable: &Option<String>, under_home: &str| match absolute(variable.as_deref()) {
        Some(set) => Some(set.to_path_buf()),
        None => home.map(|home| home.join(under_home)),
    };
    [
        base(&env.xdg_data_home, ".local/share"),
        base(&env.xdg_cache_home, ".cache"),
        base(&env.xdg_config_home, ".config"),
        base(&env.xdg_state_home, ".local/state"),
    ]
    .into_iter()
    .flatten()
    .map(|base| base.join(APP))
    .collect()
}

/// `~/.opencode`, the directory opencode reads more config from when it exists; it is never
/// created for it.
pub(crate) fn home_config_dir(env: &DirsEnv) -> Option<PathBuf> {
    absolute(env.home.as_deref()).map(|home| home.join(".opencode"))
}

/// The temporary directories programs write to: `$TMPDIR` when set to an absolute path (Node's
/// `os.tmpdir()`), and `/tmp`, which many programs use whatever `$TMPDIR` says.
pub(crate) fn temp_dirs(env: &DirsEnv) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = absolute(env.tmpdir.as_deref())
        .map(Path::to_path_buf)
        .into_iter()
        .collect();
    dirs.push(PathBuf::from("/tmp"));
    dirs
}

/// The arguments that ask opencode for its sessions without spending anything (#41): `session list`
/// reads its own database, as JSON, with no plugin loaded (`--pure`). Measured with opencode
/// 1.18.0: a JSON array of `{"id", "title", "updated", "created", "projectId", "directory"}`, one per
/// session of the project the directory it runs in belongs to.
pub(crate) const SESSION_LIST_ARGS: &[&str] = &["session", "list", "--format", "json", "--pure"];

/// Whether `listed`, what `opencode session list --format json` printed, holds the session `id`.
/// `None` when it is not such a list: then nobody can tell, and opencode says it itself when it
/// cannot open the session.
pub(crate) fn lists_session(listed: &[u8], id: &str) -> Option<bool> {
    let sessions: Vec<serde_json::Value> = serde_json::from_slice(listed).ok()?;
    Some(
        sessions
            .iter()
            .any(|session| session.get("id").and_then(serde_json::Value::as_str) == Some(id)),
    )
}

/// `value` as a path when it is set to an absolute one.
fn absolute(value: Option<&str>) -> Option<&Path> {
    value.map(Path::new).filter(|path| path.is_absolute())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::hook::EVENTS_FD_VAR;

    #[test]
    fn opencode_runs_inside_its_wrapper_on_a_recorded_session_only_when_given() {
        let wrapper = vec![
            "/usr/bin/sandbox-exec".to_owned(),
            "-f".to_owned(),
            "/a/opencode-sandbox.sb".to_owned(),
        ];
        let program = Path::new("/opt/bin/opencode");
        assert_eq!(
            opencode_argv(wrapper.clone(), program, None),
            [
                "/usr/bin/sandbox-exec",
                "-f",
                "/a/opencode-sandbox.sb",
                "/opt/bin/opencode"
            ]
        );
        let session = SessionId::parse("ses_2Ab9").unwrap();
        let argv = opencode_argv(wrapper, program, Some(&session));
        assert_eq!(argv[3..], ["/opt/bin/opencode", "-s", "ses_2Ab9"]);
    }

    #[test]
    fn a_session_is_known_only_when_opencodes_own_list_holds_it() {
        let listed = br#"[{"id":"ses_1","title":"fix","updated":2,"created":1},{"id":"ses_2"}]"#;
        assert_eq!(lists_session(listed, "ses_2"), Some(true));
        assert_eq!(lists_session(listed, "ses_9"), Some(false));
        assert_eq!(lists_session(b"[]", "ses_1"), Some(false));
        assert_eq!(lists_session(b"fake opencode ready", "ses_1"), None);
        assert_eq!(lists_session(b"", "ses_1"), None, "nobody can tell");
    }

    #[test]
    fn opencodes_directories_follow_xdg_else_home() {
        let env = DirsEnv {
            home: Some("/h".to_owned()),
            ..DirsEnv::default()
        };
        assert_eq!(
            own_dirs(&env),
            [
                PathBuf::from("/h/.local/share/opencode"),
                PathBuf::from("/h/.cache/opencode"),
                PathBuf::from("/h/.config/opencode"),
                PathBuf::from("/h/.local/state/opencode"),
            ]
        );
        let xdg = DirsEnv {
            home: Some("/h".to_owned()),
            xdg_data_home: Some("/d".to_owned()),
            xdg_cache_home: Some("relative".to_owned()),
            xdg_config_home: Some("/c".to_owned()),
            xdg_state_home: Some("/s".to_owned()),
            tmpdir: None,
        };
        assert_eq!(
            own_dirs(&xdg),
            [
                PathBuf::from("/d/opencode"),
                PathBuf::from("/h/.cache/opencode"),
                PathBuf::from("/c/opencode"),
                PathBuf::from("/s/opencode"),
            ],
            "a relative variable is not used"
        );
        assert!(own_dirs(&DirsEnv::default()).is_empty());
        assert_eq!(home_config_dir(&env), Some(PathBuf::from("/h/.opencode")));
    }

    #[test]
    fn the_temporary_directories_are_tmpdir_and_tmp() {
        let env = DirsEnv {
            tmpdir: Some("/var/folders/x/T/".to_owned()),
            ..DirsEnv::default()
        };
        assert_eq!(
            temp_dirs(&env),
            [PathBuf::from("/var/folders/x/T/"), PathBuf::from("/tmp")]
        );
        assert_eq!(temp_dirs(&DirsEnv::default()), [PathBuf::from("/tmp")]);
    }

    #[test]
    fn the_plugin_imports_nothing_but_nodes_fs_and_exports_one_plugin() {
        let imports: Vec<&str> = PLUGIN
            .lines()
            .filter(|line| line.trim_start().starts_with("import "))
            .collect();
        assert_eq!(imports, [r#"import { writeSync } from "node:fs""#]);
        assert!(!PLUGIN.contains("require("), "no other way in either");
        let exports: Vec<&str> = PLUGIN
            .lines()
            .filter(|line| line.starts_with("export "))
            .collect();
        assert_eq!(
            exports,
            ["export const OrcaTerm = async () => {"],
            "opencode calls every export as a plugin"
        );
        assert!(PLUGIN.contains(&format!("process.env.{EVENTS_FD_VAR}")));
    }
}
