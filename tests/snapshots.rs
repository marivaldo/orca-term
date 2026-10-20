//! Turn snapshots (ADR 0006, #38), driven as claude drives them (seam 1): each test runs the real
//! binary's `hook claude` in a worktree of a throwaway repository, with claude's payloads on
//! stdin (written here, or played from the recordings in `tests/fixtures/claude-hooks/`), then
//! asks real git what the snapshots hold: the turn's diff, `git blame` over the chain, what `git
//! gc` keeps, and what `worktree rm` and `worktree prune` delete.

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
mod turns;

use std::fs;

use fixture::AgentRepo;
use serde_json::{Value, json};
use turns::{SESSION, Snapped, session_end, stop, submit};

/// A fixture repository whose `main` holds `tracked.txt` and a `.gitignore` ignoring
/// `ignored.txt`.
fn repo_with_files() -> AgentRepo {
    let fx = AgentRepo::new();
    fx.repo.write("tracked.txt", "one\n");
    fx.repo.write(".gitignore", "ignored.txt\n");
    fx.repo.commit_all("files");
    fx
}

#[test]
fn a_turn_is_bounded_by_snapshots_of_the_whole_tree_leaving_the_index_alone() {
    let fx = repo_with_files();
    // Signing is on and cannot work: a snapshot that tried to sign would fail.
    fx.repo.git(&["config", "commit.gpgsign", "true"]);
    fx.repo.git(&["config", "gpg.program", "false"]);
    let wt = Snapped::new(&fx, "fix");
    let index = wt.index_bytes();

    wt.fire(&submit("p1"));
    let begin = wt.snap(1, "begin");
    assert_eq!(wt.parents(&begin), "", "the chain starts here");
    assert_eq!(
        wt.git(&["rev-parse", &format!("{begin}^{{tree}}")]),
        wt.git(&["rev-parse", "HEAD^{tree}"]),
        "nothing changed before the turn"
    );
    assert_eq!(wt.status()["turn"]["begin"], json!(begin));

    wt.write("tracked.txt", "one\ntwo\n");
    wt.write("new.txt", "fresh\n");
    wt.write("ignored.txt", "secret\n");
    let porcelain = wt.porcelain();
    let index_before_end = wt.index_bytes();
    wt.fire(&stop("p1"));

    let end = wt.snap(1, "end");
    let end_line = wt.line(1, "end");
    assert_eq!(end_line["begin"], json!(begin));
    assert_eq!(end_line["end_observed"], json!(true));
    let status = wt.status();
    assert_eq!(status["latest"]["begin"], json!(begin));
    assert_eq!(status["latest"]["end"], json!(end));
    assert_eq!(status["snapshot"], json!(end), "the chain's tip");
    assert_eq!(wt.git(&["rev-parse", &wt.chain_ref()]), end);
    assert_eq!(wt.parents(&end), begin);
    assert_eq!(
        wt.git(&["diff", "--name-status", &begin, &end]),
        "A\tnew.txt\nM\ttracked.txt",
        "the turn's edits, untracked files in, ignored files out"
    );
    assert_eq!(index_before_end, index, "the real index is never written");
    assert_eq!(wt.index_bytes(), index);
    assert_eq!(wt.porcelain(), porcelain);
    let raw = wt.git(&["cat-file", "-p", &end]);
    assert!(!raw.contains("gpgsig"), "{raw}");
    assert_eq!(wt.author(&end), "claude <claude@orca-term.invalid>");
    assert_eq!(
        wt.git(&["log", "-1", "--format=%cn <%ce>", &end]),
        "orca-term <orca-term@orca-term.invalid>"
    );
    assert_eq!(
        wt.git(&["log", "-1", "--format=%B", &end]),
        "orca-term: end of turn 1\n\nOrca-Term-Worktree: fix\nOrca-Term-Session: s-1\n\
         Orca-Term-Ends: 1\nOrca-Term-End-Observed: true"
    );
    assert_eq!(
        wt.git(&["rev-parse", "HEAD"]),
        wt.git(&["rev-parse", "main"])
    );
    assert!(!wt.admin.join("snapshot.index").exists());
    assert_eq!(wt.hook_log(), "");
}

