---
status: accepted
---

# The primary checkout is a worktree, as in Orca

Map #1 kept the primary checkout out of the fleet: it was not a worktree (`CONTEXT.md`), no agent
ran in it, and the sidebar showed it only in the project header
([issue #19](https://github.com/marivaldo/orca-term/issues/19)). The terminal application's design
draws it as the first item of its project, with a `primary` tag and an agent, and the driver kept
that ([#170](https://github.com/marivaldo/orca-term/issues/170),
[#173](https://github.com/marivaldo/orca-term/issues/173)).

orca-term follows Orca. In Orca's source the main worktree is an ordinary workspace row, ordered
first in its project unless the sort is manual (`orderMainWorktreeFirst` in
`src/shared/sidebar-list/section-order.ts`), and nothing in the files that read `isMainWorktree`
stops a terminal or an agent there.

- **The primary checkout is a worktree of its fleet.** It can run an agent in a holder, has a
  state, an unseen mark, turns and notes, and counts in the status bar.
- **It comes first** among its project's worktrees and carries the `primary` tag.
- **It is never removed.** `worktree rm` refuses it.
- **On the repository's default branch it has no pull request**, and says so; on any other branch
  it opens one like any worktree.
- Its state lives in git's admin directory like every worktree's (ADR 0003): for the primary
  checkout that is the common directory itself.

This record reverses map #1's "the primary checkout is never a worktree" and #19's "only in the
project header".

## Considered options

- **Keep the primary checkout out of the fleet** (#19). Rejected: the driver wants Orca's list,
  where the repository the person works in sits beside its worktrees.
- **List it as an item with no agent and no state**, opening only a shell and Neovim. Rejected: one
  item would behave unlike every other, and the core would still have to explain why.

## Consequences

- `CONTEXT.md` changes **Worktree** and **Primary checkout**.
- The fleet derived from git includes the primary checkout, `worktree ls --json` and `fleets
  --json` report it with a `primary` field, and the guard that asserted it never renders as a row
  is reversed. The slice that does it is its own issue on map #165's route (#171).
