---
status: accepted
---

# Turns are bounded by tree snapshots, chained under a shared ref

Neither agent supplies a turn's diff when driven one-shot, so the core makes one. Every turn,
core-driven or under takeover, is bounded by **two snapshots of the worktree's tree**, one at its
start and one at its end, taken through a throwaway copy of the index, never the real one. Each
snapshot is an **unsigned synthetic commit** whose parent is the lane's previous snapshot, with the
turn's agent as author, so the lane's turns form one chain. A turn's diff is `begin..end`, and
`git blame` over the chain answers "which turn wrote this line". The chain is held by
**`refs/orca-term/lanes/<admin-dir id>`, a ref shared by every worktree**, never by a per-worktree
ref and never by the lane's branch.

A reader will expect the per-worktree namespace, `refs/worktree/`, since
[ADR 0003](0003-lane-state-lives-in-gits-worktree-admin-dir.md) puts lane state where it dies with
the lane. We measured that it does not hold objects. Decided in
[issue #15](https://github.com/marivaldo/orca-term/issues/15), which holds the measurements
(git 2.55).

## Considered options

- **`refs/worktree/orca-term/…`** in the lane. Rejected: a `git gc` run from the primary checkout
  prunes commits reachable only from another worktree's `refs/worktree/*`, which leaves the ref
  dangling, and then `git gc` inside the lane fails with `fatal: bad object`. A reflog on the ref
  protects the chain only while its newest entry is younger than `gc.reflogExpire` (90 days), so a
  lane idle that long loses its whole review history and breaks the user's gc.
- **Tree ids in the turn files, no ref.** Rejected: nothing keeps the objects past the prune grace
  period, and there is no blame.
- **No snapshot, diff against `HEAD`.** Rejected: it merges every turn into one diff and cannot
  attribute a line to a turn.

## Consequences

- **This ref does not die with the lane**, unlike the rest of ADR 0003. `lane rm` deletes it. A
  worktree removed by hand leaves an orphan that only an explicit `lane prune` deletes, since the
  core never does a destructive write on a read path. Git reuses a freed admin-dir id, so a ref that
  does not match the tip recorded in the new lane's `lane.json` is stale, and the first begin
  snapshot of the new lane replaces it.
- The chain shows in `git log --all` and `for-each-ref` from every worktree. Default fetch and
  push refspecs do not carry it.
- `git add -A` on the copied index respects `.gitignore` and includes untracked files. It costs
  ~0.13 s on a 40k-file tree, warm, and it runs inside every takeover hook.
- Edits the person makes between turns become their own segment of the chain (the next begin
  snapshot), so they are never attributed to a turn. Edits made *during* a turn cannot be told
  apart and belong to it.
