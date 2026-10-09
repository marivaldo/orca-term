# Architecture

This file is the map of the `orca-term` core: where each concept lives and the rules the code keeps.
Read it before changing the code; update it when a module moves. The words follow the glossary in
`CONTEXT.md`, and the decision behind the shape is ADR 0010 (`docs/adr/`).

## Bird's-eye view

`orca-term` gives a person a fleet of git worktrees, each on its own branch, for parallel coding
agents. The core is a Rust command-line program: each command reads the repository through git and
the file system, decides what to do with pure rules, does it, and prints the result as text for a
person or as JSON for the Neovim client.

The core is a library (`src/lib.rs`) with a thin binary (`src/main.rs`). Inside the library, every
module sits in one of five layers, and code may use only its own layer and the layers below it:

```text
main      src/main.rs, src/lib.rs     parse the arguments, run, turn an error into an exit code
edge      src/cli.rs, src/output*     the command-line surface and everything printed
ops       src/ops*                    one module per command
adapters  src/adapters*               the only code that touches git, files and the environment
domain    src/domain*                 pure rules: data in, data or a decision out
```

A command goes down and comes back up: `cli` parses it and calls its ops module, the ops module
asks the adapters for facts, hands them to the domain for a decision, acts on the decision through
the adapters, and returns a domain value that `output` prints.

## Code map

### main

- `main.rs`: parses the arguments with clap, calls `orca_term::run`, prints an error and exits 1.
- `lib.rs`: declares the layer modules; exposes `Cli`, `run` and `output::error` to the binary.

### edge

- `cli.rs`: the clap types for every command and flag, and the dispatch to `ops`.
- `output.rs`: the only writer to stdout and stderr; renders the fleet table and the reports of
  `worktree new`, `rm` and `prune`.
- `output/json.rs`: the JSON documents of `--json`, built as views from domain values, with the
  `contract` and `version` envelope.

### ops

- `ops.rs`: what several commands share: reading the fleet and each worktree's state.
- `ops/worktree_new.rs`: `worktree new`: check the name, resolve the config, add git's worktree,
  copy the copy list, write the state file.
- `ops/worktree_ls.rs`: `worktree ls`: the fleet of the current repository.
- `ops/worktree_rm.rs`: `worktree rm`: find the target, decide the branch's fate, remove, delete
  or keep the branch.
- `ops/worktree_prune.rs`: `worktree prune`: `git worktree prune` and what it cleaned.

### adapters

- `adapters/git.rs`: every git process, one function per question (`worktree_list`,
  `default_branch`, `is_merged`, ...) or change (`worktree_add`, `delete_branch`, ...).
- `adapters/fs.rs`: reading and atomically writing files, plain byte copies, creating
  directories, resolving paths through symlinks.
- `adapters/env.rs`: the current directory, `HOME` and `XDG_CONFIG_HOME`.

### domain

- `domain/worktree.rs`: a worktree's name rules and path, which worktree a `rm` target means,
  and the branch's fate on removal.
- `domain/branch.rs`: a branch's name, the start point a new branch is created from, and git's
  rules for a valid branch name, mirrored without running git.
- `domain/fleet.rs`: the fleet derived from git's list: the primary checkout apart, names,
  gone worktrees, what a prune cleaned.
- `domain/porcelain.rs`: parsing `git worktree list --porcelain -z`.
- `domain/config.rs`: `orca-term.yaml` parsing, the precedence of the config files, `~` and
  relative path expansion.
- `domain/state.rs`: a worktree's state and why it is broken, its admin directory, and the
  `worktree.json` schema rules and bytes.
- `domain/include.rs`: the copy list: which paths `.worktreeinclude` makes copyable, and the
  rule that a path never escapes the worktree.
- `domain/contract.rs`: the `contract` and `version` integers every `--json` output carries.

### Domain types

