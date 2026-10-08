---
status: accepted
---

# Lanes share no dependency tree

A reasonable reader will assume a fleet of worktrees shares its dependency trees: every comparable
tool offers it (`stablyai/orca` symlinks `worktree.sharedDirectories` and clone-copies
`.worktreeinclude`; Worktrunk reflinks, documenting 14 GB in ~20s against ~2 min for `cp -R`), APFS
clones look nearly free, and N copies of `node_modules` looks like obvious waste. We do the
opposite. Each lane installs its own, and the core offers **no mechanism** to share, clone or
hardlink a dependency tree. What it does offer is narrower: a copy list for git-ignored *files*
(secrets, local config) and a setup script that runs the repo's own install.

Decided in [issue #8](https://github.com/marivaldo/orca-term/issues/8), which holds the
measurements.

## Considered options

- **Symlink the whole tree** (Orca's `worktree.sharedDirectories`). Rejected: the first
  `npm install` in the lane **deletes the symlink** (`npm warn reify Removing non-directory
  .../node_modules`) and does a full install anyway. Before that, Node realpaths by default, so
  `require.resolve` returns paths in the source worktree and every dependency's `__dirname` escapes
  the lane.
- **APFS clone-copy / reflink.** Rejected on measurement. pnpm's `.bin/*` are not symlinks but
  `/bin/sh` shims baking an **absolute** `NODE_PATH`, so a cloned tree resolves modules out of the
  source worktree — proved by resolving, from the lane, a package planted only in the source. The
  binary still runs, so a smoke test passes and the fault is **invisible**: two lanes on branches
  with different lockfiles silently share dependencies. A cloned CPython venv is worse — 486 of
  1078 files hold the source absolute path, and after sourcing the *clone's* `activate`,
  `VIRTUAL_ENV`, `which python` and `sys.prefix` all point at the source venv. And it buys nothing:
  clone cost scales with **file count, not bytes** (40,006 files took 6.12s) against ~0.5s for a
  warm pnpm install into an empty tree.
- **Hardlink** (`cp -Rl`). Rejected: shared inodes mean a write inside a lane mutates the primary
  checkout — appending to the lane's `typescript/package.json` changed the file in the source.
- **Share only where it is known safe.** npm, yarn classic, bundler `vendor/bundle` and a
  `uv venv --relocatable` do relocate byte-clean (zero absolute references, functionally verified at
  a new path). Rejected as a default anyway: the core cannot tell which package manager, version and
  configuration a repo uses, and the ground shifts under it — pnpm's own source claims its shims
  write `NODE_PATH` relative to their own directory, while 11.13.1 measured fully absolute. A shared
  tree would work for some repos and corrupt invisibly in others, which is the worst of the two
  outcomes.

## Consequences

- **Setup runs for every lane, and disk cost is N full trees.** Accepted deliberately. On APFS this
  is smaller than it reads: pnpm's store links measured as CoW clones rather than hardlinks, so
  per-lane pnpm cost is already near-zero without help from us.
- **The copy list is for files, not trees.** `.worktreeinclude` keeps its upstream semantics —
  literal paths only, intersected with `git ls-files --others --ignored --exclude-standard`,
  symlinks skipped — and is meant for a dotenv, not a dependency directory.
- **Lane creation latency *is* install latency.** That is why setup blocks the first turn instead of
  racing it: there is no shortcut left to hide the cost behind.
- **A repo whose install is genuinely slow gets no escape hatch from us.** Its answer is its own
  package manager's shared store — pnpm ships a page for exactly this architecture — not a
  mechanism of the core's.
