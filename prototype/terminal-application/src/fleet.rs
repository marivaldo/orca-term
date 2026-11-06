//! The fake fleet the side menu shows. Hard-coded: nothing here reads git or the core.

/// The frames of the working glyph, as the Neovim sidebar turns them.
pub const SPINNER: [&str; 4] = ["◐", "◓", "◑", "◒"];

/// The checks of the one pull request the fake fleet has: mark and name.
pub const CHECKS: [(&str, &str); 4] = [
    ("✓", "rust (ubuntu)"),
    ("✓", "rust (macos)"),
    ("✗", "lua (nvim stable)"),
    ("·", "deny"),
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum State {
    Waiting,
    Working,
    Finished,
    Idle,
}

impl State {
    pub fn word(self) -> &'static str {
        match self {
            State::Waiting => "waiting",
            State::Working => "working",
            State::Finished => "finished",
            State::Idle => "idle",
        }
    }

    /// The state's glyph; working spins with `tick`.
    pub fn glyph(self, tick: usize) -> &'static str {
        match self {
            State::Waiting => "?",
            State::Working => SPINNER[tick % SPINNER.len()],
            State::Finished => "●",
            State::Idle => "○",
        }
    }
}

pub struct Worktree {
    pub name: &'static str,
    pub state: State,
    /// The second line of its row.
    pub says: &'static str,
    /// The agent's short label (`cc` is claude-code, `oc` is opencode).
    pub agent: &'static str,
    pub age: &'static str,
    pub unseen: bool,
    pub pull_request: bool,
}

pub struct Project {
    pub name: &'static str,
    pub branch: &'static str,
    pub worktrees: Vec<Worktree>,
}

/// A place `j` and `k` stop at in the side menu: a project's header or one of its worktrees.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    Header(usize),
    Worktree(usize, usize),
}

pub fn fake() -> Vec<Project> {
    vec![
        Project {
            name: "app",
            branch: "main",
            worktrees: vec![
                Worktree {
                    name: "fix-login",
                    state: State::Waiting,
                    says: "Which of the two login flows should stay?",
                    agent: "cc",
                    age: "2m",
                    unseen: true,
                    pull_request: false,
                },
                Worktree {
                    name: "api-cache",
                    state: State::Working,
                    says: "Editing src/cache.rs",
                    agent: "cc",
                    age: "now",
                    unseen: false,
                    pull_request: false,
                },
                Worktree {
                    name: "docs-refresh",
                    state: State::Finished,
                    says: "3 files changed",
                    agent: "oc",
                    age: "1h",
                    unseen: false,
                    pull_request: true,
                },
            ],
        },
        Project {
            name: "api",
            branch: "main",
            worktrees: vec![Worktree {
                name: "rate-limit",
                state: State::Idle,
                says: "no changes",
                agent: "cc",
                age: "3d",
                unseen: false,
                pull_request: false,
            }],
        },
        Project {
            name: "empty",
            branch: "main",
            worktrees: vec![],
        },
    ]
}

/// A project's worktrees under the three attention sections, in the order they are drawn. A
/// section with nothing in it is left out.
pub fn sections(project: &Project) -> Vec<(&'static str, Vec<usize>)> {
    let mut needs_you = Vec::new();
    let mut running = Vec::new();
    let mut quiet = Vec::new();
    for (index, worktree) in project.worktrees.iter().enumerate() {
        if worktree.state == State::Waiting || worktree.unseen {
            needs_you.push(index);
        } else if worktree.state == State::Working {
            running.push(index);
        } else {
            quiet.push(index);
        }
    }
    [
        ("NEEDS YOU", needs_you),
        ("RUNNING", running),
        ("QUIET", quiet),
    ]
    .into_iter()
    .filter(|(_, members)| !members.is_empty())
    .collect()
}

/// Every stop of the side menu, top to bottom.
pub fn stops(fleet: &[Project]) -> Vec<Stop> {
    let mut stops = Vec::new();
    for (p, project) in fleet.iter().enumerate() {
        stops.push(Stop::Header(p));
        for (_, members) in sections(project) {
            for w in members {
                stops.push(Stop::Worktree(p, w));
            }
        }
    }
    stops
}
