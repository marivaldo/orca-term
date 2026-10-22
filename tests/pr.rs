//! `orca-term pr refresh`, driven as a black box (seam 1) against a fake `gh` on the isolated PATH
//! of `tests/agent.rs`: what it keeps in each worktree's `pr.json`, what it prints, and what
//! `worktree ls --json` and `worktree detail --json` then show as each worktree's `pr` (#87).

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

use std::fs;
use std::path::Path;

use fixture::AgentRepo;
use serde_json::{Value, json};

/// The fake `gh`: it records the directory it ran in and its arguments in `gh.calls` beside it,
/// then answers for the branch it is asked about (its third argument): what `gh-<branch>.json`
/// holds, exit 0; else what `gh-<branch>.err` holds on stderr, exit 4; else gh's own words for a
/// branch with no pull request, exit 1. Shell builtins only: its PATH holds nothing else.
const FAKE_GH: &str = r#"#!/bin/sh
dir="${0%/*}"
printf '%s|%s\n' "$(pwd -P)" "$*" >> "$dir/gh.calls"
branch="$3"
if [ -f "$dir/gh-$branch.json" ]; then
  while IFS= read -r line || [ -n "$line" ]; do printf '%s\n' "$line"; done < "$dir/gh-$branch.json"
  exit 0
fi
if [ -f "$dir/gh-$branch.err" ]; then
  while IFS= read -r line || [ -n "$line" ]; do printf '%s\n' "$line" >&2; done < "$dir/gh-$branch.err"
  exit 4
fi
printf 'no pull requests found for branch "%s"\n' "$branch" >&2
exit 1
"#;

/// The fixture with the fake `gh` on its PATH.
fn with_gh() -> AgentRepo {
    let fixture = AgentRepo::new();
    fixture.write_tool("gh", FAKE_GH);
    fixture
}

/// Makes the fake `gh` print `view` for `branch`, as `gh pr view --json` would.
fn answer(fixture: &AgentRepo, branch: &str, view: &Value) {
    fs::write(
        fixture.bin.join(format!("gh-{branch}.json")),
        view.to_string(),
    )
    .unwrap();
}

/// What `gh pr view --json number,title,state,isDraft,url,statusCheckRollup` prints for pull
/// request 12 in `state`, a draft or not, with the checks `rollup`.
fn view(state: &str, draft: bool, rollup: &Value) -> Value {
    json!({
        "number": 12,
        "title": "Fix the login",
        "state": state,
        "isDraft": draft,
        "url": "https://github.com/o/r/pull/12",
        "statusCheckRollup": rollup,
    })
}

/// A check run gh reports as still running, and one that succeeded.
fn checks_pending() -> Value {
    json!([
        {"__typename": "CheckRun", "name": "test", "status": "IN_PROGRESS", "conclusion": ""},
        {"__typename": "CheckRun", "name": "lint", "status": "COMPLETED", "conclusion": "SUCCESS"},
    ])
}

/// Runs the binary with `args` and `--json`, failing unless it succeeded, and parses what it
/// printed.
fn json_of(fixture: &AgentRepo, args: &[&str]) -> Value {
    let mut all = args.to_vec();
    all.push("--json");
    serde_json::from_str(&fixture.ok(&all)).unwrap()
}

/// The `pr` of the worktree `name` as `worktree ls --json` prints it.
fn listed_pr(fixture: &AgentRepo, name: &str) -> Value {
    fixture.listed(name)["pr"].clone()
}

/// What the fake `gh` recorded: one `<directory>|<arguments>` line per run.
fn gh_calls(fixture: &AgentRepo) -> Vec<String> {
    fs::read_to_string(fixture.bin.join("gh.calls"))
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect()
}

/// `fetched_at` taken out of `pr`, after checking it is a time in milliseconds.
fn without_time(mut pr: Value) -> Value {
    let fetched_at = pr["fetched_at"].as_u64().unwrap();
    assert!(fetched_at > 1_600_000_000_000, "{fetched_at}");
    pr.as_object_mut().unwrap().remove("fetched_at");
    pr
}

