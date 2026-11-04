//! `orca-term pr compose` and `orca-term pr create`, driven as a black box (seam 1) against a fake
//! `gh` on the isolated PATH of `tests/agent.rs` and a bare repository as `origin` (#132): the
//! message a new pull request starts from, the push, gh's arguments, each refusal, and the pull
//! request recorded in `pr.json` for `worktree ls`.

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
use std::path::{Path, PathBuf};
use std::process::Output;

use fixture::AgentRepo;
use serde_json::{Value, json};

/// The fake `gh`: it records the directory it ran in and its arguments in `gh.calls` beside it.
/// `gh pr create` prints `gh-create.err` on stderr and exits 1 when that file exists; else it
/// copies the body to `gh-create.body`, makes `gh pr view` answer for the `--head` branch with pull
/// request 12 (a draft with `--draft`; not when `gh-lag` exists, as GitHub may lag), and prints its
/// address. `gh pr view` answers for the branch it is asked about (its third argument): what
/// `gh-<branch>.json` holds, exit 0; else what `gh-<branch>.err` holds on stderr, exit 4; else
/// gh's own words for a branch with no pull request, exit 1. Shell builtins only: its PATH holds
/// nothing else.
const FAKE_GH: &str = r#"#!/bin/sh
dir="${0%/*}"
printf '%s|%s\n' "$(pwd -P)" "$*" >> "$dir/gh.calls"
if [ "$1 $2" = "pr create" ]; then
  if [ -f "$dir/gh-create.err" ]; then
    while IFS= read -r line || [ -n "$line" ]; do printf '%s\n' "$line" >&2; done < "$dir/gh-create.err"
    exit 1
  fi
  shift 2
  head=""; title=""; draft=false
  while [ $# -gt 0 ]; do
    case "$1" in
      --head) head="$2"; shift 2 ;;
      --title) title="$2"; shift 2 ;;
      --body-file)
        : > "$dir/gh-create.body"
        while IFS= read -r line || [ -n "$line" ]; do printf '%s\n' "$line" >> "$dir/gh-create.body"; done < "$2"
        shift 2 ;;
      --body) printf '%s' "$2" > "$dir/gh-create.body"; shift 2 ;;
      --draft) draft=true; shift ;;
      *) shift ;;
    esac
  done
  if [ ! -f "$dir/gh-lag" ]; then
    printf '{"number":12,"title":"%s","state":"OPEN","isDraft":%s,"url":"https://github.com/o/r/pull/12","statusCheckRollup":[]}\n' "$title" "$draft" > "$dir/gh-$head.json"
  fi
  printf 'https://github.com/o/r/pull/12\n'
  exit 0
fi
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

/// What `gh pr view` asks for, after the branch.
const VIEW_FIELDS: &str = "--json number,title,state,isDraft,url,statusCheckRollup";

/// A repository whose `origin` is a bare repository beside it, holding `main`, with worktrees
/// placed under the fixture's own directory, and the fake `gh` on the isolated PATH.
struct PrRepo {
    fixture: AgentRepo,
    origin: PathBuf,
    base: PathBuf,
}

impl PrRepo {
    fn new() -> Self {
        let fixture = AgentRepo::new();
        fixture.write_tool("gh", FAKE_GH);
        let tmp = fixture.repo.tmp.path().canonicalize().unwrap();
        let origin = tmp.join("origin.git");
        fixture
            .repo
            .git(&["init", "--quiet", "--bare", origin.to_str().unwrap()]);
        fixture
            .repo
            .git(&["remote", "add", "origin", origin.to_str().unwrap()]);
        fixture.repo.git(&["push", "--quiet", "origin", "main"]);
        let base = tmp.join("worktrees");
        fs::write(
            fixture.repo.root.join(".git/orca-term.yaml"),
            format!("base: {}\n", base.display()),
        )
        .unwrap();
        Self {
            fixture,
            origin,
            base,
        }
    }

    /// Creates the worktree `name` and returns its path.
    fn new_worktree(&self, name: &str) -> PathBuf {
        self.fixture.ok(&["worktree", "new", name]);
        self.base.join("repo").join(name).canonicalize().unwrap()
    }

    /// Commits a file in the worktree at `worktree` with `message`.
    fn commit(&self, worktree: &Path, file: &str, message: &str) {
        fs::write(worktree.join(file), message).unwrap();
        let dir = worktree.to_str().unwrap();
        self.fixture.repo.git(&["-C", dir, "add", file]);
        self.fixture
            .repo
            .git(&["-C", dir, "commit", "--quiet", "-m", message]);
    }