Values that would otherwise travel as a bare `String`, `PathBuf` or `bool` get a type of their
own, so a mix-up does not compile and a value is checked once, where it enters ("parse, don't
validate"):

- `WorktreeName` (`domain/worktree.rs`): a name `worktree new` accepts. It can only be built by
  parsing (`FromStr`), which clap does on the argument, so an invalid name is refused before
  anything runs, with a `WorktreeNameError` naming the rule it breaks.
- `Target` (`domain/worktree.rs`): what `worktree rm` is given, a `Name` or a `Path` (it contains
  `/` or is `.` or `..`), parsed by clap.
- `Removal` (`domain/worktree.rs`): `Safe` or `Forced`, the `--force` flag from the edge down to
  `git worktree remove`.
- `BranchOutcome` and `KeptReason` (`domain/worktree.rs`): what `worktree rm` did with the
  branch, and why it kept it.
- `Branch` and `StartPoint` (`domain/branch.rs`): a local branch's name, and the default branch a
  new worktree starts from, `Local` or `Remote` (on `origin`). `BranchNameError` names the git
  rule a branch name breaks.
- `PrimaryCheckout` (`domain/fleet.rs`): the primary checkout's path and branch. The git adapter
  and the config sources take it where they need the primary checkout, so they cannot be handed a
  worktree's path by mistake.
- `State` and `BrokenReason` (`domain/state.rs`): `NoAgent`, or `Broken` because the directory
  is missing, git marks it prunable, or the state file cannot be read.
- `AdminDir` (`domain/state.rs`): the directory holding a worktree's state, and its
  `worktree.json`.

## Invariants

- **The domain is pure.** Nothing in `src/domain/` touches the file system, starts a process,
  reads the environment or prints. It takes strings, bytes and parsed values, and returns values,
  decisions or errors. That is why it is unit-tested without a repository.
- **Only adapters touch the outside world.** Only `adapters/git.rs` starts git, only
  `adapters/fs.rs` reads or writes files, and only `adapters/env.rs` reads the environment.
- **Only `output` prints**, and only `output/json.rs` knows the JSON shape of the contract.
  Domain types never derive `Serialize` for output.
- **Each command is one ops module**: a short sequence of adapter and domain calls, readable top
  to bottom. Shared steps go in `ops.rs`.
- **Dependencies point down**: main > edge > ops > adapters > domain. A module uses its own layer
  or a lower one, through `crate::` paths, never a higher one.
- **Nothing writes on a read path**: `worktree ls` only reads.
- **Errors** are `anyhow` errors with context, written for a person. Domain parsers return small
  `thiserror` enums (`WorktreeNameError`, `BranchNameError`) whose messages a person can read.
  Typed errors for failures the client must tell apart, with a JSON `error.kind`, arrive with
  their first case (#29).

## Guards

Each invariant above is checked, not just written down (see `docs/harness.md`):

- Clippy `disallowed-methods` and `disallowed-types` (`clippy.toml`) reject processes, file
  access and environment reads everywhere; each adapter opts out with a module-level
  `#![expect(..., reason = "...")]`.
- `cargo xtask layers` rejects an upward `crate::` reference, a file whose first line is not a
  `//!` naming its layer (`//! Domain: ...`, `//! Adapter: ...`, `//! Ops: ...`, `//! Edge: ...`,
  `//! Main: ...`), and the Clippy opt-out outside adapters. Its table of modules and layers is
  `MODULES` in `xtask/src/layers.rs`; a new top-level module is added there on purpose.
- Clippy `print_stdout` and `print_stderr` keep printing in `output`.

## Tests

- **Seam 1, black box**: `tests/worktree_*.rs` run the binary against throwaway git repositories
  built by `tests/support/mod.rs`, and check what a person or the client sees.
- **Unit tests** sit next to the pure code they test, in a `#[cfg(test)] mod tests` at the end of
  each domain and edge module. Adapters keep the few tests that need a real directory.
- **Lints** have their own unit tests in `xtask/src/`.
