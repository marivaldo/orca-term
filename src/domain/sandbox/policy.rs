//! Domain: the sandbox orca-term wraps opencode in (#35, ADR 0007), generated for each launch:
//! `sandbox-exec -f <profile>` on macOS, `bwrap <arguments>` on Linux. Reads and the network stay
//! open; writes are allowed only to the places the policy lists, and denied to orca-term's own
//! state even inside them. claude needs none of this: it sandboxes itself, from its settings.
//!
//! What both platforms keep:
//!
//! - **writable**: the worktree, the git common dir, opencode's own directories, the temporary
//!   directories and the approved extra writable paths;
//! - **denied**: every worktree's admin directory (`<common>/worktrees/*/orca-term`, as claude's
//!   `denyWrite` does), `refs/orca-term`, `orca-term.yaml`, `orca-term-trust.json` and the
//!   `orca-term` directory of the common dir.
//!
//! macOS also denies renaming the directories above the denied ones (the common dir, its
//! `worktrees` and `refs`, each `worktrees/<id>`), so the state cannot be moved out from under its
//! rule; it denies connecting to any unix socket but the system's name resolver, so a command
//! cannot reach a holder's `agent.sock` or a Neovim; with `sandbox.agent_socket` on, it also allows
//! the person's signing agent's socket, that exact path and nothing else (#105). On Linux the denied places are mounted
//! read-only over the writable ones, which also keeps them from being renamed, and opencode runs
//! in a user namespace of its own, which the holders check (`protocol::admit`); it reaches pathname
//! sockets, the signing agent's among them, without any rule.

use std::path::{Path, PathBuf};

use anyhow::{Result, bail};

use crate::domain::state::ADMIN_DIR_NAME;

/// The device files every program writes to on macOS, by name.
const DEVICES: &[&str] = &[
    "/dev/null",
    "/dev/zero",
    "/dev/tty",
    "/dev/ptmx",
    "/dev/dtracehelper",
];

/// The one unix socket a sandboxed program may connect to on macOS: the system's name resolver,
/// without which no host name resolves (measured: `curl` cannot resolve a host without it).
const RESOLVER_SOCKET: &str = "/private/var/run/mDNSResponder";

/// What the sandbox allows and denies, every path absolute with its symlinks resolved, since both
/// sandboxes compare real paths.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Policy {
    /// The places the agent may write: directories (and everything under them) or files.
    pub(crate) writable: Vec<PathBuf>,
    /// The places it may never write, even inside a writable one, that exist at launch.
    pub(crate) denied: Vec<PathBuf>,
    /// The files it may never write that do not exist at launch: on Linux each is covered by an
    /// empty read-only file, which stays behind once the agent ends.
    pub(crate) denied_missing: Vec<PathBuf>,
    /// The git common dir, under which every worktree's admin directory is denied, also the ones
    /// made after the launch (macOS).
    pub(crate) common: PathBuf,
    /// The unix sockets the agent may connect to besides the name resolver's (macOS): the person's
    /// signing agent's, when `sandbox.agent_socket` is on (#105); empty otherwise.
    pub(crate) unix_sockets: Vec<PathBuf>,
}

/// The macOS profile, for `sandbox-exec -f`: everything allowed, then writes denied, then allowed
/// to the writable places and the devices, then denied again to the denied places and the
/// directories above them; a later rule wins. Unix sockets are denied but the resolver's and the
/// policy's `unix_sockets`, each by its exact path (`path-literal`, never a pattern). Refuses
/// a path the profile cannot hold: one with a double quote, a backslash or a control character
/// (measured: `sandbox-exec` reads no escaped quote in a string).
pub(crate) fn seatbelt_profile(policy: &Policy) -> Result<String> {
    for path in policy
        .writable
        .iter()
        .chain(&policy.denied)
        .chain(&policy.denied_missing)
        .chain(&policy.unix_sockets)
        .chain(std::iter::once(&policy.common))
    {
        let text = path.display().to_string();
        if text.contains(['"', '\\']) || text.chars().any(char::is_control) {
            bail!(
                "cannot sandbox opencode: the path {text:?} holds a character the macOS sandbox \
                 profile cannot express (a double quote, a backslash or a control character)"
            );
        }
    }
    Ok(profile_text(policy))
}

