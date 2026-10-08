---
status: accepted
---

# Lane state lives in git's worktree admin directory

A reasonable reader will expect a fleet tool to keep its state in the user's home, in a database:
Orca keeps SQLite plus files under the home directory, with migrations from a legacy JSON format.
We do the opposite. Each lane's state is plain files inside **git's own per-worktree admin
directory**, `$(git rev-parse --git-path orca-term)` → `.git/worktrees/<id>/orca-term/`, and the
only repo-wide state, the uncommitted config override, is `.git/orca-term.yaml` in the common dir.
The reason is that the fleet is derived from git and never from a registry of ours, so our state
must live and die on git's schedule. A store anywhere else outlives the lane whenever someone
removes a worktree behind the core's back, which leaves us collecting orphans.

Decided in [issue #9](https://github.com/marivaldo/orca-term/issues/9), which holds the
measurements.

## Considered options

- **`~/.local/state/orca-term/`, keyed by repo and lane path** (Orca's shape). Rejected: the key
  breaks when the repo moves, and `rm -rf <lane>` + `git worktree prune` leaves state behind that
  only a GC or a registry could reclaim. A GC would be a destructive write on a read path, and a
  registry would rival `git worktree list`.
- **A file inside the worktree** (`<lane>/.orca-term/`). Rejected: visible to the agent and to
  `git status`, and destroyed by `git clean -fdx`.
- **SQLite**, per lane or per repo. Rejected: each lane has at most one writer at a time (one turn
  at a time, and takeover is exclusive), so transactions buy nothing that an atomic rename and a
  per-lane `flock` do not. A per-repo database is the parallel registry again. Orca needs one
  because a daemon aggregates for it, and the core has no daemon.

## Consequences

- **State dies with the lane.** `git worktree remove` and `prune` delete the admin directory,
  turn records included, even when `lane rm` keeps an unmerged branch. The one exception is the
  ref that holds a lane's turn snapshots, which git would prune from a per-worktree namespace
  ([ADR 0006](0006-turns-are-bounded-by-tree-snapshots-under-a-shared-ref.md)). That is accepted, because
  inline review sends comments back to the lane's agent and a removed lane has none.
- **Measured on git 2.55**: the admin directory survives `worktree move` (its id is unchanged) and
  `gc`, and its id disambiguates two lanes that share a basename (`lane-c`, `lane-c1`).
  `claude -p --resume <uuid>` also works from the moved path, so a moved lane keeps its session.
- **The primary checkout is not a lane.** Inside it, `--git-path` resolves to the common dir, so
  per-lane and repo-wide state could not coexist there.
- **Volatile facts are written but never trusted**: a turn's pid and start time, the takeover
  holder, and the `opencode serve` address are checked for liveness on read. A turn whose process
  is gone reads as interrupted, and nothing is rewritten on the read path.
