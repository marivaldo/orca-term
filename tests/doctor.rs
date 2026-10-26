//! `orca-term doctor` and the `prerequisite` refusal, driven as a black box with fake programs on
//! an isolated PATH (seam 1): a program is missing when PATH lacks it, and outdated when a tiny
//! script stands in for it, printing an old version.

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
#[expect(
    dead_code,
    reason = "the fixture is shared between suites, and this one leaves part of it unused"
)]
mod support;

use std::path::Path;
use std::process::Output;

use fixture::AgentRepo;
use serde_json::{Value, json};

/// The tested claude, as `domain/prerequisite.rs` names it.
const TESTED: &str = "2.1.306";

/// The tested opencode, as `domain/prerequisite.rs` names it.
const OPENCODE_TESTED: &str = "1.18.0";

/// The newest version among the recordings in `tests/fixtures/<dir>`, named
/// `<recording>-<version>.log`.
fn newest_recorded(dir: &str) -> String {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(dir);
    std::fs::read_dir(dir)
        .unwrap()
        .filter_map(|entry| {
            let name = entry.unwrap().file_name().into_string().unwrap();
            let version = name.strip_suffix(".log")?.rsplit('-').next()?.to_owned();
            let parts: Vec<u64> = version.split('.').map(|p| p.parse().unwrap()).collect();
            Some((parts, version))
        })
        .max()
        .unwrap()
        .1
}

fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).unwrap()
}

fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).unwrap()
}

fn json(out: &Output) -> Value {
    serde_json::from_slice(&out.stdout)
        .unwrap_or_else(|err| panic!("not JSON ({err}): {}", stdout(out)))
}

/// The sandbox line on this OS, where the fixture provides every sandbox tool.
fn sandbox_ok() -> &'static str {
    if cfg!(target_os = "macos") {
        "ok       sandbox sandbox-exec"
    } else {
        "ok       sandbox bwrap and socat"
    }
}

/// Runs `doctor` with `args` from a directory that is no git repository.
fn doctor(fx: &AgentRepo, args: &[&str]) -> Output {
    let mut all = vec!["doctor"];
    all.extend_from_slice(args);
    fx.command(&all)
        .current_dir(fx.repo.tmp.path())
        .output()
        .unwrap()
}

/// The binary's version.
const VERSION: &str = env!("CARGO_PKG_VERSION");

#[test]
fn doctor_reports_every_prerequisite_outside_a_repository() {
    let fx = AgentRepo::new();
    fx.write_claude(&format!("{TESTED} (Claude Code)"), "");
    let git = fx.repo.git(&["--version"]);
    let git = git
        .trim()
        .trim_start_matches("git version ")
        .split(' ')
        .next()
        .unwrap();

    let out = doctor(&fx, &[]);

    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(
        stdout(&out),
        format!(
            "ok       git {git}\n{}\nok       claude {TESTED}\nok       opencode not installed \
             (optional)\ninfo     contract 1, orca-term {VERSION}\n",
            sandbox_ok(),
        )
    );
    assert_eq!(stderr(&out), "");
}

#[test]
fn a_newer_claude_and_an_old_opencode_are_warnings_never_refusals() {
    let fx = AgentRepo::new();
    fx.write_claude("2.1.310 (Claude Code)", "");
    fx.write_tool("opencode", "#!/bin/sh\necho 1.17.0\n");

    let out = doctor(&fx, &["--json"]);

    assert_eq!(out.status.code(), Some(0), "{}", stdout(&out));
    let doc = json(&out);
    assert_eq!(doc["contract"], 1);
    assert_eq!(doc["version"], VERSION);
    assert_eq!(doc.get("error"), None, "no error when nothing is missing");
    let checks = doc["checks"].as_array().unwrap();
    let names: Vec<&str> = checks.iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["git", "sandbox", "claude", "opencode", "contract"]);
    assert_eq!(
        checks[2],
        json!({
            "name": "claude",
            "status": "warn",
            "found": "2.1.310",
            "need": "2.1.295",
            "message": "claude 2.1.310: newer than the tested 2.1.306; it may still work",
        })
    );
    assert_eq!(
        checks[3],
        json!({
            "name": "opencode",
            "status": "warn",
            "found": "1.17.0",
            "need": "1.18.0",
            "message": "opencode 1.17.0: orca-term needs 1.18.0 or later",
        })
    );
    assert_eq!(
        checks[4],
        json!({
            "name": "contract",
            "status": "info",
            "found": "1",
            "need": null,
            "message": format!("contract 1, orca-term {VERSION}"),
        })
    );
}

/// The doctor's signing line (#105), as the text report shows it.
const SIGNING: &str = "info     signing: git signs with an ssh key whose private half is not a \
                       readable file, so it signs through your ssh agent; a worktree's agent can \
                       reach it only with `sandbox.agent_socket: true` in your global config file \
                       or .git/orca-term.yaml\n";

