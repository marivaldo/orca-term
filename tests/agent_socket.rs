//! `sandbox.agent_socket` (#105): with it on in `.git/orca-term.yaml` or the global config file,
//! `agent start` lets the agent reach the person's signing agent, and only that: claude's settings
//! carry `allowUnixSockets` with the socket's real path, opencode's macOS profile one
//! `path-literal` rule for it. Driven as a black box against the fake `claude`, the fake
//! `opencode` and fake sandbox tools on an isolated PATH, with a socket of the test's own as
//! `SSH_AUTH_SOCK` (passed explicitly, never the developer's) and a fake `gpgconf`. A committed
//! `orca-term.yaml` that sets it starts nothing. The real sandbox reaching a real throwaway
//! ssh-agent is `tests/opencode_sandbox.rs`.

#![expect(
    clippy::unwrap_used,
    reason = "clippy.toml relaxes unwrap only inside #[test] fns; fixture helpers panic on failure too"
)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "a black-box test builds real repositories and files around the binary it drives"
)]

#[path = "support/agent.rs"]
#[expect(
    dead_code,
    reason = "the agent fixture is shared between suites, and this one leaves part of it unused"
)]
mod fixture;
#[path = "support/opencode.rs"]
#[expect(
    dead_code,
    reason = "the opencode fixture is shared between suites, and this one leaves part of it unused"
)]
mod opencode;
#[expect(
    dead_code,
    reason = "the fixture is shared between suites, and this one leaves part of it unused"
)]
mod support;

use std::fs;
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Output;

use fixture::{AgentRepo, holder_alive};
use opencode::OpenCodeRepo;
use serde_json::{Value, json};

/// The setting on, as `.git/orca-term.yaml` or `orca-term.yaml` writes it.
const ON: &str = "sandbox:\n  agent_socket: true\n";

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

/// A unix socket listening at `<dir>/<name>`, and its real path. The listener must outlive the
/// test's starts, or the path would be no socket.
fn listening(dir: &Path, name: &str) -> (UnixListener, PathBuf) {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    let listener = UnixListener::bind(&path).unwrap();
    (listener, path.canonicalize().unwrap())
}

/// The short directory the test's sockets go in: inside the fixture, in the system's temporary
/// directory, since a socket's path must fit in about 100 bytes.
fn socket_dir(fx: &AgentRepo) -> PathBuf {
    fx.repo.tmp.path().join("s")
}

/// Starts claude in the worktree `name` with `SSH_AUTH_SOCK` set to `socket`, or unset when
/// `None`, and returns the `network` section of the settings it was given.
fn start_claude(fx: &AgentRepo, name: &str, socket: Option<&Path>) -> Value {
    let worktree = fx.new_worktree(name);
    let mut command = fx.command(&["agent", "start", name]);
    match socket {
        Some(socket) => command.env("SSH_AUTH_SOCK", socket),
        None => command.env_remove("SSH_AUTH_SOCK"),
    };
    let out = command.output().unwrap();
    assert!(out.status.success(), "{}", stderr(&out));
    fx.wait_for_output(name, "fake claude ready");
    let settings = fx.admin_dir(&worktree).join("claude-settings.json");
    let settings: Value = serde_json::from_slice(&fs::read(settings).unwrap()).unwrap();
    settings["sandbox"]["network"].clone()
}

#[test]
fn with_the_setting_claudes_settings_allow_exactly_the_ssh_agents_socket() {
    let fx = AgentRepo::new();
    fx.repo.write(".git/orca-term.yaml", ON);
    let (_listener, socket) = listening(&socket_dir(&fx), "agent.sock");

    let network = start_claude(&fx, "fix", Some(&socket));

    assert_eq!(
        network,
        json!({
            "allowedDomains": ["*"],
            "allowAllUnixSockets": false,
            "allowUnixSockets": [socket.display().to_string()],
        })
    );
}

