//! opencode's sandbox for real (#35, ADR 0007): the fake `opencode` is launched by `agent start`
//! through the real `sandbox-exec` (macOS) or `bwrap` (Linux), with the profile or arguments the
//! core generated, and before it says it is ready it tries to write in the worktree (allowed), in
//! the git common dir (allowed), in its own directories and the temporary directory (allowed),
//! outside (denied), in its own and another worktree's admin directory (denied), in the approvals'
//! file (denied), in `HOME` (denied), and to attach to another worktree's running agent (refused:
//! macOS denies the connection, a Linux holder closes it without a replay). Where the sandbox tool
//! is absent, or cannot make the namespaces it needs, the test says why and skips.
//!
//! On macOS, with `sandbox.agent_socket` on (#105), the fake opencode reaches a throwaway
//! ssh-agent through the socket `SSH_AUTH_SOCK` names, signs a commit with a key only that agent
//! holds, and is still refused another worktree's agent; without the setting it reaches neither.

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
use std::path::{Path, PathBuf};
use std::process::Command;

use fixture::wait_until;
use opencode::OpenCodeRepo;

/// What the fake opencode does, inside the sandbox, before it says it is ready: each write it
/// tries, one line each (`<what> written` or `<what> denied`), then the attach to the other
/// worktree's agent, kept open three seconds so a replay would have time to come, into files of
/// the worktree.
const PROBES: &str = r#"
probe() { if ( echo probe > "$2" ) 2>/dev/null; then echo "$1 written"; else echo "$1 denied"; fi; }
{
  probe worktree "$PWD/probe-inside"
  probe common "$ORCA_TERM_TEST_COMMON/probe-common"
  probe own-dir "$HOME/.local/share/opencode/probe-own"
  probe temp "/tmp/orca-term-probe-$$" && /bin/rm -f "/tmp/orca-term-probe-$$"
  probe outside "$ORCA_TERM_TEST_OUTSIDE/probe-outside"
  probe home "$HOME/probe-home"
  probe admin "$ORCA_TERM_TEST_ADMIN/probe-admin"
  probe other-admin "$ORCA_TERM_TEST_OTHER_ADMIN/probe-other-admin"
  probe trust "$ORCA_TERM_TEST_COMMON/orca-term-trust.json"
  /bin/sleep 3 | "$ORCA_TERM_TEST_BINARY" agent attach other > "$PWD/probe-attach" 2>&1
  echo "attach exit $?"
} > "$PWD/probe-results"
"#;

/// The real sandbox tool of this system, or why the test cannot use it.
fn real_tool() -> Result<PathBuf, String> {
    let name = if cfg!(target_os = "macos") {
        "sandbox-exec"
    } else {
        "bwrap"
    };
    let out = Command::new("sh")
        .args(["-c", &format!("command -v {name}")])
        .output()
        .unwrap();
    let path = String::from_utf8(out.stdout).unwrap();
    if path.trim().is_empty() {
        return Err(format!("{name} is not installed"));
    }
    let path = PathBuf::from(path.trim());
    if cfg!(target_os = "linux") {
        let made = Command::new(&path)
            .args(["--unshare-user", "--ro-bind", "/", "/", "true"])
            .output()
            .unwrap();
        if !made.status.success() {
            return Err(format!(
                "bwrap cannot make a user namespace here: {}",
                String::from_utf8_lossy(&made.stderr).trim()
            ));
        }
    }
    Ok(path)
}

/// The places the probes write to, and the worktree they run in.
struct Probed {
    demo: PathBuf,
    admin: PathBuf,
    other_admin: PathBuf,
    common: PathBuf,
    outside: PathBuf,
}

impl Probed {
    /// Starts the fake opencode, running the probes, in the new worktree `demo` through the real
    /// sandbox, the worktree `other` running the fake claude, and returns once the probes ended.
    fn run(oc: &OpenCodeRepo) -> Self {
        let other = oc.fx.new_worktree("other");
        let other_admin = oc.fx.admin_dir(&other);
        oc.ok(&["agent", "start", "other"]);
        oc.fx.wait_for_output("other", "fake claude ready");
        let demo = oc.fx.new_worktree("demo");
        let probed = Self {
            admin: oc.fx.admin_dir(&demo),
            demo,
            other_admin,
            common: oc.fx.repo.root.join(".git"),
            outside: oc.fx.repo.tmp.path().join("outside"),
        };
        fs::create_dir_all(&probed.outside).unwrap();
        let out = oc
            .command(&["agent", "start", "demo", "--agent", "opencode"])
            .env("ORCA_TERM_TEST_COMMON", &probed.common)
            .env("ORCA_TERM_TEST_OUTSIDE", &probed.outside)
            .env("ORCA_TERM_TEST_ADMIN", &probed.admin)
            .env("ORCA_TERM_TEST_OTHER_ADMIN", &probed.other_admin)
            .env("ORCA_TERM_TEST_BINARY", env!("CARGO_BIN_EXE_orca-term"))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        let results = probed.demo.join("probe-results");
        wait_until("the probes to finish", || {
            fs::read_to_string(&results).is_ok_and(|text| text.contains("attach exit"))
        });
        probed
    }
}