/// The profile's text, every path in it checked by [`seatbelt_profile`].
fn profile_text(policy: &Policy) -> String {
    let mut allowed: Vec<String> = DEVICES
        .iter()
        .map(|device| format!("(literal {})", string(device)))
        .collect();
    allowed.push(r#"(regex #"^/dev/ttys[0-9]+$")"#.to_owned());
    allowed.push(format!("(subpath {})", string("/dev/fd")));
    allowed.extend(policy.writable.iter().map(|path| subpath(path)));

    let common = policy.common.display().to_string();
    let worktrees = format!("{common}/worktrees");
    let mut denied: Vec<String> = policy
        .denied
        .iter()
        .chain(&policy.denied_missing)
        .map(|path| subpath(path))
        .collect();
    denied.push(format!(
        r#"(regex #"^{}/[^/]+/{}(/|$)")"#,
        regex(&worktrees),
        regex(ADMIN_DIR_NAME)
    ));
    // The directories above the denied ones, so nothing renames the state out of its rules.
    denied.push(format!("(literal {})", string(&common)));
    denied.push(format!("(literal {})", string(&worktrees)));
    denied.push(format!(r#"(regex #"^{}/[^/]+$")"#, regex(&worktrees)));
    denied.push(format!("(literal {})", string(&format!("{common}/refs"))));

    let mut sockets = vec![RESOLVER_SOCKET.to_owned()];
    sockets.extend(
        policy
            .unix_sockets
            .iter()
            .map(|path| path.display().to_string()),
    );
    let socket_rules: Vec<String> = sockets
        .iter()
        .map(|socket| {
            format!(
                "(allow network-outbound (remote unix-socket (path-literal {})))",
                string(socket)
            )
        })
        .collect();

    format!(
        "(version 1)\n\
         (allow default)\n\
         (deny file-write*)\n\
         (allow file-write*\n    {})\n\
         (deny file-write*\n    {})\n\
         (deny network-outbound (remote unix-socket))\n\
         {}\n",
        allowed.join("\n    "),
        denied.join("\n    "),
        socket_rules.join("\n"),
    )
}

/// The Linux arguments of `bwrap`, before `--` and the agent's command line: the whole file system
/// read-only, the devices as they are, the writable places bound read-write (those missing at
/// launch skipped), then the denied places bound read-only over them, each missing denied file
/// covered by an empty one, and a user namespace of its own. The network is kept: opencode reaches
/// its provider's API from its own process.
pub(crate) fn bwrap_args(policy: &Policy) -> Vec<String> {
    let mut args: Vec<String> = [
        "--die-with-parent",
        "--unshare-user",
        "--ro-bind",
        "/",
        "/",
        "--dev-bind",
        "/dev",
        "/dev",
    ]
    .iter()
    .map(|arg| (*arg).to_owned())
    .collect();
    for path in &policy.writable {
        push_bind(&mut args, "--bind-try", path, path);
    }
    for path in &policy.denied {
        push_bind(&mut args, "--ro-bind", path, path);
    }
    for path in &policy.denied_missing {
        push_bind(&mut args, "--ro-bind", Path::new("/dev/null"), path);
    }
    args
}

fn push_bind(args: &mut Vec<String>, flag: &str, source: &Path, destination: &Path) {
    args.push(flag.to_owned());
    args.push(source.display().to_string());
    args.push(destination.display().to_string());
}

/// The rule that matches `path` and everything under it.
fn subpath(path: &Path) -> String {
    format!("(subpath {})", string(&path.display().to_string()))
}

/// `text`, which holds no double quote, as a profile's string.
fn string(text: &str) -> String {
    format!("\"{text}\"")
}

/// `text`, which holds no double quote nor backslash, matched literally inside a profile's regular
/// expression: every character that means something there is escaped.
fn regex(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if ".^$*+?()[]{}|".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> Policy {
        Policy {
            writable: vec![
                PathBuf::from("/w/repo-fix"),
                PathBuf::from("/r/.git"),
                PathBuf::from("/h/.local/share/opencode"),
            ],
            denied: vec![
                PathBuf::from("/r/.git/worktrees/fix/orca-term"),
                PathBuf::from("/r/.git/refs/orca-term"),
                PathBuf::from("/r/.git/orca-term-trust.json"),
                PathBuf::from("/r/.git/orca-term"),
            ],
            denied_missing: vec![PathBuf::from("/r/.git/orca-term.yaml")],
            common: PathBuf::from("/r/.git"),
            unix_sockets: Vec::new(),
        }
    }

    #[test]
    fn the_profile_allows_the_writable_places_then_denies_our_state() {
        assert_eq!(
            seatbelt_profile(&policy()).unwrap(),
            r#"(version 1)
(allow default)
(deny file-write*)
(allow file-write*
    (literal "/dev/null")
    (literal "/dev/zero")
    (literal "/dev/tty")
    (literal "/dev/ptmx")
    (literal "/dev/dtracehelper")
    (regex #"^/dev/ttys[0-9]+$")
    (subpath "/dev/fd")
    (subpath "/w/repo-fix")
    (subpath "/r/.git")
    (subpath "/h/.local/share/opencode"))
(deny file-write*
    (subpath "/r/.git/worktrees/fix/orca-term")
    (subpath "/r/.git/refs/orca-term")
    (subpath "/r/.git/orca-term-trust.json")
    (subpath "/r/.git/orca-term")
    (subpath "/r/.git/orca-term.yaml")
    (regex #"^/r/\.git/worktrees/[^/]+/orca-term(/|$)")
    (literal "/r/.git")
    (literal "/r/.git/worktrees")
    (regex #"^/r/\.git/worktrees/[^/]+$")
    (literal "/r/.git/refs"))
(deny network-outbound (remote unix-socket))
(allow network-outbound (remote unix-socket (path-literal "/private/var/run/mDNSResponder")))
"#
        );
    }

    #[test]
    fn the_signing_agents_socket_is_allowed_by_its_exact_path_only_when_given() {
        let without = seatbelt_profile(&policy()).unwrap();
        assert_eq!(
            without.matches("remote unix-socket (path-literal").count(),
            1,
            "only the resolver's: {without}"
        );
        let mut with = policy();
        with.unix_sockets = vec![PathBuf::from("/private/var/folders/x/T/agent.sock")];
        let profile = seatbelt_profile(&with).unwrap();
        assert!(
            profile.ends_with(
                "(deny network-outbound (remote unix-socket))\n\
                 (allow network-outbound (remote unix-socket (path-literal \
                 \"/private/var/run/mDNSResponder\")))\n\
                 (allow network-outbound (remote unix-socket (path-literal \
                 \"/private/var/folders/x/T/agent.sock\")))\n"
            ),
            "{profile}"
        );
        assert_eq!(
            profile.replace(
                "(allow network-outbound (remote unix-socket (path-literal \
                 \"/private/var/folders/x/T/agent.sock\")))\n",
                ""
            ),
            without,
            "nothing else changes"
        );
        assert!(!profile.contains("subpath \"/private/var/folders"));
        assert!(!profile.contains("(remote unix-socket (regex"));
        assert_eq!(
            bwrap_args(&with),
            bwrap_args(&policy()),
            "Linux needs no rule"
        );
    }

    #[test]
    fn a_socket_path_the_profile_cannot_hold_is_refused() {
        let mut odd = policy();
        odd.unix_sockets = vec![PathBuf::from(r#"/t/a"b.sock"#)];
        let err = seatbelt_profile(&odd).unwrap_err().to_string();
        assert!(err.contains("cannot sandbox opencode"), "{err}");
    }

    #[test]
    fn paths_are_quoted_and_escaped_in_the_profile() {
        assert_eq!(string("/a b"), r#""/a b""#);
        assert_eq!(regex("/r/a.b+(c)/[d] e"), r"/r/a\.b\+\(c\)/\[d\] e");
    }

    #[test]
    fn a_path_the_profile_cannot_hold_is_refused() {
        for bad in [r#"/w/a"b"#, r"/w/a\b", "/w/a\nb"] {
            let mut odd = policy();
            odd.writable.push(PathBuf::from(bad));
            let err = seatbelt_profile(&odd).unwrap_err().to_string();
            assert!(err.contains("cannot sandbox opencode"), "{err}");
        }
    }

    #[test]
    fn bwrap_binds_the_writable_places_then_our_state_read_only_over_them() {
        let args = bwrap_args(&policy());
        let expected: Vec<String> = [
            "--die-with-parent",
            "--unshare-user",
            "--ro-bind",
            "/",
            "/",
            "--dev-bind",
            "/dev",
            "/dev",
            "--bind-try",
            "/w/repo-fix",
            "/w/repo-fix",
            "--bind-try",
            "/r/.git",
            "/r/.git",
            "--bind-try",
            "/h/.local/share/opencode",
            "/h/.local/share/opencode",
            "--ro-bind",
            "/r/.git/worktrees/fix/orca-term",
            "/r/.git/worktrees/fix/orca-term",
            "--ro-bind",
            "/r/.git/refs/orca-term",
            "/r/.git/refs/orca-term",
            "--ro-bind",
            "/r/.git/orca-term-trust.json",
            "/r/.git/orca-term-trust.json",
            "--ro-bind",
            "/r/.git/orca-term",
            "/r/.git/orca-term",
            "--ro-bind",
            "/dev/null",
            "/r/.git/orca-term.yaml",
        ]
        .iter()
        .map(|arg| (*arg).to_owned())
        .collect();
        assert_eq!(args, expected);
        assert!(
            !args.iter().any(|arg| arg.starts_with("--unshare-net")),
            "the network is kept"
        );
    }
}
