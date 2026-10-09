---
status: accepted
---

# The core is a layered library with a thin binary, enforced by guards

The core is built mostly by coding agents, and other people will have to read it later, so its
shape has to be obvious and has to stay that way without anyone policing it by hand. The code lives
in a **library** (`src/lib.rs`), and `src/main.rs` only parses arguments, calls the library and
turns an error into an exit code. Inside the library, modules sit in **five layers**, and each may
depend only on the layers below it:

1. **main**: the binary.
2. **edge**: `cli` (the command-line surface) and `output`, the only module that writes to stdout
   or stderr, together with the JSON views it prints.
3. **ops**: one module per command (`worktree new`, `ls`, `rm`, `prune`, ...), each a thin
   sequence of calls into adapters and domain, following Cargo's own "each command is a thin
   wrapper around ops".
4. **adapters**: the only modules that touch the outside world: `git`, the file system, and later
   the holder, the sandbox, Neovim and the agents.
5. **domain**: pure rules (worktree, fleet, config, state, the copy list). They never touch the
   file system, processes, the environment or stdout, and they import only other domain modules.

Domain values get types of their own, parsed once at the edge ("parse, don't validate"): a
`WorktreeName` can only exist if it is valid, and enums replace booleans and string results
(`Removal::{Safe, Forced}`, `State::Broken(reason)`). Traits appear only when two real
implementations exist.

Errors are typed where someone reacts to them. Domain parsers return small `thiserror` enums.
Failures the Neovim client must tell apart (a held lock, a schema too new, a missing sandbox) are
typed and leave the core as a JSON `error.kind` with an exit code of their own; they are added with
the first such case. Everything else uses `anyhow` with context, for messages a person reads.

Decided in [issue #47](https://github.com/marivaldo/orca-term/issues/47), grounded in the research
on branch `research/rust-architecture` (`docs/research/rust-architecture.md`).

## Considered options

- **A binary crate only**, as the code started. Rejected: `pub` means nothing, doctests do not
  run, and nothing separates the CLI from the logic.
- **Several crates now** (core, cli, adapters). Rejected for now: the compiler would enforce the
  layers, but at about 2,000 lines the build and maintenance cost outweighs it. Revisit at about
  10,000 lines, the threshold rust-analyzer's author gives.
- **Layers documented but not enforced.** Rejected: coding agents do not reliably follow prose,
  and this project turns every convention into a guard.
- **`thiserror` everywhere, or `anyhow` everywhere.** Rejected: the first means a lot of
  error-conversion code nobody reads; the second leaves the client unable to react to anything
  but the message text.

## Consequences

- **Layout:** one file per module, `foo.rs` growing into `foo.rs` plus `foo/`, never `mod.rs`
  (Clippy `mod_module_files`). A file is split when it holds two concepts, not at a line count.
- **Visibility:** `pub(crate)` by default, and `pub` only for what `main.rs` and the tests call
  (rustc `unreachable_pub`).
- **Size:** functions of at most 60 lines (`too-many-lines-threshold`) and at most 4 levels of
  nesting (`excessive-nesting-threshold`).
- **Layers are guarded twice:** Clippy `disallowed-methods` and `disallowed-types` reject
  `std::process`, `std::fs` and `std::env` outside adapters, and `cargo xtask layers` checks every
  `use crate::` against a module-to-layer table and that each file opens with a `//!` naming its
  layer.
- **`ARCHITECTURE.md`** at the root is the map for people and agents: the layers, where each
  concept lives and the invariants. It is updated with every change that moves a module.
- **Not adopted:** `cognitive_complexity` (Clippy's own docs say it does not measure that),
  `missing_docs_in_private_items` (noise), and `cargo-modules` in CI (it only finds cycles).
- Three refactor slices bring the existing code in line before
  [Worktree setup](https://github.com/marivaldo/orca-term/issues/29).
