# Architecture

This file is the map of `orca-term`, its core and its Neovim client: where each concept lives and the
rules the code keeps.
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
adapters  src/adapters*               the only code that touches git, files, processes and the environment
domain    src/domain*                 pure rules: data in, data or a decision out
```

A command goes down and comes back up: `cli` parses it and calls its ops module, the ops module
asks the adapters for facts, hands them to the domain for a decision, acts on the decision through
the adapters, and returns a domain value that `output` prints.

## Code map

### main

- `main.rs`: parses the arguments with clap, calls `orca_term::run` and exits with the code it
  returns, or hands the error to `output::error`, which prints it and returns its exit code.
- `lib.rs`: declares the layer modules; exposes `Cli`, `run`, `output::error` and
  `output::Format` to the binary.

### edge

- `cli.rs`: the clap types for every command and flag, the dispatch to `ops`, and the exit code
  of a command that did its job (1 when a setup failed).
- `output.rs`: the only writer to stdout and stderr; renders the fleet table, the reports of
  `worktree new`, `setup`, `rm` and `prune`, and errors (as text on stderr, or as JSON on stdout
  with `--json`). `Format` is text or JSON. Before a command that may run a setup starts, `cli`
  has `output` say on stderr what it starts (`creating worktree fix...`); `live_echo` is the
  writer the setup's output is then echoed to while it runs (stderr, or nowhere with `--json`),
  handed by `cli` to the ops module.
- `output/json.rs`: the JSON documents of `--json` (the fleet, `worktree new`, `worktree setup`
  and errors), built as views from domain values, with the `contract` and `version` envelope.

### ops

- `ops.rs`: what several commands share: reading the fleet and deriving each worktree's state,
  finding the worktree a command names, taking a worktree's lock, writing `worktree.json`,
  reading the config files, and running a worktree's setup.
- `ops/worktree_new.rs`: `worktree new`: check the name, resolve the config, add git's worktree,
  lock it, copy the copy list, run the setup.
- `ops/worktree_setup.rs`: `worktree setup`: find the target, lock it (or fail as `busy`), run
  the setup again.
- `ops/worktree_ls.rs`: `worktree ls`: the fleet of the current repository.
- `ops/worktree_rm.rs`: `worktree rm`: find the target, lock it (or fail as `busy`, even with
  `--force`), decide the branch's fate, remove, delete or keep the branch.
- `ops/worktree_prune.rs`: `worktree prune`: `git worktree prune` and what it cleaned.

### adapters

- `adapters/git.rs`: every git process, one function per question (`worktree_list`,
  `default_branch`, `is_merged`, ...) or change (`worktree_add`, `delete_branch`, ...).
- `adapters/fs.rs`: reading and atomically writing files, plain byte copies, creating
  directories, resolving paths through symlinks, and a worktree's lock: `WorktreeLock`, taken
  without waiting, and the read-only probe `ls` uses.
- `adapters/script.rs`: the setup script's process, `bash -e -c <script>` in the worktree, its
  stdout and stderr merged through one pipe and copied to the setup log and the live echo.
- `adapters/env.rs`: the current directory, `HOME` and `XDG_CONFIG_HOME`.

### domain

- `domain/worktree.rs`: a worktree's name rules and path, which worktree a `rm` target means,
  and the branch's fate on removal.
- `domain/branch.rs`: a branch's name, the start point a new branch is created from, and git's
  rules for a valid branch name, mirrored without running git.
- `domain/fleet.rs`: the fleet derived from git's list: the primary checkout apart, names,
  gone worktrees, what a prune cleaned.
- `domain/porcelain.rs`: parsing `git worktree list --porcelain -z`.
- `domain/config.rs`: `orca-term.yaml` parsing (`base`, `scripts.setup`), the precedence of the
  config files, which checkout supplies the committed file (the primary checkout for `base`, the
  worktree for `scripts.setup`), `~` and relative path expansion.
- `domain/state.rs`: a worktree's state and why it is broken or not ready, derived from
  `worktree.json`, the lock and the setup log; its admin directory; and the `worktree.json`
  schema rules and bytes.
- `domain/setup.rs`: the setup: how it ended, what `worktree.json` records, the variables its
  script gets, the last line of its log, and the reports of the commands that run it.
- `domain/error.rs`: the typed errors a client tells apart, with their `kind` and exit code.
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
- `State` (`domain/state.rs`): `NoAgent`, `SettingUp`, `NotReady(NotReadyReason)` or
  `Broken(BrokenReason)`. `NotReadyReason` is `Interrupted` (a setup recorded as running whose
  lock nobody holds) or `Failed` with its `Exit` and the last line of its log. `BrokenReason` says
  the directory is missing, git marks it prunable, or the state file cannot be read. Broken wins
  over every other state.
- `AdminDir` (`domain/state.rs`): the directory holding a worktree's state: its `worktree.json`,
  its `setup.log` and its `lock`.
- `LockStatus` (`domain/state.rs`): `Held` or `Free`, what the read-only probe of a worktree's
  lock found.
- `SetupRecord` and `Exit` (`domain/setup.rs`): what `worktree.json` records about the setup,
  `Running`, `Succeeded` or `Failed(Exit)`, where `Exit` is a non-zero `Code` or a `Signal`.
- `SetupOutcome`, `SetupReport` and `SetupDone` (`domain/setup.rs`): what running the setup did
  (`NoScript`, `Succeeded`, `Failed`), the report of a command that ran it (the script and its
  source, the log, the state it left), and what `worktree setup` did.
- `TypedError` (`domain/error.rs`): the failures a client tells apart; so far `Busy`.
- `WorktreeLock` (`adapters/fs.rs`): proof that this process holds a worktree's lock. It can
  only be made by taking the lock, and dropping it releases the lock.

## Invariants

- **The domain is pure.** Nothing in `src/domain/` touches the file system, starts a process,
  reads the environment or prints. It takes strings, bytes and parsed values, and returns values,
  decisions or errors. That is why it is unit-tested without a repository.
- **Only adapters touch the outside world.** Only `adapters/git.rs` starts git, only
  `adapters/script.rs` starts the setup script, only `adapters/fs.rs` reads or writes files, and
  only `adapters/env.rs` reads the environment.
- **Only `output` prints**, and only `output/json.rs` knows the JSON shape of the contract.
  Domain types never derive `Serialize` for output. A setup's live echo is a writer `output`
  makes and `cli` hands down, so ops and adapters never name stdout or stderr.
- **Each command is one ops module**: a short sequence of adapter and domain calls, readable top
  to bottom. Shared steps go in `ops.rs`.
- **Dependencies point down**: main > edge > ops > adapters > domain. A module uses its own layer
  or a lower one, through `crate::` paths, never a higher one.
- **Nothing writes on a read path**: `worktree ls` only reads. It probes a worktree's lock with
  a non-blocking try that it releases at once, reads the setup log only for a failed setup, and
  never rewrites an interrupted setup (ADR 0003: volatile facts are checked on read).
- **Writes to a worktree's state hold its lock.** The function that writes `worktree.json`
  (`ops::write_state`) takes a `&WorktreeLock`, so a write without the lock does not compile.
  `worktree ls` reads a setup recorded as running once more when its lock is free, since the
  setup records its end before releasing the lock. `worktree new` takes the lock as soon as
  git's worktree exists and holds it until the setup
  ends; `worktree setup` and `worktree rm` take it without waiting and fail as `busy` when it is
  held.
- **A failed setup undoes nothing**: the worktree, its branch and its log stay, and the worktree
  is not ready until `worktree setup` succeeds.
- **Errors** are `anyhow` errors with context, written for a person. Domain parsers return small
  `thiserror` enums (`WorktreeNameError`, `BranchNameError`) whose messages a person can read.
  Failures the client must tell apart are `TypedError` cases (`domain/error.rs`), carried inside
  the `anyhow` error and found again by downcasting. Without `--json` an error is printed on
  stderr as `orca-term: <message>`; with `--json` it is a JSON document on stdout:
  `{"contract": 1, "version": "...", "error": {"kind": "...", "message": "..."}}`, where `kind` is
  the typed error's (`busy`) or `other`.
- **Exit codes**: 0 on success; 1 for an untyped failure, and for `worktree new` and
  `worktree setup` when the setup failed (the worktree exists and the normal report says it is
  not ready); 2 for a usage error (clap); 3 for `busy`, a worktree whose lock another process
  holds.

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
- The compiler keeps writes to a worktree's state under its lock: `ops::write_state` takes a
  `&WorktreeLock`, which only taking the lock can make.

## Tests

- **Seam 1, black box**: `tests/worktree_*.rs` run the binary against throwaway git repositories
  built by `tests/support/mod.rs`, and check what a person or the client sees.
- **Unit tests** sit next to the pure code they test, in a `#[cfg(test)] mod tests` at the end of
  each domain and edge module. Adapters keep the few tests that need a real directory.