#[test]
fn doctor_names_the_agent_socket_setting_when_only_an_agent_holds_the_ssh_signing_key() {
    let fx = AgentRepo::new();
    fx.write_claude(&format!("{TESTED} (Claude Code)"), "");
    let keys = fx.repo.tmp.path().join("keys");
    std::fs::create_dir_all(&keys).unwrap();
    let public = keys.join("signing.pub");
    std::fs::write(&public, "ssh-ed25519 AAAAnotakey throwaway\n").unwrap();
    fx.repo.git(&["config", "gpg.format", "ssh"]);
    fx.repo
        .git(&["config", "user.signingkey", public.to_str().unwrap()]);
    let report = || {
        let out = fx.command(&["doctor"]).output().unwrap();
        assert_eq!(out.status.code(), Some(0), "information never refuses");
        stdout(&out)
    };

    let text = report();
    assert!(
        text.ends_with(&format!(
            "{SIGNING}info     contract 1, orca-term {VERSION}\n"
        )),
        "{text}"
    );
    assert!(!text.contains("signing.pub"), "never the key's path");
    let doc = json(&fx.command(&["doctor", "--json"]).output().unwrap());
    let checks = doc["checks"].as_array().unwrap();
    let names: Vec<&str> = checks.iter().map(|c| c["name"].as_str().unwrap()).collect();
    assert_eq!(
        names,
        [
            "git", "sandbox", "claude", "opencode", "signing", "contract"
        ]
    );
    assert_eq!(checks[4]["status"], "info");
    assert_eq!(checks[4]["found"], Value::Null);

    // The private half readable as a file: git signs without the agent.
    std::fs::write(keys.join("signing"), "not a key either\n").unwrap();
    assert!(!report().contains("signing:"));

    // A key written in the config itself, which only an agent can hold.
    fx.repo
        .git(&["config", "user.signingkey", "key::ssh-ed25519 AAAAnotakey"]);
    let text = report();
    assert!(text.contains(SIGNING), "{text}");
    assert!(!text.contains("AAAAnotakey"), "never the key");

    // Signing with openpgp is not judged.
    fx.repo.git(&["config", "gpg.format", "openpgp"]);
    assert!(!report().contains("signing:"));
}

#[test]
fn doctor_prints_the_whole_report_then_refuses_as_prerequisite() {
    let fx = AgentRepo::new();
    fx.write_tool("git", "#!/bin/sh\necho 'git version 2.30.1'\n");
    fx.remove_tool("claude");
    for tool in ["sandbox-exec", "bwrap", "socat"] {
        fx.remove_tool(tool);
    }

    let out = doctor(&fx, &[]);

    assert_eq!(out.status.code(), Some(9));
    let report = stdout(&out);
    let lines: Vec<&str> = report.lines().collect();
    assert_eq!(lines.len(), 5, "{report}");
    assert_eq!(
        lines[0],
        "too old  git 2.30.1: orca-term needs 2.38 or later"
    );
    assert!(lines[1].starts_with("missing  sandbox: "), "{report}");
    assert_eq!(
        lines[2],
        "missing  claude: not found on PATH; install Claude Code \
         (https://docs.anthropic.com/en/docs/claude-code)"
    );
    assert_eq!(lines[3], "ok       opencode not installed (optional)");
    assert!(lines[4].starts_with("info     contract 1"), "{report}");
    let sandbox = lines[1].trim_start_matches("missing  ");
    assert_eq!(
        stderr(&out),
        format!(
            "orca-term: git 2.30.1: orca-term needs 2.38 or later; {sandbox}; claude: not found \
             on PATH; install Claude Code (https://docs.anthropic.com/en/docs/claude-code); run \
             orca-term doctor for the full report\n"
        )
    );

    let out = doctor(&fx, &["--json"]);
    assert_eq!(out.status.code(), Some(9));
    let doc = json(&out);
    assert_eq!(doc["checks"][0]["status"], "too_old");
    assert_eq!(doc["checks"][1]["status"], "missing");
    assert_eq!(doc["checks"][2]["status"], "missing");
    assert_eq!(doc["checks"][2]["found"], Value::Null);
    assert_eq!(doc["error"]["kind"], "prerequisite");
    let message = doc["error"]["message"].as_str().unwrap();
    assert!(
        message.starts_with("git 2.30.1: orca-term needs 2.38 or later; sandbox: "),
        "{message}"
    );
    assert!(message.ends_with("; run orca-term doctor for the full report"));
}