    fn run(&self, args: &[&str]) -> Output {
        self.fixture.run(args)
    }

    fn ok(&self, args: &[&str]) -> String {
        self.fixture.ok(args)
    }

    /// Runs the binary with `args` and `--json`, failing unless it succeeded, and parses what it
    /// printed.
    fn json_of(&self, args: &[&str]) -> Value {
        let mut all = args.to_vec();
        all.push("--json");
        serde_json::from_str(&self.ok(&all)).unwrap()
    }

    /// Runs the binary with `args` and `--json`, expecting it to fail with `code`, and returns the
    /// error object it printed.
    fn error_of(&self, args: &[&str], code: i32) -> Value {
        let mut all = args.to_vec();
        all.push("--json");
        let out = self.run(&all);
        assert_eq!(out.status.code(), Some(code), "{out:?}");
        let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
        doc["error"].clone()
    }

    /// What the fake `gh` recorded: one `<directory>|<arguments>` line per run.
    fn gh_calls(&self) -> Vec<String> {
        fs::read_to_string(self.fixture.bin.join("gh.calls"))
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    /// The commit `origin` has for `branch`, or `None` when it has no such branch.
    fn origin_has(&self, branch: &str) -> Option<String> {
        let found = self.fixture.repo.git(&[
            "--git-dir",
            self.origin.to_str().unwrap(),
            "for-each-ref",
            "--format=%(objectname)",
            &format!("refs/heads/{branch}"),
        ]);
        let found = found.trim();
        (!found.is_empty()).then(|| found.to_owned())
    }

    /// The commit `HEAD` names in the worktree at `worktree`.
    fn head(&self, worktree: &Path) -> String {
        self.fixture
            .repo
            .git(&["-C", worktree.to_str().unwrap(), "rev-parse", "HEAD"])
            .trim()
            .to_owned()
    }

    /// The `pr` of the worktree `name` as `worktree ls --json` prints it.
    fn listed_pr(&self, name: &str) -> Value {
        self.fixture.listed(name)["pr"].clone()
    }
}

/// `fetched_at` taken out of `pr`, after checking it is a time in milliseconds.
fn without_time(mut pr: Value) -> Value {
    let fetched_at = pr["fetched_at"].as_u64().unwrap();
    assert!(fetched_at > 1_600_000_000_000, "{fetched_at}");
    pr.as_object_mut().unwrap().remove("fetched_at");
    pr
}

/// The pull request the fake `gh` creates, as `pr refresh` shows it, without `fetched_at`.
fn created(title: &str, state: &str) -> Value {
    json!({
        "number": 12,
        "title": title,
        "state": state,
        "url": "https://github.com/o/r/pull/12",
        "checks": "none",
    })
}

#[test]
fn compose_with_no_commits_gives_the_branch_name_and_runs_no_gh() {
    let repo = PrRepo::new();
    repo.new_worktree("demo");

    let composed = repo.json_of(&["pr", "compose", "demo"]);

    assert_eq!(composed["contract"], 1);
    assert_eq!(composed["title"], "demo");
    assert_eq!(composed["body"], "");
    assert_eq!(composed["base"], "main");
    assert_eq!(composed["branch"], "demo");
    assert_eq!(composed["commits"], 0);
    assert_eq!(composed["uncommitted"], 0);
    assert_eq!(repo.ok(&["pr", "compose", "demo"]), "demo\n");
    assert_eq!(repo.gh_calls(), Vec::<String>::new(), "compose ran gh");
    assert_eq!(repo.origin_has("demo"), None, "compose pushed");
}

#[test]
fn compose_with_one_commit_gives_its_subject_and_body_and_with_several_the_subjects() {
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login\n\nThe token expired.");

    let one = repo.json_of(&["pr", "compose", "demo"]);
    assert_eq!(one["title"], "Fix the login");
    assert_eq!(one["body"], "The token expired.");
    assert_eq!(one["commits"], 1);
    assert_eq!(
        repo.ok(&["pr", "compose", "demo"]),
        "Fix the login\n\nThe token expired.\n"
    );

    repo.commit(&demo, "b.txt", "Add a test");
    let several = repo.json_of(&["pr", "compose", "demo"]);
    assert_eq!(several["title"], "demo");
    assert_eq!(several["body"], "- Fix the login\n- Add a test");
    assert_eq!(several["commits"], 2);
    assert_eq!(repo.gh_calls(), Vec::<String>::new(), "compose ran gh");
}

#[test]
fn compose_takes_the_body_from_the_template_and_counts_uncommitted_files() {
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login\n\nThe token expired.");
    fs::create_dir_all(demo.join(".github")).unwrap();
    fs::write(
        demo.join(".github/PULL_REQUEST_TEMPLATE.md"),
        "## What\n\n## Why\n",
    )
    .unwrap();
    fs::write(demo.join("a.txt"), "changed").unwrap();

    let composed = repo.json_of(&["pr", "compose", "demo"]);

    assert_eq!(composed["title"], "Fix the login");
    assert_eq!(composed["body"], "## What\n\n## Why");
    assert_eq!(composed["uncommitted"], 2, "the template and a.txt");
    assert_eq!(
        repo.ok(&["pr", "compose", "demo"]),
        "Fix the login\n\n## What\n\n## Why\n"
    );
}

#[test]
fn create_pushes_the_branch_creates_the_pull_request_and_records_it() {
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login");
    let body = repo.fixture.repo.tmp.path().join("body.md");
    fs::write(&body, "The token expired.\n").unwrap();

    let made = repo.json_of(&[
        "pr",
        "create",
        "demo",
        "--title",
        "Fix the login",
        "--body-file",
        body.to_str().unwrap(),
    ]);

    assert_eq!(made["contract"], 1);
    assert_eq!(made["name"], "demo");
    assert_eq!(made["path"], demo.to_str().unwrap());
    assert_eq!(made["pushed"], true);
    assert_eq!(made["uncommitted"], 0);
    assert_eq!(
        without_time(made["pr"].clone()),
        created("Fix the login", "open")
    );
    assert_eq!(repo.origin_has("demo"), Some(repo.head(&demo)), "pushed");
    let upstream = repo.fixture.repo.git(&[
        "-C",
        demo.to_str().unwrap(),
        "rev-parse",
        "--abbrev-ref",
        "demo@{upstream}",
    ]);
    assert_eq!(upstream.trim(), "origin/demo", "the upstream is set");
    let place = demo.display();
    assert_eq!(
        repo.gh_calls(),
        [
            format!("{place}|pr view demo {VIEW_FIELDS}"),
            format!(
                "{place}|pr create --base main --head demo --title Fix the login --body-file {}",
                body.display()
            ),
            format!("{place}|pr view demo {VIEW_FIELDS}"),
        ],
        "asked first, created, then read back as pr refresh reads it"
    );
    assert_eq!(
        fs::read_to_string(repo.fixture.bin.join("gh-create.body")).unwrap(),
        "The token expired.\n"
    );
    assert_eq!(
        without_time(repo.listed_pr("demo")),
        created("Fix the login", "open"),
        "worktree ls shows it at once"
    );
}

#[test]
fn a_draft_without_a_body_says_so_and_names_the_uncommitted_files_left_out() {
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login");
    fs::write(demo.join("notes.txt"), "not committed").unwrap();

    let out = repo.ok(&["pr", "create", "demo", "--title", "Fix it", "--draft"]);

    assert_eq!(
        out,
        "demo: created PR #12 (draft): https://github.com/o/r/pull/12\n1 file with uncommitted \
         changes was left out of the pull request\n"
    );
    let calls = repo.gh_calls();
    assert_eq!(
        calls[1],
        format!(
            "{}|pr create --base main --head demo --title Fix it --body  --draft",
            demo.display()
        )
    );
    assert_eq!(
        fs::read_to_string(repo.fixture.bin.join("gh-create.body")).unwrap(),
        ""
    );
    assert_eq!(repo.listed_pr("demo")["state"], "draft");
}

#[test]
fn a_pull_request_github_does_not_show_yet_is_recorded_from_its_address() {
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login");
    fs::write(repo.fixture.bin.join("gh-lag"), "").unwrap();

    let made = repo.json_of(&["pr", "create", "demo", "--title", "Fix it"]);

    assert_eq!(without_time(made["pr"].clone()), created("Fix it", "open"));
    assert_eq!(
        without_time(repo.listed_pr("demo")),
        created("Fix it", "open")
    );
}

#[test]
fn a_branch_with_no_commits_ahead_is_refused_before_anything_runs() {
    let repo = PrRepo::new();
    repo.new_worktree("demo");

    let error = repo.error_of(&["pr", "create", "demo", "--title", "Fix it"], 10);

    assert_eq!(error["kind"], "no_commits");
    assert_eq!(
        error["message"],
        "worktree demo has no commits ahead of main on its branch demo, so there is nothing to \
         open a pull request with; commit your changes first (nothing was pushed)"
    );
    assert_eq!(repo.gh_calls(), Vec::<String>::new());
    assert_eq!(repo.origin_has("demo"), None);
}

#[test]
fn a_branch_whose_pull_request_is_open_is_refused_naming_it() {
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login");
    let open = json!({
        "number": 7, "title": "t", "state": "OPEN", "isDraft": false,
        "url": "https://github.com/o/r/pull/7", "statusCheckRollup": [],
    });
    fs::write(repo.fixture.bin.join("gh-demo.json"), open.to_string()).unwrap();

    let error = repo.error_of(&["pr", "create", "demo", "--title", "Fix it"], 11);

    assert_eq!(error["kind"], "pr_exists");
    assert_eq!(
        error["message"],
        "worktree demo already has an open pull request, #7: https://github.com/o/r/pull/7 \
         (nothing was pushed)"
    );
    assert_eq!(repo.gh_calls().len(), 1, "only asked");
    assert_eq!(repo.origin_has("demo"), None);
}

#[test]
fn gh_missing_or_not_logged_in_is_a_prerequisite_and_nothing_is_pushed() {
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login");
    fs::write(
        repo.fixture.bin.join("gh-demo.err"),
        "To get started with GitHub CLI, please run:  gh auth login\n",
    )
    .unwrap();

    let error = repo.error_of(&["pr", "create", "demo", "--title", "Fix it"], 9);
    assert_eq!(error["kind"], "prerequisite");
    assert_eq!(
        error["message"],
        "cannot create a pull request for worktree demo: To get started with GitHub CLI, please \
         run:  gh auth login; install the GitHub CLI (gh) and log in with `gh auth login`, then \
         try again (nothing was pushed)"
    );

    repo.fixture.remove_tool("gh");
    let error = repo.error_of(&["pr", "create", "demo", "--title", "Fix it"], 9);
    assert_eq!(error["kind"], "prerequisite");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains(": GitHub CLI is unavailable;"),
        "{error}"
    );
    assert_eq!(repo.origin_has("demo"), None);
}

