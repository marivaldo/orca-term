//! `orca-term pr compose`, `orca-term pr create` and `orca-term pr merge`, driven as a black box
//! (seam 1) against a fake `gh` on the isolated PATH of `tests/agent.rs` and a bare repository as
//! `origin` (#132, #136): the message a new pull request starts from, the push, gh's arguments,
//! each refusal, the merge, and the pull request recorded in `pr.json` for `worktree ls`. Last,
//! `worktree rm` deleting the branch of a pull request recorded merged at the branch's tip (#137).

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
/// request 12 (a draft with `--draft`; not when `gh-lag` exists, as GitHub may lag) whose
/// `headRefOid` is the branch's commit (read from the repository's loose ref), and prints its
/// address. `gh pr view` answers for the branch it is asked about (its third argument): what
/// `gh-<branch>.json` holds, exit 0; else what `gh-<branch>.err` holds on stderr, exit 4; else
/// gh's own words for a branch with no pull request, exit 1. `gh pr merge <number>` (#136) prints
/// `gh-merge.err` on stderr and exits 1 when that file exists; else it turns `"state":"OPEN"` into
/// `"state":"MERGED"` in the `gh-<branch>.json` of that number and exits 0. When `gh-merge-held`
/// exists it exits 0 and leaves the pull request open, as a merge queue or a required check may;
/// when `gh-merge-forget` exists it empties that file instead, so gh can no longer say. Shell
/// builtins only: its PATH holds nothing else.
const FAKE_GH: &str = r#"#!/bin/sh
dir="${0%/*}"
printf '%s|%s\n' "$(pwd -P)" "$*" >> "$dir/gh.calls"
if [ "$1 $2" = "pr merge" ]; then
  if [ -f "$dir/gh-merge.err" ]; then
    while IFS= read -r line || [ -n "$line" ]; do printf '%s\n' "$line" >&2; done < "$dir/gh-merge.err"
    exit 1
  fi
  [ -f "$dir/gh-merge-held" ] && exit 0
  open='"state":"OPEN"'
  for file in "$dir"/gh-*.json; do
    [ -f "$file" ] || continue
    line=""
    IFS= read -r line < "$file" || :
    case "$line" in
      *"\"number\":$3,"*|*"\"number\":$3}"*)
        if [ -f "$dir/gh-merge-forget" ]; then
          : > "$file"
        else
          case "$line" in
            *$open*) printf '%s"state":"MERGED"%s\n' "${line%%$open*}" "${line#*$open}" > "$file" ;;
          esac
        fi ;;
    esac
  done
  exit 0
fi
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
  oid=""
  if [ -f .git ]; then
    IFS= read -r gitfile < .git || :
    gitdir="${gitfile#gitdir: }"
    ref="${gitdir%/worktrees/*}/refs/heads/$head"
    [ -f "$ref" ] && { IFS= read -r oid < "$ref" || :; }
  fi
  if [ ! -f "$dir/gh-lag" ]; then
    printf '{"number":12,"title":"%s","state":"OPEN","isDraft":%s,"url":"https://github.com/o/r/pull/12","statusCheckRollup":[],"headRefOid":"%s"}\n' "$title" "$draft" "$oid" > "$dir/gh-$head.json"
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
const VIEW_FIELDS: &str = "--json number,title,state,isDraft,url,statusCheckRollup,headRefOid";

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

/// What `gh pr view` asks for before a merge, after the branch.
const MERGE_VIEW_FIELDS: &str =
    "--json number,title,state,isDraft,url,statusCheckRollup,headRefOid,mergeable,reviewDecision";

/// Makes the fake `gh` answer for the branch `demo` with pull request 12 in `state` (`OPEN`,
/// `MERGED` or `CLOSED`), a draft or not, with `mergeable` and `reviewDecision` as gh prints them,
/// and one passing check.
fn answer_pr(repo: &PrRepo, state: &str, draft: bool, mergeable: &str, review: &str) {
    let pr = json!({
        "number": 12, "title": "Fix the login", "state": state, "isDraft": draft,
        "url": "https://github.com/o/r/pull/12",
        "statusCheckRollup": [{"status": "COMPLETED", "conclusion": "SUCCESS"}],
        "mergeable": mergeable, "reviewDecision": review,
    });
    fs::write(repo.fixture.bin.join("gh-demo.json"), pr.to_string()).unwrap();
}

/// The runs of the fake `gh` that merged, their arguments only.
fn merges(repo: &PrRepo) -> Vec<String> {
    repo.gh_calls()
        .iter()
        .filter_map(|call| call.split_once('|').map(|(_, args)| args.to_owned()))
        .filter(|args| args.starts_with("pr merge"))
        .collect()
}