/// The `pr.json` of the worktree at `worktree`, parsed.
fn pr_file(fixture: &AgentRepo, worktree: &Path) -> Value {
    let text = fs::read_to_string(fixture.admin_dir(worktree).join("pr.json")).unwrap();
    serde_json::from_str(&text).unwrap()
}

#[test]
fn an_open_pull_request_with_running_checks_is_kept_and_shown_by_ls_and_detail() {
    let fixture = with_gh();
    let demo = fixture.new_worktree("demo");
    assert_eq!(listed_pr(&fixture, "demo"), Value::Null, "never fetched");
    answer(&fixture, "demo", &view("OPEN", false, &checks_pending()));

    let refreshed = json_of(&fixture, &["pr", "refresh"]);

    let shown = json!({
        "number": 12,
        "title": "Fix the login",
        "state": "open",
        "url": "https://github.com/o/r/pull/12",
        "checks": "pending",
    });
    assert_eq!(refreshed["contract"], 1);
    let worktrees = refreshed["worktrees"].as_array().unwrap();
    assert_eq!(worktrees.len(), 1);
    assert_eq!(worktrees[0]["name"], "demo");
    assert_eq!(worktrees[0]["path"], demo.to_str().unwrap());
    assert_eq!(without_time(worktrees[0]["pr"].clone()), shown);
    assert_eq!(without_time(listed_pr(&fixture, "demo")), shown);
    let detail = json_of(&fixture, &["worktree", "detail", "demo"]);
    assert_eq!(without_time(detail["worktree"]["pr"].clone()), shown);
    assert_eq!(
        gh_calls(&fixture),
        [format!(
            "{}|pr view demo --json number,title,state,isDraft,url,statusCheckRollup",
            demo.display()
        )],
        "gh asked once, in the worktree, about its branch"
    );
    let file = pr_file(&fixture, &demo);
    assert_eq!(file["schema"], 1);
    assert_eq!(file["unavailable"], Value::Null);
    assert_eq!(file["pr"]["state"], "open");
    assert_eq!(file["fetched_at"], worktrees[0]["pr"]["fetched_at"]);
}

#[test]
fn each_worktree_says_in_one_line_what_was_found_and_a_new_refresh_replaces_it() {
    let fixture = with_gh();
    fixture.new_worktree("demo");
    fixture.new_worktree("docs");
    answer(&fixture, "demo", &view("OPEN", true, &json!([])));

    let first = fixture.ok(&["pr", "refresh"]);
    assert_eq!(first, "demo: PR #12 draft, no checks\ndocs: no PR\n");
    assert_eq!(
        without_time(listed_pr(&fixture, "docs")),
        json!({"none": true})
    );

    let passed = json!([{"__typename": "StatusContext", "context": "ci", "state": "SUCCESS"}]);
    answer(&fixture, "demo", &view("MERGED", false, &passed));
    let second = fixture.ok(&["pr", "refresh"]);
    assert_eq!(second, "demo: PR #12 merged, checks passing\ndocs: no PR\n");
    assert_eq!(listed_pr(&fixture, "demo")["state"], "merged");
}

#[test]
fn without_gh_on_path_every_worktree_is_unavailable_and_the_command_succeeds() {
    let fixture = AgentRepo::new();
    let demo = fixture.new_worktree("demo");

    let out = fixture.run(&["pr", "refresh"]);

    assert!(out.status.success(), "{out:?}");
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "demo: PR unavailable: GitHub CLI is unavailable\n"
    );
    assert_eq!(
        without_time(listed_pr(&fixture, "demo")),
        json!({"unavailable": "GitHub CLI is unavailable"})
    );
    assert_eq!(pr_file(&fixture, &demo)["pr"], Value::Null);
}