#[test]
fn blame_over_the_chain_gives_each_line_to_its_turn_or_to_the_person() {
    let fx = AgentRepo::new();
    let wt = Snapped::new(&fx, "fix");
    wt.fire(&submit("p1"));
    wt.append("file.txt", "turn one");
    wt.fire(&stop("p1"));
    wt.append("file.txt", "person");
    wt.fire(&submit("p2"));
    wt.append("file.txt", "turn two");
    wt.fire(&stop("p2"));

    let (end1, begin2, end2) = (wt.snap(1, "end"), wt.snap(2, "begin"), wt.snap(2, "end"));
    assert_eq!(wt.parents(&end2), begin2);
    assert_eq!(wt.parents(&begin2), end1);
    assert_eq!(wt.parents(&end1), wt.snap(1, "begin"));
    let blame = wt.git(&["blame", "-l", "-s", &end2, "--", "file.txt"]);
    let by_line: Vec<(&str, &str)> = blame
        .lines()
        .map(|line| {
            let (commit, rest) = line.split_once(' ').unwrap();
            (commit, rest.rsplit_once(')').unwrap().1.trim())
        })
        .collect();
    assert_eq!(
        by_line,
        [
            (end1.as_str(), "turn one"),
            (begin2.as_str(), "person"),
            (end2.as_str(), "turn two"),
        ],
        "{blame}"
    );
    assert_eq!(wt.author(&end1), "claude <claude@orca-term.invalid>");
    assert_eq!(
        wt.author(&begin2),
        "Fixture <fixture@example.invalid>",
        "the person's edits between turns, under their git identity"
    );
    assert_eq!(wt.author(&end2), "claude <claude@orca-term.invalid>");
}

#[test]
fn an_interrupted_turn_ends_on_the_snapshot_that_begins_the_next_and_stop_is_observed() {
    let fx = AgentRepo::new();
    let wt = Snapped::new(&fx, "fix");
    // SessionStart, a prompt, Esc, another prompt, Stop, SessionEnd.
    wt.play("tui-esc-2.1.300.log");

    let first_end = wt.line(1, "end");
    assert_eq!(first_end["outcome"], "interrupted");
    assert_eq!(first_end["end_observed"], json!(false));
    let shared = wt.snap(1, "end");
    assert_eq!(wt.snap(2, "begin"), shared, "one snapshot, both halves");
    assert_eq!(wt.parents(&shared), wt.snap(1, "begin"));
    assert_eq!(wt.author(&shared), "claude <claude@orca-term.invalid>");
    let subject = wt.git(&["log", "-1", "--format=%s", &shared]);
    assert_eq!(subject, "orca-term: end of turn 1, begin of turn 2");
    assert_eq!(wt.line(2, "end")["end_observed"], json!(true));
    let tip = wt.snap(2, "end");
    assert_eq!(
        wt.git(&["rev-list", "--count", &tip]),
        "3",
        "SessionStart and SessionEnd with no open turn take none"
    );
    assert_eq!(wt.git(&["rev-parse", &wt.chain_ref()]), tip);
}

#[test]
fn session_end_closes_an_open_turn_with_its_end_not_observed() {
    let fx = AgentRepo::new();
    let wt = Snapped::new(&fx, "fix");
    wt.fire(&submit("p1"));
    wt.append("file.txt", "half done");
    wt.fire(&session_end());

    let end_line = wt.line(1, "end");
    assert_eq!(end_line["outcome"], "interrupted");
    assert_eq!(end_line["end_observed"], json!(false));
    assert_eq!(wt.status()["latest"]["end_observed"], json!(false));
    let end = wt.snap(1, "end");
    assert_eq!(
        wt.git(&["diff", "--name-only", &wt.snap(1, "begin"), &end]),
        "file.txt"
    );
    assert!(
        wt.git(&["log", "-1", "--format=%B", &end])
            .contains("Orca-Term-End-Observed: false")
    );
}

#[test]
fn a_stale_ref_is_replaced_by_a_snapshot_with_no_parent() {
    let fx = AgentRepo::new();
    let wt = Snapped::new(&fx, "fix");
    // A previous worktree with the same id left its chain; nothing is recorded for this one.
    let stale = wt.git(&["rev-parse", "main"]);
    wt.git(&["update-ref", &wt.chain_ref(), &stale]);

    wt.fire(&submit("p1"));
    let begin = wt.snap(1, "begin");
    assert_ne!(begin, stale);
    assert_eq!(wt.parents(&begin), "", "the stale chain is not continued");
    assert_eq!(wt.git(&["rev-parse", &wt.chain_ref()]), begin);
    assert_eq!(wt.hook_log(), "");
}