#[test]
fn a_failed_push_is_push_failed_with_gits_words_and_creates_nothing() {
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login");
    let gone = repo.fixture.repo.tmp.path().join("no-such-origin.git");
    repo.fixture
        .repo
        .git(&["remote", "set-url", "origin", gone.to_str().unwrap()]);

    let error = repo.error_of(&["pr", "create", "demo", "--title", "Fix it"], 12);

    assert_eq!(error["kind"], "push_failed");
    let message = error["message"].as_str().unwrap();
    assert!(
        message.starts_with("could not push branch demo of worktree demo to origin: "),
        "{message}"
    );
    assert!(
        message.contains("does not appear to be a git repository"),
        "git's words: {message}"
    );
    assert!(
        message.ends_with("; no pull request was created"),
        "{message}"
    );
    let calls = repo.gh_calls();
    assert_eq!(calls.len(), 1, "only asked: {calls:?}");
    assert_eq!(repo.listed_pr("demo"), Value::Null, "nothing recorded");
}

#[test]
fn a_failed_gh_create_says_why_and_that_the_branch_was_pushed() {
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login");
    fs::write(
        repo.fixture.bin.join("gh-create.err"),
        "pull request create failed: GraphQL: No commits between main and demo\n",
    )
    .unwrap();

    let error = repo.error_of(&["pr", "create", "demo", "--title", "Fix it"], 1);

    assert_eq!(error["kind"], "other");
    assert_eq!(
        error["message"],
        "gh pr create failed for worktree demo: pull request create failed: GraphQL: No commits \
         between main and demo; branch demo was pushed to origin, but no pull request was created"
    );
    assert_eq!(repo.origin_has("demo"), Some(repo.head(&demo)));
    assert_eq!(repo.listed_pr("demo"), Value::Null, "nothing recorded");
}

#[test]
fn an_empty_title_is_refused_before_anything_runs() {
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login");

    let error = repo.error_of(&["pr", "create", "demo", "--title", " "], 1);

    assert_eq!(
        error["message"],
        "the pull request's title is empty; pass one with --title"
    );
    assert_eq!(repo.gh_calls(), Vec::<String>::new());
    assert_eq!(repo.origin_has("demo"), None);
}

#[test]
fn a_body_file_that_does_not_exist_is_refused_before_the_push() {
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login");

    let args = [
        "pr",
        "create",
        "demo",
        "--title",
        "Fix it",
        "--body-file",
        "/nonexistent/body.md",
    ];
    let error = repo.error_of(&args, 1);

    assert_eq!(
        error["message"],
        "the body file /nonexistent/body.md does not exist (nothing was pushed)"
    );
    assert_eq!(repo.origin_has("demo"), None, "nothing pushed");
}
