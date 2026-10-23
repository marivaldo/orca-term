//! Ops: `doctor`, which checks every program orca-term needs (git, claude's sandbox, the agents)
//! and says which contract this binary speaks. It only looks: it installs nothing, needs no
//! repository and never fails; the edge turns a missing or too old program into the exit code.
//! When git signs with an ssh key only an agent holds, it also names the setting that lets a
//! worktree's agent sign (#105).

use crate::adapters::{env, fs, git};
use crate::domain::prerequisite::{self, Check, Report};
use crate::domain::sandbox::signing::{self, SshSigningKey};
use crate::domain::sandbox::{self, Platform};
use crate::ops::check_program;

/// Every check, in the order the report shows them: git, the sandbox, each agent, signing when it
/// needs the person's agent, the contract.
pub(crate) fn run() -> Report {
    let path_dirs = env::path_dirs();
    let mut checks = vec![
        check_program(&path_dirs, &prerequisite::GIT).1,
        sandbox::check(Platform::current(), &|tool| {
            fs::executable_in(&path_dirs, tool).is_some()
        }),
    ];
    for agent in prerequisite::AGENTS {
        checks.push(check_program(&path_dirs, agent).1);
    }
    checks.extend(signing_check());
    checks.push(prerequisite::contract_check());
    Report { checks }
}

/// The doctor's word on signing, when git's config in the current directory signs with an ssh key
/// whose private half is not a readable file, so only the person's agent can sign with it. `None`
/// otherwise, and when git cannot be asked. The key is never read nor printed.
fn signing_check() -> Option<Check> {
    let cwd = env::current_dir().ok()?;
    let format = git::config_value(&cwd, signing::FORMAT_KEY).ok()?;
    let key = git::config_value(&cwd, signing::SIGNING_KEY_KEY).ok()?;
    let home = env::config_env().home;
    let agent_only =
        match signing::ssh_signing_key(format.as_deref(), key.as_deref(), home.as_deref()) {
            SshSigningKey::NotJudged => false,
            SshSigningKey::Literal => true,
            SshSigningKey::PublicFile { private } => !fs::is_readable_file(&private),
        };
    agent_only.then(signing::agent_signing_check)
}
