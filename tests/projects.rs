//! The project list (#149): `orca-term project add|ls|rm` and `orca-term fleets`, driven as a
//! black box. Every command runs with `XDG_STATE_HOME` set to a temporary directory of the test,
//! so the list never lands in the developer's own state.

#![expect(
    clippy::unwrap_used,
    reason = "clippy.toml relaxes unwrap only inside #[test] fns; fixture helpers panic on failure too"
)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "a black-box test builds real repositories and files around the binary it drives"
)]

#[expect(
    dead_code,
    reason = "the fixture is shared between suites, and this one leaves part of it unused"
)]
mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use assert_fs::TempDir;
use serde_json::{Value, json};
use support::Repo;

/// A temporary `XDG_STATE_HOME`, which holds the project list.
struct State(TempDir);

impl State {
    fn new() -> Self {
        Self(TempDir::new().unwrap())
    }

    fn home(&self) -> &Path {
        self.0.path()
    }

    /// The project list's file.
    fn file(&self) -> PathBuf {
        self.home().join("orca-term").join("projects.json")
    }

    /// Runs the binary from `dir` with `args`, `repo`'s git environment and this state.
    fn run(&self, repo: &Repo, dir: &Path, args: &[&str]) -> Output {
        repo.orca_term(dir)
            .env("XDG_STATE_HOME", self.home())
            .args(args)
            .output()
            .unwrap()
    }

