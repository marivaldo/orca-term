//! `orca-term worktree rebase` and `orca-term agent resolve`, driven as a black box (seam 1) with
//! real git, a bare repository as `origin` that gets a new commit on `main` from another clone, and
//! the fake `claude` of `tests/agent.rs` on its isolated PATH (#141): a clean rebase, a branch
//! already up to date, a conflict left in progress with its files named, the abort, each refusal,
//! a repository with no default branch on `origin`, and the prompt typed into the agent.

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
#[path = "support/turns.rs"]
#[expect(
    dead_code,
    reason = "the turn helpers are shared between suites, and this one leaves part of them unused"
)]
mod turns;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Output;

use fixture::{AgentRepo, wait_until};
use serde_json::{Value, json};
use turns::{Snapped, submit};

/// The file both sides change in the conflict tests, as committed on `main`.
const SHARED: &str = "shared.txt";

/// A repository with `shared.txt` on `main`, a bare repository as `origin` holding it, another
/// clone of `origin` to move its `main` with, and worktrees placed under the fixture's own
/// directory.
struct RebaseRepo {
    fixture: AgentRepo,
    /// Another clone of `origin`, where the base gets new commits.
    upstream: PathBuf,
    base: PathBuf,
}

impl RebaseRepo {
    fn new() -> Self {
        let fixture = AgentRepo::new();
        fixture.repo.write(SHARED, "one\n");
        fixture.repo.commit_all("shared");
        let tmp = fixture.repo.tmp.path().canonicalize().unwrap();
        let origin = tmp.join("origin.git");
        let origin = origin.to_str().unwrap();
        fixture.repo.git(&["init", "--quiet", "--bare", origin]);
        fixture.repo.git(&["remote", "add", "origin", origin]);
        fixture.repo.git(&["push", "--quiet", "origin", "main"]);
        let upstream = tmp.join("upstream");
        fixture
            .repo
            .git(&["clone", "--quiet", origin, upstream.to_str().unwrap()]);
        let base = tmp.join("worktrees");
        fs::write(
            fixture.repo.root.join(".git/orca-term.yaml"),
            format!("base: {}\n", base.display()),
        )
        .unwrap();
        Self {
            fixture,
            upstream,
            base,
        }
    }

    /// Creates the worktree `name` and returns its path.
    fn new_worktree(&self, name: &str) -> PathBuf {
        self.fixture.ok(&["worktree", "new", name]);
        self.base.join("repo").join(name).canonicalize().unwrap()
    }

    /// Runs git in `dir` and returns its stdout, trimmed.
    fn git_in(&self, dir: &Path, args: &[&str]) -> String {
        let mut all = vec!["-C", dir.to_str().unwrap()];
        all.extend_from_slice(args);
        self.fixture.repo.git(&all).trim().to_owned()
    }

    /// Writes `contents` to `file` in `dir` and commits it with `message`.
    fn commit_in(&self, dir: &Path, file: &str, contents: &str, message: &str) {
        fs::write(dir.join(file), contents).unwrap();
        self.git_in(dir, &["add", file]);
        self.git_in(dir, &["commit", "--quiet", "-m", message]);
    }