#[test]
fn the_socket_is_allowed_by_its_real_path_and_the_global_file_turns_it_on_too() {
    let fx = AgentRepo::new();
    let config = fx.repo.xdg.join("orca-term/config.yaml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, ON).unwrap();
    let dir = socket_dir(&fx);
    let (_listener, socket) = listening(&dir, "agent.sock");
    let link = dir.join("link.sock");
    std::os::unix::fs::symlink(&socket, &link).unwrap();

    let network = start_claude(&fx, "fix", Some(&link));

    assert_eq!(
        network["allowUnixSockets"],
        json!([socket.display().to_string()]),
        "the link resolved"
    );
}

#[test]
fn without_the_setting_or_without_a_socket_nothing_changes() {
    let fx = AgentRepo::new();
    let dir = socket_dir(&fx);
    let (_listener, socket) = listening(&dir, "agent.sock");
    let plain = json!({"allowedDomains": ["*"], "allowAllUnixSockets": false});

    assert_eq!(start_claude(&fx, "off", Some(&socket)), plain, "off");

    fx.repo.write(".git/orca-term.yaml", ON);
    let file = dir.join("not-a-socket");
    fs::write(&file, "").unwrap();
    assert_eq!(start_claude(&fx, "file", Some(&file)), plain, "no socket");
    assert_eq!(start_claude(&fx, "unset", None), plain, "unset");
    let gone = dir.join("gone.sock");
    assert_eq!(start_claude(&fx, "gone", Some(&gone)), plain, "missing");
}

#[test]
fn a_committed_agent_socket_starts_nothing_and_cannot_be_approved() {
    let fx = AgentRepo::new();
    fx.repo.write("orca-term.yaml", ON);
    fx.repo.commit_all("hand over the agent");
    let worktree = fx.new_worktree("fix");
    let (_listener, socket) = listening(&socket_dir(&fx), "agent.sock");

    for args in [
        &["agent", "start", "fix"][..],
        &["agent", "start", "fix", "--approve-sandbox"],
    ] {
        let out = fx
            .command(args)
            .env("SSH_AUTH_SOCK", &socket)
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(1), "{args:?}");
        assert_eq!(
            stderr(&out),
            format!(
                "orca-term: {} sets `sandbox.agent_socket`, which orca-term reads only from your \
                 global config file or .git/orca-term.yaml, never from a committed \
                 orca-term.yaml: it would hand your signing agent's keys to whoever wrote the \
                 repository; remove it from that file\n",
                worktree.join("orca-term.yaml").display()
            )
        );
    }
    let out = fx
        .command(&["agent", "start", "fix", "--json"])
        .env("SSH_AUTH_SOCK", &socket)
        .output()
        .unwrap();
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(
        doc["error"]["kind"], "other",
        "not `untrusted`: nothing to approve"
    );
    assert_eq!(fx.starts(), 0, "nothing was launched");
    assert!(!holder_alive(&fx.admin_dir(&worktree)));
    assert!(!fx.repo.root.join(".git/orca-term-trust.json").exists());
}

#[test]
fn gpg_agents_socket_is_allowed_only_when_git_signs_with_openpgp() {
    let fx = AgentRepo::new();
    fx.repo.write(".git/orca-term.yaml", ON);
    let (_listener, gpg) = listening(&socket_dir(&fx), "S.gpg-agent");
    fx.write_tool(
        "gpgconf",
        &format!(
            "#!/bin/sh\n[ \"$1 $2\" = \"--list-dirs agent-socket\" ] && echo '{}'\n",
            gpg.display()
        ),
    );

    let network = start_claude(&fx, "openpgp", None);
    assert_eq!(
        network["allowUnixSockets"],
        json!([gpg.display().to_string()])
    );

    fx.repo.git(&["config", "gpg.format", "ssh"]);
    let network = start_claude(&fx, "ssh", None);
    assert_eq!(network.get("allowUnixSockets"), None);
}

#[test]
fn opencodes_profile_allows_the_socket_by_one_exact_rule() {
    let oc = OpenCodeRepo::new();
    let (_listener, socket) = listening(&socket_dir(&oc.fx), "agent.sock");
    let rule = format!(
        "(allow network-outbound (remote unix-socket (path-literal \"{}\")))",
        socket.display()
    );
    let start = |name: &str| {
        let worktree = oc.fx.new_worktree(name);
        let out = oc
            .command(&["agent", "start", name, "--agent", "opencode"])
            .env("SSH_AUTH_SOCK", &socket)
            .output()
            .unwrap();
        assert!(out.status.success(), "{}", stderr(&out));
        oc.fx.wait_for_output(name, "fake opencode ready");
        (
            oc.fx.admin_dir(&worktree).join("opencode-sandbox.sb"),
            oc.sandbox_argv(),
        )
    };

    let (profile_off, argv_off) = start("off");
    oc.fx.repo.write(".git/orca-term.yaml", ON);
    let (profile_on, argv_on) = start("on");

    if cfg!(target_os = "macos") {
        let off = fs::read_to_string(profile_off).unwrap();
        let on = fs::read_to_string(profile_on).unwrap();
        assert!(!off.contains(&rule), "{off}");
        assert_eq!(on.matches(&rule).count(), 1, "{on}");
        assert!(
            on.ends_with(&format!(
                "(deny network-outbound (remote unix-socket))\n(allow network-outbound (remote \
                 unix-socket (path-literal \"/private/var/run/mDNSResponder\")))\n{rule}\n"
            )),
            "{on}"
        );
        let socket_rules = on.matches("(remote unix-socket (path-literal").count();
        assert_eq!(socket_rules, 2, "the resolver's and the agent's: {on}");
    } else {
        // bwrap reaches pathname sockets already: no argument names the socket.
        let socket = socket.display().to_string();
        assert!(!argv_off.iter().any(|arg| arg.contains(&socket)));
        assert!(!argv_on.iter().any(|arg| arg.contains(&socket)));
    }
}