    /// Runs the binary, failing the test unless it succeeded, and returns its stdout.
    fn ok(&self, repo: &Repo, dir: &Path, args: &[&str]) -> String {
        let out = self.run(repo, dir, args);
        assert!(
            out.status.success(),
            "{args:?} failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// Runs the binary with `args` and `--json`, which must succeed, and parses its document.
    fn json(&self, repo: &Repo, dir: &Path, args: &[&str]) -> Value {
        let mut args = args.to_vec();
        args.push("--json");
        serde_json::from_str(&self.ok(repo, dir, &args)).unwrap()
    }

    /// The listed projects' paths, from `project ls --json`.
    fn listed(&self, repo: &Repo) -> Vec<String> {
        self.json(repo, &repo.root, &["project", "ls"])["projects"]
            .as_array()
            .unwrap()
            .iter()
            .map(|project| project["path"].as_str().unwrap().to_owned())
            .collect()
    }
}

fn text(path: &Path) -> String {
    path.to_str().unwrap().to_owned()
}

/// A new git repository with no commit at `parent/name`, made with no global or system config.
fn init_repository(parent: &Path, name: &str) -> PathBuf {
    let dir = parent.join(name);
    std::fs::create_dir_all(&dir).unwrap();
    let out = Command::new("git")
        .args(["init", "--quiet", "--initial-branch=main"])
        .arg(&dir)
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    dir.canonicalize().unwrap()
}

#[test]
fn adding_from_the_primary_checkout_a_worktree_or_a_subdirectory_adds_the_primary_checkout() {
    let repo = Repo::new();
    let sub = repo.write("src/lib.txt", "x\n");
    let worktree = repo.add_worktree(&repo.tmp.path().join("fix"), "fix");

    let from_worktree = State::new();
    let added = from_worktree.json(&repo, &worktree, &["project", "add"]);
    assert_eq!(
        added,
        json!({"contract": added["contract"], "version": added["version"], "name": "repo", "path": text(&repo.root), "added": true})
    );
    assert_eq!(from_worktree.listed(&repo), [text(&repo.root)]);

    let from_subdirectory = State::new();
    let out = from_subdirectory.ok(&repo, sub.parent().unwrap(), &["project", "add"]);
    assert_eq!(
        out.trim(),
        format!("added project repo at {}", repo.root.display())
    );
    assert_eq!(from_subdirectory.listed(&repo), [text(&repo.root)]);

    let from_primary = State::new();
    from_primary.ok(&repo, repo.tmp.path(), &["project", "add", "repo"]);
    assert_eq!(from_primary.listed(&repo), [text(&repo.root)]);
    assert_eq!(
        std::fs::read_to_string(from_primary.file()).unwrap(),
        format!(
            r#"{{"schema":1,"projects":[{{"path":"{}"}}]}}"#,
            repo.root.display()
        )
    );
}

#[test]
fn adding_twice_changes_nothing_and_says_so() {
    let repo = Repo::new();
    let state = State::new();
    state.ok(&repo, &repo.root, &["project", "add"]);
    let before = std::fs::read(state.file()).unwrap();

    let out = state.ok(&repo, &repo.root, &["project", "add"]);
    assert_eq!(
        out.trim(),
        format!(
            "project repo at {} is already on the list",
            repo.root.display()
        )
    );
    let again = state.json(&repo, &repo.root, &["project", "add"]);
    assert_eq!(again["added"], false);
    assert_eq!(std::fs::read(state.file()).unwrap(), before);
    assert_eq!(state.listed(&repo), [text(&repo.root)]);
}

#[test]
fn projects_added_at_the_same_time_are_all_kept() {
    let repo = Repo::new();
    let state = State::new();
    let dirs: Vec<PathBuf> = (0..8)
        .map(|n| init_repository(repo.tmp.path(), &format!("p{n}")))
        .collect();
    let children: Vec<_> = dirs
        .iter()
        .map(|dir| {
            repo.command(dir)
                .env("XDG_STATE_HOME", state.home())
                .args(["project", "add"])
                .stdout(std::process::Stdio::null())
                .spawn()
                .unwrap()
        })
        .collect();
    for mut child in children {
        assert!(child.wait().unwrap().success());
    }
    let mut listed = state.listed(&repo);
    listed.sort();
    let mut wanted: Vec<String> = dirs.iter().map(|dir| text(dir)).collect();
    wanted.sort();
    assert_eq!(listed, wanted, "the lock lost no addition");
}

#[test]
fn adding_a_directory_outside_a_repository_is_refused_as_not_a_repository() {
    let repo = Repo::new();
    let state = State::new();
    let outside = TempDir::new().unwrap();
    let outside = outside.path().canonicalize().unwrap();

    let out = state.run(&repo, &outside, &["project", "add", "--json"]);
    assert_eq!(out.status.code(), Some(22));
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"]["kind"], "not_a_repository");
    assert_eq!(
        doc["error"]["message"],
        format!(
            "{} is not inside a git repository, so it cannot be added as a project (nothing was \
             added)",
            outside.display()
        )
    );

    let out = state.run(&repo, &repo.root, &["project", "add", &text(&outside)]);
    assert_eq!(out.status.code(), Some(22));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("is not inside a git repository"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(!state.file().exists(), "nothing was written");
}

#[test]
fn listing_marks_a_gone_project_and_keeps_it() {
    let repo = Repo::new();
    let state = State::new();
    let gone = init_repository(repo.tmp.path(), "old");
    state.ok(&repo, &repo.root, &["project", "add"]);
    state.ok(&repo, &gone, &["project", "add"]);
    std::fs::remove_dir_all(&gone).unwrap();

    let listed = state.json(&repo, &repo.root, &["project", "ls"]);
    assert_eq!(
        listed["projects"],
        json!([
            {"name": "repo", "path": text(&repo.root), "gone": false},
            {"name": "old", "path": text(&gone), "gone": true},
        ])
    );
    let table = state.ok(&repo, &repo.root, &["project", "ls"]);
    let lines: Vec<&str> = table.lines().collect();
    assert_eq!(lines.len(), 3, "{table}");
    assert!(
        lines[0].starts_with("NAME") && lines[0].ends_with("STATE"),
        "{table}"
    );
    assert!(
        lines[1].starts_with("repo") && !lines[1].ends_with("gone"),
        "{table}"
    );
    assert!(
        lines[2].starts_with("old") && lines[2].ends_with("gone"),
        "{table}"
    );
}

#[test]
fn an_empty_list_says_how_to_add_a_project() {
    let repo = Repo::new();
    let state = State::new();
    let out = state.ok(&repo, &repo.root, &["project", "ls"]);
    assert_eq!(
        out.trim(),
        "no projects yet; add one with `orca-term project add`"
    );
    assert_eq!(
        state.json(&repo, &repo.root, &["project", "ls"])["projects"],
        json!([])
    );
    assert_eq!(
        state.json(&repo, &repo.root, &["fleets"])["projects"],
        json!([])
    );
}

#[test]
fn removing_by_path_by_name_and_with_an_ambiguous_name() {
    let first = Repo::new();
    let second = Repo::new();
    let state = State::new();
    let api = init_repository(first.tmp.path(), "api");
    let worktree = first.add_worktree(&first.tmp.path().join("fix"), "fix");
    for dir in [&first.root, &second.root, &api] {
        state.ok(&first, dir, &["project", "add"]);
    }

    // Both repositories are named `repo`.
    let out = state.run(&first, &first.root, &["project", "rm", "repo", "--json"]);
    assert_eq!(out.status.code(), Some(23));
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"]["kind"], "ambiguous_project");
    assert_eq!(
        doc["error"]["candidates"],
        json!([text(&first.root), text(&second.root)])
    );
    assert_eq!(
        doc["error"]["message"],
        format!(
            "several projects are named repo: {}, {}; remove one by its path (nothing was removed)",
            first.root.display(),
            second.root.display()
        )
    );
    assert_eq!(state.listed(&first).len(), 3);

    // A path inside a project, here its linked worktree, names the project.
    let removed = state.json(&first, &worktree, &["project", "rm", "."]);
    assert_eq!(removed["name"], "repo");
    assert_eq!(removed["path"], text(&first.root));
    assert_eq!(state.listed(&first), [text(&second.root), text(&api)]);

    let out = state.ok(&first, &first.root, &["project", "rm", "api"]);
    assert_eq!(
        out.trim(),
        format!(
            "removed project api at {} from the list (nothing on disk was touched)",
            api.display()
        )
    );
    assert_eq!(state.listed(&first), [text(&second.root)]);

    let out = state.run(&first, &first.root, &["project", "rm", "api"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no project named api is on the list"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Nothing on disk was touched.
    assert!(api.join(".git").is_dir());
    assert!(worktree.is_dir());
    assert!(first.git(&["branch", "--list", "fix"]).contains("fix"));
}

#[test]
fn removing_a_gone_project_by_its_path() {
    let repo = Repo::new();
    let state = State::new();
    let gone = init_repository(repo.tmp.path(), "old");
    state.ok(&repo, &gone, &["project", "add"]);
    std::fs::remove_dir_all(&gone).unwrap();
    state.ok(&repo, &repo.root, &["project", "rm", &text(&gone)]);
    assert_eq!(state.listed(&repo), Vec::<String>::new());
}

/// A gone project as `fleets --json` reports it: marked gone, with no fleet and the error saying
/// how to take it off the list.
fn check_the_gone_project(project: &Value, gone: &Path) {
    assert_eq!(project["name"], "old");
    assert_eq!(project["gone"], true);
    assert_eq!(project["fleet"], Value::Null);
    assert_eq!(
        project["error"]["message"],
        format!(
            "the project's directory {} is gone; take it off the list with `orca-term project rm \
             {}`",
            gone.display(),
            gone.display()
        )
    );
}

#[test]
fn fleets_reads_every_project_and_reports_a_broken_and_a_gone_one() {
    let first = Repo::new();
    let second = Repo::new();
    let state = State::new();
    first.add_worktree(&first.tmp.path().join("fix"), "fix");
    let broken = init_repository(first.tmp.path(), "broken");
    let gone = init_repository(first.tmp.path(), "old");
    for dir in [&first.root, &broken, &second.root, &gone] {
        state.ok(&first, dir, &["project", "add"]);
    }
    std::fs::remove_dir_all(broken.join(".git")).unwrap();
    std::fs::remove_dir_all(&gone).unwrap();

    let doc = state.json(&first, &first.root, &["fleets"]);
    let projects = doc["projects"].as_array().unwrap();
    assert_eq!(projects.len(), 4);

    // A readable project carries what `worktree ls --json` prints for it.
    let mut listed = state.json(&first, &first.root, &["worktree", "ls"]);
    listed.as_object_mut().unwrap().remove("contract");
    listed.as_object_mut().unwrap().remove("version");
    assert_eq!(projects[0]["name"], "repo");
    assert_eq!(projects[0]["path"], text(&first.root));
    assert_eq!(projects[0]["gone"], false);
    assert_eq!(projects[0]["fleet"], listed);
    assert_eq!(projects[0]["error"], Value::Null);
    assert_eq!(projects[0]["fleet"]["worktrees"][0]["name"], "fix");

    assert_eq!(projects[1]["name"], "broken");
    assert_eq!(projects[1]["gone"], false);
    assert_eq!(projects[1]["fleet"], Value::Null);
    assert_eq!(projects[1]["error"]["kind"], "other");
    assert!(
        projects[1]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("not inside a git repository"),
        "{}",
        projects[1]["error"]
    );

    assert_eq!(projects[2]["path"], text(&second.root));
    assert_eq!(projects[2]["fleet"]["worktrees"], json!([]));
    assert_eq!(projects[2]["error"], Value::Null);

    check_the_gone_project(&projects[3], &gone);

    let table = state.ok(&first, &first.root, &["fleets"]);
    assert!(
        table.starts_with(&format!("repo  {}\n  NAME", first.root.display())),
        "{table}"
    );
    assert!(
        table.contains(&format!(
            "\n\nold  {}\n  the project's directory",
            gone.display()
        )),
        "{table}"
    );
}
