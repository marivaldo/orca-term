---
status: accepted
---

# Multi-repo is in scope, through a project list kept outside every repository

Spec [#25](https://github.com/marivaldo/orca-term/issues/25) put "multi-repo workspaces and
cross-repo aggregation" out of scope: orca-term knew only the repository of the current directory.
The driver now asks for Orca's model, where the sidebar shows every repository the person works
on, each with its worktrees. This record reverses that line of #25's out of scope.

orca-term follows Orca. Orca keeps a list of repositories, added by the person (`stablyai/orca`,
`src/renderer/src/store/repos/repo-add-actions.ts`), and groups the worktrees under each one. In
orca-term, such a repository is a **project** (`CONTEXT.md`): its primary checkout's path, on a
list kept in the order projects were added.

- The list lives in orca-term's state directory, `$XDG_STATE_HOME/orca-term/projects.json` (or
  `~/.local/state/orca-term/projects.json`), with a `schema` field like every other state file.
  It belongs to the person, not to any repository, so it cannot live in a git common dir as the
  rest of orca-term's state does (ADR 0003).
- `project add [<path>]` resolves a linked worktree or a subdirectory to its primary checkout,
  adds nothing the second time, and refuses a path outside every repository (`not_a_repository`).
  `project ls` marks a project whose directory is gone instead of dropping it. `project rm <path
  or name>` only takes it off the list, and refuses a name several projects share
  (`ambiguous_project`).
- `fleets --json` reads every project's fleet in one call, each as `worktree ls --json` prints
  it; a gone or broken project carries its error, and the others are still read.
- A fleet stays one repository's worktrees (`CONTEXT.md`, **Fleet**): the project list groups
  fleets, it does not merge them. Every worktree command still acts on one repository.

Decided in [issue #149](https://github.com/marivaldo/orca-term/issues/149); the sidebar that
groups worktrees by project is [issue #150](https://github.com/marivaldo/orca-term/issues/150).

## Considered options

- **Keep one repository per Neovim**, as #25 decided. Rejected: the driver works across several
  repositories and wants them in one sidebar, as Orca shows them.
- **Find repositories by scanning directories.** Rejected: Orca lets the person add repositories,
  and a scan would guess wrong and cost time on every read.
- **Keep the list in the Neovim client.** Rejected: the core is the only writer of state, and the
  shell can then list, add and remove projects too.
- **Drop a project whose directory is gone.** Rejected: a disk unmounted for a while would lose
  the project; it is marked gone, and the person removes it.

## Consequences

- `PROJECTS_SCHEMA` joins the state schemas, with its sample in
  `tests/fixtures/schema-1/state/orca-term/projects.json`.
- The launcher (`orca-term` with no arguments and `orca-term open`, #148) adds the repository of
  the current directory through `ops::project_add::add_repository_of` before it opens Neovim
  ([issue #152](https://github.com/marivaldo/orca-term/issues/152)). A project list that cannot
  be changed never stops the launch: nothing is added, and the sidebar shows the problem when it
  reads the same list through `fleets`.

  > **Changed by [#193](https://github.com/marivaldo/orca-term/issues/193)** (2026-11-11): the
  > application, opened by `orca-term app` or by `orca-term` with no arguments, adds the
  > repository as it opens (`ops::project_add::add_current_repository`), never failing for it, and
  > `orca-term open` is removed.
- The Neovim client registers with every project's primary checkout for live updates (#150).