/// The fixture, in `target/tmp`: not in the system's temporary directory, which the sandbox lets
/// opencode write, so its "outside" and its `HOME` are outside everything opencode may write. The
/// real sandbox tool `tool` replaces the fake one, and the fake opencode runs the probes.
fn fixture_with(tool: &Path) -> OpenCodeRepo {
    let oc = OpenCodeRepo::new_in(Path::new(env!("CARGO_TARGET_TMPDIR")));
    let name = tool.file_name().unwrap().to_str().unwrap().to_owned();
    fs::remove_file(oc.fx.bin.join(&name)).unwrap();
    std::os::unix::fs::symlink(tool, oc.fx.bin.join(&name)).unwrap();
    oc.write_opencode("1.18.0", PROBES);
    oc
}

#[test]
fn opencode_writes_only_where_its_sandbox_allows_and_reaches_no_other_agent() {
    let tool = match real_tool() {
        Ok(tool) => tool,
        Err(why) => {
            eprintln!("skipped: opencode's real sandbox cannot run here: {why}");
            return;
        }
    };
    let oc = fixture_with(&tool);
    let Probed {
        demo,
        admin,
        other_admin,
        common,
        outside,
    } = Probed::run(&oc);
    let results = fs::read_to_string(demo.join("probe-results")).unwrap();
    let lines: Vec<&str> = results.lines().collect();
    assert_eq!(
        lines[..9],
        [
            "worktree written",
            "common written",
            "own-dir written",
            "temp written",
            "outside denied",
            "home denied",
            "admin denied",
            "other-admin denied",
            "trust denied",
        ],
        "{results}"
    );
    assert!(!outside.join("probe-outside").exists());
    assert!(!admin.join("probe-admin").exists());
    assert!(!other_admin.join("probe-other-admin").exists());
    assert_eq!(
        fs::read_to_string(common.join("orca-term-trust.json")).unwrap(),
        r#"{"schema":1,"approved":[]}"#
    );
    let attach = fs::read_to_string(demo.join("probe-attach")).unwrap();
    assert!(
        !attach.contains("fake claude ready"),
        "the other agent's screen reached opencode's sandbox: {attach}"
    );
    if cfg!(target_os = "macos") {
        // The sandbox itself refuses the connection to the socket.
        assert!(
            attach.contains("could not connect") && attach.contains("Operation not permitted"),
            "{attach}"
        );
        assert_eq!(lines.get(9), Some(&"attach exit 1"), "{results}");
    } else {
        // The connection goes through, the network being kept, and the holder closes it from
        // another user namespace before writing anything: the attach sees itself detached.
        assert_eq!(attach, "[detached]\n");
    }
    // The other agent serves a client outside the sandbox all the same.
    oc.fx.wait_for_output("other", "fake claude ready");
}

/// What the fake opencode does, inside the sandbox, with the throwaway ssh-agent (#105): asks it
/// for its keys (`ssh-add -l`, exit 0 when it answers, 2 when it cannot be reached), commits a
/// file signed with the agent-only key, then attaches to the other worktree's agent as
/// [`PROBES`] does.
const SIGNING_PROBES: &str = r#"
{
  /usr/bin/ssh-add -l > /dev/null 2>&1
  echo "agent exit $?"
  echo signed > "$PWD/signed.txt"
  if git add signed.txt && git commit -q -S -m signed > /dev/null 2> "$PWD/sign-err"; then
    echo "commit signed"
  else
    echo "commit failed"
  fi
  /bin/sleep 3 | "$ORCA_TERM_TEST_BINARY" agent attach other > "$PWD/probe-attach" 2>&1
  echo "attach exit $?"
} > "$PWD/probe-results"
"#;

/// A throwaway `ssh-agent` on a socket of its own, holding a throwaway key whose private file is
/// deleted once added: only the agent can sign with it. Never the developer's agent nor key.
struct ThrowawayAgent {
    child: std::process::Child,
    /// Where the socket and the public key are.
    _dir: assert_fs::TempDir,
    socket: PathBuf,
    public_key: PathBuf,
}