/// The merged pull request the fake `gh` reports, as `pr refresh` shows it, without `fetched_at`.
fn merged_pr() -> Value {
    json!({
        "number": 12,
        "title": "Fix the login",
        "state": "merged",
        "url": "https://github.com/o/r/pull/12",
        "checks": "passing",
    })
}

#[test]
fn merge_squashes_by_default_records_it_merged_and_leaves_the_worktree_and_its_branch() {
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login");
    answer_pr(&repo, "OPEN", false, "MERGEABLE", "APPROVED");
    let head = repo.head(&demo);

    let merged = repo.json_of(&["pr", "merge", "demo"]);

    assert_eq!(merged["contract"], 1);
    assert_eq!(merged["name"], "demo");
    assert_eq!(merged["path"], demo.to_str().unwrap());
    assert_eq!(merged["method"], "squash");
    assert_eq!(merged["merged"], true);
    assert_eq!(without_time(merged["pr"].clone()), merged_pr());
    let place = demo.display();
    assert_eq!(
        repo.gh_calls(),
        [
            format!("{place}|pr view demo {MERGE_VIEW_FIELDS}"),
            format!("{place}|pr merge 12 --squash"),
            format!("{place}|pr view demo {VIEW_FIELDS}"),
        ],
        "asked with the merge fields, merged by number, then read back as pr refresh reads it"
    );
    assert!(
        repo.gh_calls()
            .iter()
            .all(|call| !call.contains("--delete-branch")),
        "never deletes the branch"
    );
    assert_eq!(
        without_time(repo.listed_pr("demo")),
        merged_pr(),
        "worktree ls shows it merged at once"
    );
    assert!(demo.is_dir(), "the worktree stays");
    assert_eq!(repo.head(&demo), head, "the worktree is untouched");
    let branch = repo
        .fixture
        .repo
        .git(&["rev-parse", "--verify", "refs/heads/demo"]);
    assert_eq!(branch.trim(), head, "the local branch stays");
}

#[test]
fn each_method_passes_its_flag_and_says_it_in_one_line() {
    let repo = PrRepo::new();
    repo.new_worktree("demo");
    for (flag, method) in [
        ("--squash", "squash"),
        ("--merge", "merge"),
        ("--rebase", "rebase"),
    ] {
        answer_pr(&repo, "OPEN", false, "MERGEABLE", "");

        let out = repo.ok(&["pr", "merge", "demo", flag]);

        assert_eq!(
            out,
            format!("demo: merged PR #12 ({method}): https://github.com/o/r/pull/12\n")
        );
        assert_eq!(
            merges(&repo).last().unwrap(),
            &format!("pr merge 12 {flag}")
        );
    }
    assert_eq!(merges(&repo).len(), 3);
}

#[test]
fn two_methods_at_once_are_a_usage_error() {
    let repo = PrRepo::new();
    repo.new_worktree("demo");
    answer_pr(&repo, "OPEN", false, "MERGEABLE", "");

    let out = repo.run(&["pr", "merge", "demo", "--squash", "--rebase"]);

    assert_eq!(out.status.code(), Some(2), "{out:?}");
    assert_eq!(repo.gh_calls(), Vec::<String>::new());
}

#[test]
fn no_open_pull_request_a_draft_a_conflict_or_a_blocking_review_is_refused_merging_nothing() {
    let repo = PrRepo::new();
    repo.new_worktree("demo");

    let error = repo.error_of(&["pr", "merge", "demo"], 13);
    assert_eq!(error["kind"], "no_open_pr");
    assert_eq!(
        error["message"],
        "worktree demo has no pull request to merge; create one first with `orca-term pr create \
         demo` (nothing was merged)"
    );

    let cases = [
        (
            ("MERGED", false, "UNKNOWN", ""),
            13,
            "no_open_pr",
            "PR #12 of worktree demo is merged already; there is nothing to merge",
        ),
        (
            ("CLOSED", false, "UNKNOWN", ""),
            13,
            "no_open_pr",
            "PR #12 of worktree demo is closed; reopen it on GitHub first (nothing was merged)",
        ),
        (
            ("OPEN", true, "MERGEABLE", ""),
            14,
            "pr_draft",
            "PR #12 of worktree demo is a draft; mark it ready for review first (nothing was \
             merged)",
        ),
        (
            ("OPEN", false, "CONFLICTING", "APPROVED"),
            15,
            "pr_conflicting",
            "PR #12 of worktree demo conflicts with its base branch on GitHub; update the branch \
             first (nothing was merged)",
        ),
        (
            ("OPEN", false, "MERGEABLE", "CHANGES_REQUESTED"),
            16,
            "not_approved",
            "PR #12 of worktree demo has changes requested in its review; address them and get \
             it approved first (nothing was merged)",
        ),
        (
            ("OPEN", false, "MERGEABLE", "REVIEW_REQUIRED"),
            16,
            "not_approved",
            "PR #12 of worktree demo needs an approving review first (nothing was merged)",
        ),
    ];
    for ((state, draft, mergeable, review), code, kind, message) in cases {
        answer_pr(&repo, state, draft, mergeable, review);

        let error = repo.error_of(&["pr", "merge", "demo"], code);

        assert_eq!(error["kind"], kind, "{state} {mergeable} {review}");
        assert_eq!(error["message"], message);
    }
    assert_eq!(merges(&repo), Vec::<String>::new(), "nothing was merged");
    assert_eq!(repo.listed_pr("demo"), Value::Null, "nothing recorded");
}

