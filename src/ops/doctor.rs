//! Ops: `doctor`, which checks every program orca-term needs (git, claude's sandbox, the agents)
//! and says which contract this binary speaks. It only looks: it installs nothing, needs no
//! repository and never fails; the edge turns a missing or too old program into the exit code.

use crate::adapters::{env, fs};
use crate::domain::prerequisite::{self, Report};
use crate::domain::sandbox::{self, Platform};
use crate::ops::check_program;

/// Every check, in the order the report shows them: git, the sandbox, each agent, the contract.
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
    checks.push(prerequisite::contract_check());
    Report { checks }
}