- **Seam 2, the Neovim client against the real core**: `spec/*_spec.lua` run busted via nlua in
  headless Neovim against the binary `cargo build` makes, in throwaway git repositories built by
  `spec/support.lua`. See "Neovim client" below.
- **Lints** have their own unit tests in `xtask/src/`.

## Neovim client

The Neovim client is Lua at the repository root, beside the crate (ADR 0008), laid out as a
Neovim plugin: Neovim runs `plugin/` at startup and `require` finds the modules in `lua/`. It needs
Neovim 0.11 or later. The client is thin: it runs the core's commands with `--json` and shows
what they print.

- `plugin/orca-term.lua`: defines the `:OrcaTerm <subcommand>` command, with completion of the
  subcommands, and nothing else.
- `lua/orca-term/init.lua`: `setup(opts)` and `config`, the settings (so far `binary`, the path to
  the core binary). Calling setup() is optional; `config` holds the defaults until then.
- `lua/orca-term/core.lua`: the link to the core. `find_binary` takes `binary` from setup(), else
  `target/release/orca-term` or `target/debug/orca-term` in the repository the client is loaded
  from, else `orca-term` on PATH. Each command (so far `worktree_ls`) runs the binary with
  `--json`, waits for it, decodes the JSON and refuses a document whose `contract` differs from
  the client's own `CONTRACT`. It returns the document, or `nil` and an error `{ kind, message }`
  whose message names the fix; it never throws.
- `lua/orca-term/sidebar.lua`: `:OrcaTerm sidebar`, a stub until issue #30 builds the sidebar.

Its rules:

- **Only `core.lua` talks to the core.** It is the only Lua file that starts a process; every
  other module calls it. `cargo xtask client-boundary` refuses a process started anywhere else in
  `lua/` or `plugin/`.
- **The client never computes state.** The fleet, each worktree's state and every decision come
  from the core's JSON; the client renders them. No logic is bound to the Neovim process.
- **No global keymap.** Every key the client adds is local to one of its buffers;
  `spec/plugin_spec.lua` checks that loading the client defines no global keymap.