#[test]
fn gh_missing_or_not_logged_in_is_a_prerequisite_and_nothing_is_merged() {
    let repo = PrRepo::new();
    repo.new_worktree("demo");
    fs::write(
        repo.fixture.bin.join("gh-demo.err"),
        "To get started with GitHub CLI, please run:  gh auth login\n",
    )
    .unwrap();

    let error = repo.error_of(&["pr", "merge", "demo"], 9);
    assert_eq!(error["kind"], "prerequisite");
    assert_eq!(
        error["message"],
        "cannot merge the pull request of worktree demo: To get started with GitHub CLI, please \
         run:  gh auth login; install the GitHub CLI (gh) and log in with `gh auth login`, then \
         try again (nothing was merged)"
    );
    assert_eq!(merges(&repo), Vec::<String>::new());

    repo.fixture.remove_tool("gh");
    let error = repo.error_of(&["pr", "merge", "demo"], 9);
    assert_eq!(error["kind"], "prerequisite");
    assert!(
        error["message"]
            .as_str()
            .unwrap()
            .contains(": GitHub CLI is unavailable;"),
        "{error}"
    );
}

#[test]
fn anything_else_github_refuses_comes_back_as_ghs_first_line() {
    let repo = PrRepo::new();
    repo.new_worktree("demo");
    answer_pr(&repo, "OPEN", false, "MERGEABLE", "APPROVED");
    fs::write(
        repo.fixture.bin.join("gh-merge.err"),
        "X Pull request o/r#12 is not mergeable: the base branch policy prohibits the merge.\n\
         To have the pull request merged after all the requirements have been met, add the \
         `--auto` flag.\n",
    )
    .unwrap();

    let error = repo.error_of(&["pr", "merge", "demo"], 1);

    assert_eq!(error["kind"], "other");
    assert_eq!(
        error["message"],
        "gh pr merge failed for PR #12 of worktree demo: X Pull request o/r#12 is not mergeable: \
         the base branch policy prohibits the merge."
    );
    assert_eq!(merges(&repo), ["pr merge 12 --squash"]);
    assert_eq!(repo.listed_pr("demo"), Value::Null, "nothing recorded");
}

#[test]
fn a_merge_gh_cannot_read_back_is_recorded_merged() {
    let repo = PrRepo::new();
    repo.new_worktree("demo");
    answer_pr(&repo, "OPEN", false, "MERGEABLE", "APPROVED");
    fs::write(repo.fixture.bin.join("gh-merge-forget"), "").unwrap();

    let merged = repo.json_of(&["pr", "merge", "demo", "--rebase"]);

    assert_eq!(merged["method"], "rebase");
    assert_eq!(merged["merged"], true);
    assert_eq!(without_time(merged["pr"].clone()), merged_pr());
    assert_eq!(without_time(repo.listed_pr("demo")), merged_pr());
}

#[test]
fn a_merge_github_still_shows_open_is_kept_open_and_never_called_merged() {
    let repo = PrRepo::new();
    repo.new_worktree("demo");
    answer_pr(&repo, "OPEN", false, "MERGEABLE", "APPROVED");
    fs::write(repo.fixture.bin.join("gh-merge-held"), "").unwrap();
    let mut open = merged_pr();
    open["state"] = json!("open");

    let out = repo.ok(&["pr", "merge", "demo"]);

    assert_eq!(
        out,
        "demo: gh accepted the merge of PR #12 (squash), but GitHub still shows it open: a merge \
         queue or a required check may be holding it; run orca-term pr refresh demo later\n"
    );
    assert_eq!(
        without_time(repo.listed_pr("demo")),
        open,
        "the cache says open"
    );

    let held = repo.json_of(&["pr", "merge", "demo", "--merge"]);

    assert_eq!(held["merged"], false);
    assert_eq!(held["method"], "merge");
    assert_eq!(without_time(held["pr"].clone()), open);
    assert_eq!(
        merges(&repo),
        ["pr merge 12 --squash", "pr merge 12 --merge"]
    );
}