#[test]
fn any_other_gh_failure_is_unavailable_with_its_first_line() {
    let fixture = with_gh();
    fixture.new_worktree("demo");
    fs::write(
        fixture.bin.join("gh-demo.err"),
        "To get started with GitHub CLI, please run:  gh auth login\nAlternatively, set GH_TOKEN.\n",
    )
    .unwrap();

    let refreshed = json_of(&fixture, &["pr", "refresh", "demo"]);

    assert_eq!(
        without_time(refreshed["worktrees"][0]["pr"].clone()),
        json!({"unavailable": "To get started with GitHub CLI, please run:  gh auth login"})
    );
}

#[test]
fn a_repository_with_no_remote_on_github_is_unavailable_as_not_a_github_remote() {
    let fixture = with_gh();
    fixture.new_worktree("demo");
    fs::write(
        fixture.bin.join("gh-demo.err"),
        "none of the git remotes configured for this repository point to a known GitHub host. \
         To tell gh about a new GitHub host, please use `gh auth login`\n",
    )
    .unwrap();

    assert_eq!(
        fixture.ok(&["pr", "refresh"]),
        "demo: PR unavailable: not a GitHub remote\n"
    );
    assert_eq!(
        without_time(listed_pr(&fixture, "demo")),
        json!({"unavailable": "not a GitHub remote"})
    );
}

#[test]
fn a_closed_pull_request_is_closed_with_its_failing_checks() {
    let fixture = with_gh();
    fixture.new_worktree("demo");
    let failed = json!([
        {"__typename": "CheckRun", "name": "test", "status": "COMPLETED", "conclusion": "FAILURE"},
    ]);
    answer(&fixture, "demo", &view("CLOSED", false, &failed));

    assert_eq!(
        fixture.ok(&["pr", "refresh"]),
        "demo: PR #12 closed, checks failing\n"
    );
    let pr = listed_pr(&fixture, "demo");
    assert_eq!(
        (&pr["state"], &pr["checks"]),
        (&json!("closed"), &json!("failing"))
    );
}

#[test]
fn named_worktrees_are_the_only_ones_asked_and_the_primary_or_an_unknown_one_is_refused() {
    let fixture = with_gh();
    let demo = fixture.new_worktree("demo");
    let docs = fixture.new_worktree("docs");

    assert_eq!(fixture.ok(&["pr", "refresh", "docs"]), "docs: no PR\n");
    assert!(!fixture.admin_dir(&demo).join("pr.json").exists());
    assert!(fixture.admin_dir(&docs).join("pr.json").exists());
    assert_eq!(listed_pr(&fixture, "demo"), Value::Null);

    let unknown = fixture.run(&["pr", "refresh", "nope", "--json"]);
    assert_eq!(unknown.status.code(), Some(1));
    let error: Value = serde_json::from_slice(&unknown.stdout).unwrap();
    assert_eq!(error["error"]["kind"], "other");
    let primary = fixture.run(&["pr", "refresh", "."]);
    assert_eq!(primary.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&primary.stderr).contains("is the primary checkout"),
        "{primary:?}"
    );
    assert_eq!(gh_calls(&fixture).len(), 1, "gh ran only for docs");
}

#[test]
fn listing_writes_no_pr_file_and_a_newer_schema_is_never_written_over() {
    let fixture = with_gh();
    let demo = fixture.new_worktree("demo");
    fixture.ok(&["worktree", "ls", "--json"]);
    fixture.ok(&["worktree", "detail", "demo", "--json"]);
    let file = fixture.admin_dir(&demo).join("pr.json");
    assert!(!file.exists(), "a read wrote pr.json");
    assert_eq!(gh_calls(&fixture), Vec::<String>::new(), "a read ran gh");

    let newer = r#"{"schema":2,"fetched_at":1,"pr":null,"unavailable":null}"#;
    fs::write(&file, newer).unwrap();
    assert_eq!(
        listed_pr(&fixture, "demo"),
        Value::Null,
        "read as never fetched"
    );
    let out = fixture.run(&["pr", "refresh"]);
    assert_eq!(out.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("pr.json has schema 2"),
        "{out:?}"
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), newer);
}