    /// Gives `origin`'s `main` a new commit writing `contents` to `file`, from the other clone;
    /// the repository's own `main` and `origin/main` do not move. Returns the new commit.
    fn advance_origin(&self, file: &str, contents: &str) -> String {
        self.commit_in(&self.upstream, file, contents, "upstream change");
        self.git_in(&self.upstream, &["push", "--quiet", "origin", "main"]);
        self.git_in(&self.upstream, &["rev-parse", "HEAD"])
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

    /// The rebase's temporary refs left in the repository: there must never be any.
    fn temp_refs(&self) -> String {
        self.fixture.repo.git(&[
            "for-each-ref",
            "--format=%(refname)",
            "refs/orca-term/rebase/",
        ])
    }

    /// Makes `demo` and `origin` change `shared.txt` each their own way, then rebases `demo`,
    /// which stops on the conflict. Returns the worktree and its commit before the rebase.
    fn conflicted(&self) -> (PathBuf, String) {
        let demo = self.new_worktree("demo");
        self.commit_in(&demo, SHARED, "mine\n", "mine");
        let before = self.git_in(&demo, &["rev-parse", "HEAD"]);
        self.advance_origin(SHARED, "theirs\n");
        let out = self.run(&["worktree", "rebase", "demo"]);
        assert_eq!(out.status.code(), Some(19), "{out:?}");
        (demo, before)
    }
}

/// Makes the fake claude, once started, take its terminal raw and record every byte typed into it,
/// untouched by the terminal's line discipline, until it is stopped.
fn record_typing(fx: &AgentRepo) {
    fx.write_claude(
        "2.1.296 (Claude Code)",
        "/bin/stty raw -echo\necho \"fake claude recording\"\nexec /bin/cat > \"$log.stdin\"",
    );
}

/// Every byte typed into the recording fake claude so far.
fn typed(fx: &AgentRepo) -> Vec<u8> {
    fs::read(fx.log.with_extension("stdin")).unwrap_or_default()
}

#[test]
fn a_clean_rebase_replays_the_branch_onto_origin_and_leaves_no_temp_ref() {
    let repo = RebaseRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit_in(&demo, "a.txt", "a\n", "Add a");
    repo.commit_in(&demo, "b.txt", "b\n", "Add b");
    let local_main = repo.git_in(&repo.fixture.repo.root, &["rev-parse", "main"]);
    let upstream = repo.advance_origin("c.txt", "c\n");

    let text = repo.ok(&["worktree", "rebase", "demo"]);

    assert_eq!(
        text,
        "demo: rebased onto origin/main (2 commits replayed)\n"
    );
    assert_eq!(repo.git_in(&demo, &["rev-parse", "HEAD~2"]), upstream);
    assert_eq!(
        repo.git_in(&demo, &["log", "--format=%s", "-3"]),
        "Add b\nAdd a\nupstream change"
    );
    assert_eq!(repo.temp_refs(), "", "the temporary ref is deleted");
    assert_eq!(
        repo.git_in(&repo.fixture.repo.root, &["rev-parse", "main"]),
        local_main,
        "the local main is never touched"
    );
    assert_eq!(repo.fixture.listed("demo")["operation"], Value::Null);
}

#[test]
fn a_branch_holding_the_base_is_already_up_to_date_and_the_fleet_names_the_base() {
    let repo = RebaseRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit_in(&demo, "a.txt", "a\n", "Add a");
    let head = repo.git_in(&demo, &["rev-parse", "HEAD"]);

    let rebased = repo.json_of(&["worktree", "rebase", "demo"]);

    assert_eq!(
        rebased,
        json!({
            "contract": 1,
            "version": rebased["version"],
            "name": "demo",
            "path": demo.to_str().unwrap(),
            "base": "origin/main",
            "outcome": "up_to_date",
            "replayed": 0,
        })
    );
    assert_eq!(
        repo.ok(&["worktree", "rebase", "demo"]),
        "demo: already up to date with origin/main\n"
    );
    assert_eq!(repo.git_in(&demo, &["rev-parse", "HEAD"]), head);
    assert_eq!(repo.temp_refs(), "");
    let fleet: Value = serde_json::from_str(&repo.ok(&["worktree", "ls", "--json"])).unwrap();
    assert_eq!(fleet["remote_base"], "origin/main");

    // Once origin moves on, the JSON says what was replayed.
    repo.advance_origin("c.txt", "c\n");
    let rebased = repo.json_of(&["worktree", "rebase", "demo"]);
    assert_eq!(rebased["outcome"], "rebased");
    assert_eq!(rebased["replayed"], 1);
}

#[test]
fn a_conflict_stays_in_progress_naming_its_files_and_abort_puts_the_branch_back() {
    let repo = RebaseRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit_in(&demo, SHARED, "mine\n", "mine");
    let before = repo.git_in(&demo, &["rev-parse", "HEAD"]);
    repo.advance_origin(SHARED, "theirs\n");

    let error = repo.error_of(&["worktree", "rebase", "demo"], 19);

    assert_eq!(error["kind"], "rebase_conflict");
    assert_eq!(error["files"], json!([SHARED]));
    assert_eq!(
        error["message"],
        "the rebase of worktree demo onto origin/main stopped on conflicts in shared.txt; it \
         stays in progress: send them to the agent or abort the rebase, from the worktree menu or \
         with `orca-term agent resolve demo` or `orca-term worktree rebase demo --abort`"
    );
    assert_eq!(
        repo.temp_refs(),
        "",
        "the temporary ref is deleted on conflict too"
    );
    assert_eq!(repo.fixture.listed("demo")["operation"], "rebase");
    let detail = repo.ok(&["worktree", "detail", "demo"]);
    assert!(detail.contains("rebase in progress (1/1)"), "{detail}");

    // A second rebase is refused while this one is in progress.
    let again = repo.error_of(&["worktree", "rebase", "demo"], 17);
    assert_eq!(again["kind"], "operation_in_progress");
    assert!(
        again["message"]
            .as_str()
            .unwrap()
            .starts_with("worktree demo has a rebase in progress already"),
        "{again}"
    );

    let text = repo.ok(&["worktree", "rebase", "demo", "--abort"]);

    assert_eq!(
        text,
        "demo: aborted the rebase; the branch is back where it was\n"
    );
    assert_eq!(repo.git_in(&demo, &["rev-parse", "HEAD"]), before);
    assert_eq!(repo.fixture.listed("demo")["operation"], Value::Null);
    assert_eq!(fs::read_to_string(demo.join(SHARED)).unwrap(), "mine\n");

    let none = repo.error_of(&["worktree", "rebase", "demo", "--abort"], 20);
    assert_eq!(none["kind"], "no_rebase");
    assert_eq!(
        none["message"],
        "worktree demo has no rebase in progress to abort"
    );
}

#[test]
fn abort_answers_json() {
    let repo = RebaseRepo::new();
    let (demo, before) = repo.conflicted();

    let aborted = repo.json_of(&["worktree", "rebase", "demo", "--abort"]);

    assert_eq!(aborted["name"], "demo");
    assert_eq!(aborted["path"], demo.to_str().unwrap());
    assert_eq!(aborted["aborted"], true);
    assert_eq!(repo.git_in(&demo, &["rev-parse", "HEAD"]), before);
}

#[test]
fn changes_not_committed_are_refused_as_dirty_naming_them_and_nothing_is_fetched() {
    let repo = RebaseRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit_in(&demo, "a.txt", "a\n", "Add a");
    let head = repo.git_in(&demo, &["rev-parse", "HEAD"]);
    repo.advance_origin("c.txt", "c\n");
    fs::write(demo.join("a.txt"), "changed\n").unwrap();
    fs::write(demo.join("new.txt"), "new\n").unwrap();

    let error = repo.error_of(&["worktree", "rebase", "demo"], 6);

    assert_eq!(error["kind"], "dirty");
    assert_eq!(error["files"], json!(["a.txt", "new.txt"]));
    assert_eq!(
        error["message"],
        format!(
            "worktree demo at {} has changes not committed (a.txt, new.txt); commit or stash \
             them first (nothing was rebased)",
            demo.display()
        )
    );
    assert_eq!(repo.git_in(&demo, &["rev-parse", "HEAD"]), head);
    assert_eq!(
        repo.git_in(&demo, &["stash", "list"]),
        "",
        "nothing is stashed"
    );
    assert_eq!(repo.temp_refs(), "");
}

#[test]
fn a_merge_in_progress_is_refused() {
    let repo = RebaseRepo::new();
    let demo = repo.new_worktree("demo");
    repo.commit_in(&demo, SHARED, "mine\n", "mine");
    repo.git_in(
        &repo.fixture.repo.root,
        &["switch", "--quiet", "-c", "other"],
    );
    repo.commit_in(&repo.fixture.repo.root, SHARED, "other\n", "other");
    // The merge stops on the conflict, and stays in progress.
    repo.fixture
        .repo
        .git_failing(&["-C", demo.to_str().unwrap(), "merge", "--quiet", "other"]);
    assert_eq!(repo.fixture.listed("demo")["operation"], "merge");

    let error = repo.error_of(&["worktree", "rebase", "demo"], 17);

    assert_eq!(error["kind"], "operation_in_progress");
    assert_eq!(
        error["message"],
        "worktree demo has a merge in progress; finish or abort it first (nothing was rebased)"
    );
}

#[test]
fn an_agent_in_a_turn_is_refused_as_busy() {
    let repo = RebaseRepo::new();
    let demo = repo.new_worktree("demo");
    let admin = repo.fixture.admin_dir(&demo);
    let snapped = Snapped {
        fx: &repo.fixture,
        worktree: demo.clone(),
        admin,
    };
    repo.ok(&["agent", "start", "demo"]);
    repo.fixture.wait_for_output("demo", "fake claude ready");
    snapped.fire(&submit("p1"));
    assert_eq!(repo.fixture.listed("demo")["state"], "working");
    repo.advance_origin("c.txt", "c\n");

    let error = repo.error_of(&["worktree", "rebase", "demo"], 3);

    assert_eq!(error["kind"], "busy");
    assert_eq!(
        error["message"],
        format!(
            "worktree demo at {} is busy: its agent is in a turn, working or waiting for you; \
             rebase once the turn ends, or stop it first with `orca-term agent stop demo` \
             (nothing was rebased)",
            demo.display()
        )
    );
    assert_eq!(repo.temp_refs(), "");
}

#[test]
fn a_repository_with_no_default_branch_on_origin_is_refused_and_never_rebases_onto_main() {
    let fixture = AgentRepo::new();
    let base = fixture
        .repo
        .tmp
        .path()
        .canonicalize()
        .unwrap()
        .join("worktrees");
    fs::write(
        fixture.repo.root.join(".git/orca-term.yaml"),
        format!("base: {}\n", base.display()),
    )
    .unwrap();
    fixture.ok(&["worktree", "new", "demo"]);
    let demo = base.join("repo/demo");
    // The local main moves on: a rebase must still not go onto it.
    fixture.repo.write("local.txt", "local\n");
    fixture.repo.commit_all("local");
    let head = fixture
        .repo
        .git(&["-C", demo.to_str().unwrap(), "rev-parse", "HEAD"]);

    let out = fixture.run(&["worktree", "rebase", "demo", "--json"]);

    assert_eq!(out.status.code(), Some(18), "{out:?}");
    let doc: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(doc["error"]["kind"], "no_remote_base");
    assert_eq!(
        doc["error"]["message"],
        "worktree demo cannot be rebased: this repository has no default branch on an `origin` \
         remote to rebase onto, and a rebase never goes onto a local branch (nothing was rebased)"
    );
    assert_eq!(
        fixture
            .repo
            .git(&["-C", demo.to_str().unwrap(), "rev-parse", "HEAD"]),
        head
    );
    let fleet: Value = serde_json::from_str(&fixture.ok(&["worktree", "ls", "--json"])).unwrap();
    assert_eq!(fleet["remote_base"], Value::Null);
}

#[test]
fn resolve_types_one_prompt_naming_the_conflicts_into_the_agent() {
    let repo = RebaseRepo::new();
    let (demo, _) = repo.conflicted();
    record_typing(&repo.fixture);
    repo.ok(&["agent", "start", "demo"]);
    repo.fixture
        .wait_for_output("demo", "fake claude recording");

    let sent = repo.json_of(&["agent", "resolve", "demo"]);

    let prompt = "A rebase of this worktree's branch stopped on conflicts. Resolve them.\n\
                  \n\
                  Files in conflict:\n\
                  - shared.txt\n\
                  \n\
                  1. Start from `git status`.\n\
                  2. Resolve each file, keeping the intent of both sides; do not take ours or \
                  theirs wholesale.\n\
                  3. `git add` each file once it is resolved.\n\
                  4. Run `GIT_EDITOR=true git rebase --continue`. If it stops on new conflicts, \
                  resolve them the same way, until the rebase is done.\n\
                  \n\
                  Never run `git rebase --abort`, `git reset`, `git stash` or `git push`, and \
                  make no other commits. When you are done, say how you resolved each file.";
    assert_eq!(sent["prompt"], prompt);
    assert_eq!(sent["files"], json!([SHARED]));
    assert_eq!(sent["name"], "demo");
    assert_eq!(sent["path"], demo.to_str().unwrap());
    let expected = [b"\x1b[200~", prompt.as_bytes(), b"\x1b[201~", b"\r"].concat();
    wait_until("the prompt reaching the agent", || {
        typed(&repo.fixture) == expected
    });
    assert_eq!(
        repo.fixture.listed("demo")["operation"],
        "rebase",
        "the rebase stays in progress"
    );
}

#[test]
fn resolve_prints_the_prompt_as_text() {
    let repo = RebaseRepo::new();
    repo.conflicted();
    repo.ok(&["agent", "start", "demo"]);
    repo.fixture.wait_for_output("demo", "fake claude ready");

    let text = repo.ok(&["agent", "resolve", "demo"]);

    assert!(
        text.starts_with("A rebase of this worktree's branch stopped on conflicts."),
        "{text}"
    );
    assert!(
        text.ends_with("\n\ndemo: sent the agent the conflicts to resolve\n"),
        "{text}"
    );
}

#[test]
fn resolve_is_refused_with_no_rebase_or_no_running_agent() {
    let repo = RebaseRepo::new();
    let demo = repo.new_worktree("demo");
    let none = repo.error_of(&["agent", "resolve", "demo"], 20);
    assert_eq!(none["kind"], "no_rebase");
    assert_eq!(
        none["message"],
        "worktree demo has no rebase in progress, so there are no conflicts to resolve"
    );

    repo.commit_in(&demo, SHARED, "mine\n", "mine");
    repo.advance_origin(SHARED, "theirs\n");
    repo.run(&["worktree", "rebase", "demo"]);

    let stopped = repo.error_of(&["agent", "resolve", "demo"], 7);

    assert_eq!(stopped["kind"], "no_holder");
    assert_eq!(
        stopped["message"],
        format!(
            "worktree demo at {} has no running agent to resolve the conflicts; start it with \
             `orca-term agent start demo`, then try again (the rebase stays in progress)",
            demo.display()
        )
    );
    assert_eq!(repo.fixture.starts(), 0, "no agent was started");
}