#[test]
fn every_snapshot_survives_gc_from_the_primary_checkout() {
    let fx = AgentRepo::new();
    let wt = Snapped::new(&fx, "fix");
    for prompt in ["p1", "p2"] {
        wt.fire(&submit(prompt));
        wt.append("file.txt", prompt);
        wt.fire(&stop(prompt));
    }
    let snapshots = [
        wt.snap(1, "begin"),
        wt.snap(1, "end"),
        wt.snap(2, "begin"),
        wt.snap(2, "end"),
    ];

    fx.repo.git(&["reflog", "expire", "--expire=now", "--all"]);
    fx.repo.git(&["gc", "--prune=now", "--quiet"]);
    for snapshot in &snapshots {
        fx.repo.git(&["cat-file", "-e", snapshot]);
    }
    wt.git(&["gc", "--quiet"]);
    assert_eq!(wt.git(&["rev-parse", &wt.chain_ref()]), snapshots[3]);
}

#[test]
fn worktree_rm_deletes_the_chain_and_prune_deletes_the_orphans_only() {
    let fx = AgentRepo::new();
    let removed = Snapped::new(&fx, "gone-by-rm");
    let kept = Snapped::new(&fx, "kept");
    let by_hand = Snapped::new(&fx, "gone-by-hand");
    for wt in [&removed, &kept, &by_hand] {
        wt.fire(&submit("p1"));
        assert!(wt.has_ref(&wt.chain_ref()));
    }

    fx.ok(&["worktree", "rm", "gone-by-rm"]);
    assert!(!kept.has_ref(&removed.chain_ref()), "rm deletes its chain");
    assert!(kept.has_ref(&kept.chain_ref()));

    fs::remove_dir_all(&by_hand.worktree).unwrap();
    assert!(
        kept.has_ref(&by_hand.chain_ref()),
        "nothing deletes it on a read path"
    );
    fx.ok(&["worktree", "ls"]);
    assert!(kept.has_ref(&by_hand.chain_ref()));
    let out = fx.ok(&["worktree", "prune"]);
    assert_eq!(
        out,
        format!(
            "pruned {}\ndeleted {}\n",
            by_hand.worktree.display(),
            by_hand.chain_ref()
        )
    );
    assert!(!kept.has_ref(&by_hand.chain_ref()));
    assert!(kept.has_ref(&kept.chain_ref()), "other chains stay");
}

#[test]
fn a_failed_snapshot_still_records_the_turn_without_its_id() {
    let fx = AgentRepo::new();
    let wt = Snapped::new(&fx, "fix");
    // No git on PATH: the snapshot cannot be taken.
    let empty = fx.repo.tmp.path().join("no-tools");
    fs::create_dir_all(&empty).unwrap();
    wt.fire_with(&submit("p1"), Some(&empty));

    let start = wt.line(1, "start");
    assert_eq!(start["begin"], Value::Null);
    let status = wt.status();
    assert_eq!(status["turn"]["number"], 1, "the turn is recorded");
    assert_eq!(status["turn"]["begin"], Value::Null);
    assert_eq!(status["snapshot"], Value::Null, "the tip did not move");
    assert!(
        wt.hook_log().contains("no snapshot was taken"),
        "{}",
        wt.hook_log()
    );
    assert!(!wt.has_ref(&wt.chain_ref()));
    let logged = wt.hook_log();
    let tool = json!({
        "hook_event_name": "PreToolUse",
        "session_id": SESSION,
        "prompt_id": "p1",
        "tool_name": "Bash",
        "tool_input": {"command": "ls"},
    });
    wt.fire_with(&tool, Some(&empty));
    assert_eq!(
        wt.hook_log(),
        logged,
        "an event that neither opens nor closes a turn runs no git"
    );
    assert_eq!(wt.status()["turn"]["activity"], "Bash: ls");

    wt.fire(&stop("p1"));
    let end_line = wt.line(1, "end");
    assert_eq!(end_line["begin"], Value::Null);
    let end = end_line["end"].as_str().unwrap();
    assert_eq!(wt.git(&["rev-parse", &wt.chain_ref()]), end);
    assert_eq!(wt.turns().len(), 2, "one start, one end");
}