/// Makes the fake `gh` answer for the branch `demo` with pull request 12 in `state`, whose head
/// commit is `head` (none when `None`, as gh answered before #137 asked for it).
fn answer_pr_at(repo: &PrRepo, state: &str, head: Option<&str>) {
    let mut pr = json!({
        "number": 12, "title": "Fix the login", "state": state, "isDraft": false,
        "url": "https://github.com/o/r/pull/12", "statusCheckRollup": [],
    });
    if let Some(head) = head {
        pr["headRefOid"] = json!(head);
    }
    fs::write(repo.fixture.bin.join("gh-demo.json"), pr.to_string()).unwrap();
}

/// Removes the worktree `demo` without `--force`, and returns its `branch` as `--json` prints it.
fn removed_branch(repo: &PrRepo) -> Value {
    repo.json_of(&["worktree", "rm", "demo"])["branch"].clone()
}

/// Whether the primary checkout still has the branch `demo`.
fn has_demo_branch(repo: &PrRepo) -> bool {
    !repo
        .fixture
        .repo
        .git(&["branch", "--list", "demo"])
        .trim()
        .is_empty()
}

#[test]
fn rm_deletes_the_branch_of_a_squash_merged_pull_request_whose_head_is_its_tip() {
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login");
    repo.ok(&["pr", "create", "demo", "--title", "Fix the login"]);
    repo.ok(&["pr", "merge", "demo"]);
    let asked = repo.gh_calls().len();

    let out = repo.ok(&["worktree", "rm", "demo"]);

    assert_eq!(
        out.lines().last().unwrap(),
        "deleted branch demo (merged as PR #12)",
        "{out}"
    );
    assert!(!has_demo_branch(&repo), "the branch is deleted");
    assert!(!demo.exists(), "the worktree is removed");
    assert_eq!(repo.gh_calls().len(), asked, "the removal never runs gh");
}

#[test]
fn rm_says_merged_as_pr_in_json_and_keeps_the_ancestor_check_first() {
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login");
    answer_pr_at(&repo, "MERGED", Some(&repo.head(&demo)));
    repo.ok(&["pr", "refresh", "demo"]);

    assert_eq!(
        removed_branch(&repo),
        json!({"name": "demo", "status": "deleted", "reason": "merged as PR #12"})
    );
    assert!(!has_demo_branch(&repo));

    // A branch git already sees merged says so, whatever its pull request.
    repo.new_worktree("demo");
    answer_pr_at(&repo, "MERGED", Some(&repo.head(&demo)));
    repo.ok(&["pr", "refresh", "demo"]);
    assert_eq!(removed_branch(&repo)["reason"], "merged into main");
}

#[test]
fn rm_keeps_a_branch_with_a_commit_after_the_merged_head() {
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login");
    answer_pr_at(&repo, "MERGED", Some(&repo.head(&demo)));
    repo.ok(&["pr", "refresh", "demo"]);
    repo.commit(&demo, "b.txt", "One more thing");

    assert_eq!(
        removed_branch(&repo),
        json!({"name": "demo", "status": "kept", "reason": "not merged into main"})
    );
    assert!(has_demo_branch(&repo), "nothing local is lost");
}

#[test]
fn rm_keeps_the_branch_when_the_cache_has_no_head() {
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login");
    // `pr.json` exactly as an orca-term before #137 wrote it: no `head` at all.
    let admin = repo.fixture.admin_dir(&demo);
    fs::write(
        admin.join("pr.json"),
        r#"{"schema":1,"fetched_at":1793047381369,"pr":{"number":12,"title":"Fix the login","state":"merged","url":"https://github.com/o/r/pull/12","checks":"passing"},"unavailable":null}"#,
    )
    .unwrap();
    assert_eq!(
        repo.listed_pr("demo")["state"],
        "merged",
        "the old cache reads"
    );

    assert_eq!(removed_branch(&repo)["status"], "kept");
    assert!(has_demo_branch(&repo));
    assert_eq!(repo.gh_calls(), Vec::<String>::new(), "never asks gh");

    // gh answering with no head is no proof either.
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login");
    answer_pr_at(&repo, "MERGED", None);
    repo.ok(&["pr", "refresh", "demo"]);
    assert_eq!(removed_branch(&repo)["status"], "kept");
    assert!(has_demo_branch(&repo));
    assert!(!demo.exists());
}

#[test]
fn rm_keeps_the_branch_of_a_closed_unmerged_pull_request() {
    let repo = PrRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit(&demo, "a.txt", "Fix the login");
    answer_pr_at(&repo, "CLOSED", Some(&repo.head(&demo)));
    repo.ok(&["pr", "refresh", "demo"]);

    assert_eq!(
        removed_branch(&repo),
        json!({"name": "demo", "status": "kept", "reason": "not merged into main"})
    );
    assert!(has_demo_branch(&repo));
}
