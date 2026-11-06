//! The fake fleet the side menu shows: the design's (README, screenshot 4a). Hard-coded: nothing
//! here reads git or the core.

use crate::theme::State;

/// One check of a pull request: its state, name and what it says on the right.
pub struct Check {
    pub state: State,
    pub name: &'static str,
    pub detail: &'static str,
}

pub struct PullRequest {
    pub number: u32,
    pub title: &'static str,
    pub summary: &'static str,
    /// The glyph the side menu shows before the agent: the worst of the checks.
    pub check: State,
    pub checks: Vec<Check>,
}

pub struct Worktree {
    pub name: &'static str,
    pub branch: &'static str,
    pub primary: bool,
    /// The agent's name; `None` when no agent runs there (the primaries).
    pub agent: Option<&'static str>,
    pub state: State,
    pub last_message: &'static str,
    pub age: &'static str,
    pub unseen: bool,
    pub pr: Option<PullRequest>,
    pub ahead: u32,
    /// The agent stopped: the centre shows the failure card instead of starting it.
    pub failure: bool,
}

pub struct Project {
    pub name: &'static str,
    pub worktrees: Vec<Worktree>,
}

/// A worktree as (project, worktree) indexes.
pub type Key = (usize, usize);

fn worktree(name: &'static str, branch: &'static str, state: State) -> Worktree {
    Worktree {
        name,
        branch,
        primary: false,
        agent: Some("claude"),
        state,
        last_message: "",
        age: "",
        unseen: false,
        pr: None,
        ahead: 2,
        failure: false,
    }
}

fn primary(age: &'static str) -> Worktree {
    Worktree {
        primary: true,
        agent: None,
        last_message: "no agent running",
        age,
        ahead: 0,
        ..worktree("main", "main", State::Idle)
    }
}

pub fn fake() -> Vec<Project> {
    vec![
        Project {
            name: "app",
            worktrees: vec![
                primary("5h"),
                Worktree {
                    last_message: "Which login flow should stay?",
                    age: "2m",
                    unseen: true,
                    ..worktree("fix-login", "fix/login-flow", State::Waiting)
                },
                Worktree {
                    last_message: "Editing src/cache.rs",
                    age: "now",
                    ..worktree("api-cache", "feat/api-cache", State::Working)
                },
                Worktree {
                    agent: Some("opencode"),
                    last_message: "3 files changed",
                    age: "1h",
                    ahead: 3,
                    pr: Some(PullRequest {
                        number: 131,
                        title: "docs: refresh for 0.9",
                        summary: "draft · 1 failing · 1 running",
                        check: State::Failed,
                        checks: vec![
                            check(State::Done, "build", "1m 12s"),
                            check(State::Done, "lint", "38s"),
                            check(State::Failed, "spellcheck", "2 errors"),
                            check(State::Working, "preview deploy", "running"),
                            check(State::Waiting, "review", "requested"),
                        ],
                    }),
                    ..worktree("docs-refresh", "docs/0.9-refresh", State::Done)
                },
            ],
        },
        Project {
            name: "api",
            worktrees: vec![
                primary("2d"),
                Worktree {
                    last_message: "agent exited with code 1",
                    age: "12m",
                    unseen: true,
                    failure: true,
                    ..worktree("retry-jobs", "fix/retry-jobs", State::Failed)
                },
                Worktree {
                    last_message: "no changes",
                    age: "3d",
                    ..worktree("rate-limit", "feat/rate-limit", State::Idle)
                },
            ],
        },
        Project {
            name: "empty",
            worktrees: vec![],
        },
    ]
}

fn check(state: State, name: &'static str, detail: &'static str) -> Check {
    Check {
        state,
        name,
        detail,
    }
}

/// Every worktree in the side menu's order, top to bottom.
pub fn order(fleet: &[Project]) -> Vec<Key> {
    fleet
        .iter()
        .enumerate()
        .flat_map(|(p, project)| (0..project.worktrees.len()).map(move |w| (p, w)))
        .collect()
}

/// The status bar's counts, of the worktrees with an agent: needs you (waiting), failed,
/// running (working), quiet (done and idle).
pub fn counts(fleet: &[Project]) -> [usize; 4] {
    let mut counts = [0; 4];
    for worktree in fleet.iter().flat_map(|p| &p.worktrees) {
        if worktree.agent.is_none() {
            continue;
        }
        let at = match worktree.state {
            State::Waiting => 0,
            State::Failed => 1,
            State::Working => 2,
            State::Done | State::Idle => 3,
        };
        counts[at] += 1;
    }
    counts
}

/// A changed file of the right card.
#[derive(Clone)]
pub struct FileChange {
    /// `M`, `A` or `D`.
    pub kind: char,
    pub path: String,
    pub add: u32,
    pub del: u32,
}

/// The design's files, for the snapshots.
pub fn mock_files(name: &str) -> Vec<FileChange> {
    let list: &[(char, &str, u32, u32)] = match name {
        "fix-login" => &[
            ('M', "src/auth/login.rs", 18, 9),
            ('M', "src/auth/session.rs", 6, 2),
            ('A', "src/auth/flag.rs", 31, 0),
            ('D', "templates/password.html", 0, 10),
        ],
        "retry-jobs" => &[
            ('M', "src/jobs/runner.rs", 22, 4),
            ('A', "src/jobs/backoff.rs", 38, 0),
        ],
        "docs-refresh" => &[
            ('M', "README.md", 41, 9),
            ('M', "CONTRIBUTING.md", 12, 3),
            ('A', "CHANGELOG.md", 11, 0),
        ],
        _ => &[],
    };
    list.iter()
        .map(|(kind, path, add, del)| FileChange {
            kind: *kind,
            path: (*path).to_owned(),
            add: *add,
            del: *del,
        })
        .collect()
}