impl ThrowawayAgent {
    /// Starts it, or says why it cannot (no ssh tools here).
    fn start() -> Result<Self, String> {
        for tool in [
            "/usr/bin/ssh-agent",
            "/usr/bin/ssh-add",
            "/usr/bin/ssh-keygen",
        ] {
            if !Path::new(tool).exists() {
                return Err(format!("{tool} is not installed"));
            }
        }
        // In the system's temporary directory: a socket's path must fit in about 100 bytes.
        let dir = assert_fs::TempDir::new().unwrap();
        let socket = dir.path().join("a.sock");
        let key = dir.path().join("agentonly");
        let made = Command::new("/usr/bin/ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", "throwaway", "-f"])
            .arg(&key)
            .output()
            .unwrap();
        assert!(made.status.success(), "{made:?}");
        let child = Command::new("/usr/bin/ssh-agent")
            .arg("-D")
            .arg("-a")
            .arg(&socket)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let agent = Self {
            child,
            socket,
            public_key: key.with_extension("pub"),
            _dir: dir,
        };
        wait_until("the throwaway ssh-agent's socket", || agent.socket.exists());
        let added = Command::new("/usr/bin/ssh-add")
            .arg("-q")
            .arg(&key)
            .env("SSH_AUTH_SOCK", &agent.socket)
            .output()
            .unwrap();
        assert!(added.status.success(), "{added:?}");
        fs::remove_file(&key).unwrap();
        Ok(agent)
    }
}

impl Drop for ThrowawayAgent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Makes the fixture's repository sign every commit's `-S` with ssh through `agent`'s key, in its
/// own local config.
fn sign_through(oc: &OpenCodeRepo, agent: &ThrowawayAgent) {
    let repo = &oc.fx.repo;
    repo.git(&["config", "gpg.format", "ssh"]);
    repo.git(&["config", "gpg.ssh.program", "/usr/bin/ssh-keygen"]);
    repo.git(&[
        "config",
        "user.signingkey",
        agent.public_key.to_str().unwrap(),
    ]);
}

/// Starts the fake opencode, running [`SIGNING_PROBES`], in the new worktree `name` through the
/// real sandbox with `agent`'s socket as `SSH_AUTH_SOCK`, and returns once the probes ended: the
/// worktree, the probes' results and what the attach printed.
fn probe_signing(
    oc: &OpenCodeRepo,
    agent: &ThrowawayAgent,
    name: &str,
) -> (PathBuf, String, String) {
    let worktree = oc.fx.new_worktree(name);
    let out = oc
        .command(&["agent", "start", name, "--agent", "opencode"])
        .env("SSH_AUTH_SOCK", &agent.socket)
        .env("ORCA_TERM_TEST_BINARY", env!("CARGO_BIN_EXE_orca-term"))
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let results = worktree.join("probe-results");
    wait_until("the probes to finish", || {
        fs::read_to_string(&results).is_ok_and(|text| text.contains("attach exit"))
    });
    let attach = fs::read_to_string(worktree.join("probe-attach")).unwrap();
    let results = fs::read_to_string(&results).unwrap();
    (worktree, results, attach)
}

#[test]
fn with_agent_socket_on_opencode_signs_through_the_agent_and_still_reaches_no_other_agent() {
    if !cfg!(target_os = "macos") {
        eprintln!("skipped: on Linux bwrap reaches pathname sockets without any rule");
        return;
    }
    let tool = match real_tool() {
        Ok(tool) => tool,
        Err(why) => {
            eprintln!("skipped: opencode's real sandbox cannot run here: {why}");
            return;
        }
    };
    let agent = match ThrowawayAgent::start() {
        Ok(agent) => agent,
        Err(why) => {
            eprintln!("skipped: no throwaway ssh-agent here: {why}");
            return;
        }
    };
    let oc = fixture_with(&tool);
    oc.write_opencode("1.18.0", SIGNING_PROBES);
    sign_through(&oc, &agent);
    let other = oc.fx.new_worktree("other");
    oc.ok(&["agent", "start", "other"]);
    oc.fx.wait_for_output("other", "fake claude ready");

    let (_, off, _) = probe_signing(&oc, &agent, "off");
    assert_eq!(
        off, "agent exit 2\ncommit failed\nattach exit 1\n",
        "without the setting the agent is out of reach"
    );

    let repo = &oc.fx.repo;
    repo.write(".git/orca-term.yaml", "sandbox:\n  agent_socket: true\n");
    let (on_worktree, on, attach) = probe_signing(&oc, &agent, "on");
    assert_eq!(on, "agent exit 0\ncommit signed\nattach exit 1\n", "{on}");
    let head = repo.git(&[
        "-C",
        on_worktree.to_str().unwrap(),
        "cat-file",
        "-p",
        "HEAD",
    ]);
    assert!(head.contains("-----BEGIN SSH SIGNATURE-----"), "{head}");
    assert!(
        attach.contains("could not connect") && attach.contains("Operation not permitted"),
        "the holder's socket stays denied: {attach}"
    );
    assert!(!other.join("probe-results").exists());
    oc.fx.wait_for_output("other", "fake claude ready");
}