#[test]
fn every_command_that_runs_git_refuses_an_old_or_missing_git_with_the_doctor_line() {
    let fx = AgentRepo::new();
    let worktree = fx.new_worktree("fix");
    let worktree = worktree.to_str().unwrap();
    fx.write_tool("git", "#!/bin/sh\necho 'git version 2.37.9'\n");
    let old = "git 2.37.9: orca-term needs 2.38 or later; run orca-term doctor for the full report";
    let commands: [&[&str]; 8] = [
        &["worktree", "ls"],
        &["worktree", "new", "other"],
        &["worktree", "prune"],
        &["agent", "start", "fix"],
        &["agent", "stop", "fix"],
        &["note", "ls", "fix"],
        &["turn", "ls", "fix"],
        &["nvim", "register", "/tmp/orca-term-test.sock"],
    ];
    for args in commands {
        let out = fx.run(args);
        assert_eq!(out.status.code(), Some(9), "{args:?}: {}", stderr(&out));
        assert_eq!(stderr(&out), format!("orca-term: {old}\n"), "{args:?}");
    }
    let out = fx.run(&["worktree", "ls", "--json"]);
    assert_eq!(out.status.code(), Some(9));
    let doc = json(&out);
    assert_eq!(doc["error"]["kind"], "prerequisite");
    assert_eq!(doc["error"]["message"], old);
    assert_eq!(fx.starts(), 0, "nothing was launched");

    fx.remove_tool("git");
    let out = fx.run(&["turn", "ls", "fix"]);
    assert_eq!(out.status.code(), Some(9));
    assert_eq!(
        stderr(&out),
        "orca-term: git: not found on PATH; install git 2.38 or later; run orca-term doctor for \
         the full report\n"
    );
    assert!(Path::new(worktree).is_dir(), "nothing was changed");
}

#[test]
fn the_hook_never_checks_git_and_stays_silent() {
    let fx = AgentRepo::new();
    fx.remove_tool("git");
    let admin = fx.repo.tmp.path().join("admin");

    let out = fx.run(&["hook", "claude", "--admin", admin.to_str().unwrap(), "Stop"]);

    assert_eq!(out.status.code(), Some(0));
    assert_eq!(stdout(&out), "");
    assert_eq!(stderr(&out), "");
}

#[test]
fn agent_start_refuses_a_claude_below_the_floor_or_missing_as_prerequisite() {
    let fx = AgentRepo::new();
    fx.new_worktree("fix");

    fx.write_claude("2.1.294 (Claude Code)", "");
    let out = fx.run(&["agent", "start", "fix", "--json"]);
    assert_eq!(out.status.code(), Some(9), "{}", stdout(&out));
    let doc = json(&out);
    assert_eq!(doc["error"]["kind"], "prerequisite");
    assert_eq!(
        doc["error"]["message"],
        "claude 2.1.294: orca-term needs 2.1.295 or later; run orca-term doctor for the full \
         report"
    );

    fx.remove_tool("claude");
    let out = fx.run(&["agent", "start", "fix"]);
    assert_eq!(out.status.code(), Some(9));
    assert_eq!(
        stderr(&out),
        "orca-term: claude: not found on PATH; install Claude Code \
         (https://docs.anthropic.com/en/docs/claude-code); run orca-term doctor for the full \
         report\n"
    );
    assert_eq!(fx.starts(), 0, "nothing was launched");
}

#[test]
fn the_tested_claude_is_the_newest_one_recorded_in_the_fixtures() {
    let newest = newest_recorded("claude-hooks");
    assert_eq!(
        newest, TESTED,
        "raise CLAUDE_TESTED in domain/prerequisite.rs"
    );

    let fx = AgentRepo::new();
    fx.write_claude(&format!("{newest} (Claude Code)"), "");
    let doc = json(&doctor(&fx, &["--json"]));
    assert_eq!(doc["checks"][2]["status"], "ok", "{doc}");
}

#[test]
fn the_tested_opencode_is_the_newest_one_recorded_in_the_fixtures() {
    let newest = newest_recorded("opencode-events");
    assert_eq!(
        newest, OPENCODE_TESTED,
        "raise OPENCODE_TESTED in domain/prerequisite.rs"
    );

    let fx = AgentRepo::new();
    fx.write_tool("opencode", &format!("#!/bin/sh\necho {newest}\n"));
    let doc = json(&doctor(&fx, &["--json"]));
    assert_eq!(doc["checks"][3]["name"], "opencode", "{doc}");
    assert_eq!(doc["checks"][3]["status"], "ok", "{doc}");

    fx.write_tool("opencode", "#!/bin/sh\necho 99.0.0\n");
    let doc = json(&doctor(&fx, &["--json"]));
    assert_eq!(doc["checks"][3]["status"], "warn", "{doc}");
    assert_eq!(
        doc["checks"][3]["message"],
        format!("opencode 99.0.0: newer than the tested {OPENCODE_TESTED}; it may still work")
    );
}
