# Research: the core's Rust code architecture

Input for [#47 Decide the core's Rust code architecture](https://github.com/marivaldo/orca-term/issues/47).
Researched 2026-10-09 against rustc and Clippy 1.97 (`rust-toolchain.toml`), the code on
`feat/lane-new-rm` (commit `2048d68`), and primary sources.

**Tags.** `[source]` = a primary source, linked. `[docs]` = this repository's own docs or code.
`[measured]` = reproduced locally on rustc/Clippy 1.97; the command is given. `[unverified]` =
believed but not checked against a primary source. Untagged sentences under a *Recommendation*
heading are this document's judgement, not claims.

## Recommendation in one screen

1. **Library plus a thin binary**, in the one package that exists today. `src/main.rs` shrinks to
   parse, call, map the error to an exit code; everything else moves under `src/lib.rs`. The
   glossary already defines the core as "a library plus a CLI" `[docs: CONTEXT.md]`; the code
   is binary-only. The xtask stays a workspace member.
2. **`foo.rs` + `foo/`, never `mod.rs`** inside `src/`, enforced by `clippy::mod_module_files`.
   Modules are named after glossary nouns (lane, fleet, turn, note, holder, agent, ...) or after
   the one external system they talk to (git, fs, nvim, sandbox).
3. **Four layers with one direction**: *edge* (`cli`, `output`) → *ops* (one function per command)
   → *adapters* (git, fs, later holder, sandbox, nvim) → *domain* (pure types and decisions).
   Domain imports only domain. Enforced by Clippy's `disallowed_methods`/`disallowed_types` for
   purity and a small `cargo xtask layers` check for the import direction, with an
   `ARCHITECTURE.md` as the map.
4. **Types**: newtypes `LaneName`, `PrimaryCheckout`, `AdminDir`, `Branch`; enums `LaneTarget`,
   `StartPoint`, `Removal`, `KeptReason`, and a `State` that carries why a lane is broken instead
   of a `gone: bool` beside it. Serializable JSON views live in `output`, apart from domain types.
5. **Errors**: `anyhow` stays in ops, adapters and the edge. Domain parsers return small
   `thiserror` enums. When the Neovim client must branch on a failure (schema too new, lock
   held, sandbox missing), that failure becomes a typed error found at the edge by downcasting.
6. **Guards**: add `mod_module_files`, `unreachable_pub`, `too-many-lines-threshold = 60`,
   `excessive-nesting-threshold = 4`, the `disallowed_*` lists, `cargo xtask layers`; drop the dead
   `module_name_repetitions` allow. Do not adopt `cognitive_complexity` or `redundant_pub_crate`.

The full proposal, with a one-line rationale per item, is in
[Recommendations for orca-term](#recommendations-for-orca-term).

## What exists today

`feat/lane-new-rm`, about 1,700 lines under `src/` plus 900 lines of integration tests
`[measured: wc -l]`:

| Module | Lines | What it does | Talks to the outside world? |
|---|---:|---|---|
| `main.rs` | 26 | declares all modules, parses argv, maps the error to an exit code | no |
| `cli.rs` | 92 | clap types and dispatch; reads `current_dir`, builds `Env` | yes (cwd) |
| `config.rs` | 345 | YAML layers, precedence, `~` expansion; `Env::from_process` | yes (files, env) |
| `contract.rs` | 7 | `CONTRACT`, `VERSION` | no |
| `fleet.rs` | 226 | `git worktree list` parsing, lanes, state per lane | yes (git, `is_dir`) |
| `git.rs` | 52 | the one place that spawns git | yes (process) |
| `include.rs` | 158 | `.worktreeinclude` list and copy | yes (git, files) |
| `lane.rs` | 374 | `lane new`, `lane rm`, `lane prune`, default-branch discovery | yes (git, files) |
| `output.rs` | 260 | the only writer of stdout/stderr; tables and the JSON envelope | yes (stdout) |
| `state.rs` | 197 | `lane.json` schema, atomic write | yes (git, files) |

Observations `[docs]`, from reading the modules and their `use crate::` lines:

- The internal import graph is already acyclic: `cli → {config, fleet, lane, output}`,
  `lane → {config, fleet, git, include, state}`, `fleet → {git, state}`, `include → git`,
  `state → git`, `output → {config, contract, fleet, include, lane, state}`.
- Pure logic is already factored into private functions inside each module
  (`fleet::parse_porcelain`, `config::resolve`/`expand`/`normalize`, `include::stays_inside`,
  `output::render_*`), and those are what the unit tests cover. What is missing is a boundary:
  decisions and I/O sit in the same module, and `lane.rs` calls `git::probe` with raw argument
  arrays in six places.
- Domain values are bare `String`/`PathBuf`/`bool`: `lane::create(dir: &Path, name: &str, ..)`
  takes either a directory or the primary checkout through the same `&Path`;
  `lane::remove(.., force: bool)`; `merged_into_default` returns
  `Result<Result<String, String>>`; `Lane` carries `state: State` *and* `gone: bool` *and*
  `detail: Option<String>`, which allows combinations that mean nothing (`gone` with `NoAgent`).
- The JSON contract (ADR 0008) is the `Serialize` derive on `Fleet`/`Lane`/`Worktree`, with
  `#[serde(skip)]` on internal fields, so renaming a domain field silently changes the contract
  (the `lane ls --json` tests would catch it; nothing in the type system does).
- Errors are `anyhow` throughout, with good `.context()` messages naming files and commands.
- Every item is `pub` or private; with no library, every `pub` is unreachable from outside, so
  `pub` says nothing. Turning on rustc's `unreachable_pub` reports 52 items (42 in `src/`, 7 in
  `tests/support/mod.rs`, 3 in `xtask`) `[measured: cargo clippy --workspace --all-targets -- -W unreachable_pub]`.
- Function size is moderate: nothing exceeds Clippy's default 100 lines; the longest are
  `lane::remove` (59), `lane::create` (58), `include::copy` (41) and `lane::resolve` (41)
  `[measured: too-many-lines-threshold = 40]`. No block is nested deeper than 4
  `[measured: excessive-nesting-threshold = 4, 0 hits]`.

What is coming, from the ADRs, shapes the layout: a holder process per lane that owns a PTY
(ADR 0009), an OS sandbox around every agent (ADR 0007), per-launch hooks for claude-code and
opencode, turns bounded by tree snapshots (ADR 0006), msgpack-RPC notifications to Neovim (ADR
0004), state files with a `schema` integer and a per-lane lock (ADR 0003, 0008, harness row for
#29) `[docs: docs/adr/]`.

## 1. Crate shape: library plus thin binary, or binary only

### Findings

- The Rust Book's chapter on binary projects prescribes the split: "Split your program into a
  *main.rs* file and a *lib.rs* file and move your program's logic to *lib.rs*", and limits what
  stays in `main` to calling the argument parsing, setting up configuration, "Calling a `run`
  function in *lib.rs*" and "Handling the error if `run` returns an error". The reason: "Because
  you can't test the `main` function directly, this structure lets you test all of your
  program's logic by moving it out of the `main` function." It also allows that "As long as your
  command line parsing logic is small, it can remain in the `main` function". Its own listing is
  looser than its guideline: only `search` moves to *src/lib.rs*, while `Config`, `Config::build`
  and `run` stay in *src/main.rs*, with "Almost all of our work will be done in *src/lib.rs* from
  here on out" [source](https://doc.rust-lang.org/book/ch12-03-improving-error-handling-and-modularity.html)
- The testing chapter states the consequence for integration tests: with only *src/main.rs*, "we
  can't create integration tests in the *tests* directory and bring functions defined in the
  *src/main.rs* file into scope with a `use` statement. Only library crates expose functions that
  other crates can use", which "is one of the reasons Rust projects that provide a binary have a
  straightforward *src/main.rs* file that calls logic that lives in the *src/lib.rs* file."
  [source](https://doc.rust-lang.org/book/ch11-03-test-organization.html)
- Cargo's default layout puts the library at `src/lib.rs` and the executable at `src/main.rs`,
  side by side [source](https://doc.rust-lang.org/cargo/guide/project-layout.html). "Binaries can
  use the public API of the package's library", and so can integration tests; the library cannot
  see the binary [source](https://doc.rust-lang.org/cargo/reference/cargo-targets.html).
- Doctests only exist for libraries: the `doctest` field "is only relevant for libraries, it has no
  effect on other sections" [source](https://doc.rust-lang.org/cargo/reference/cargo-targets.html).
- `cargo doc` already documents private items for binaries: `--document-private-items` "will be
  enabled by default if documenting a binary target"
  [source](https://doc.rust-lang.org/cargo/commands/cargo-doc.html). So better rustdoc is *not* an
  argument for the split; doctests and visibility are.
- Cargo itself is a library plus a binary: `src/lib.rs` documents "[`ops`]: Every major operation
  is implemented here. Each command is a thin wrapper around ops", and the binary lives at
  `src/bin/cargo/{main.rs, cli.rs, commands/}` [source](https://github.com/rust-lang/cargo/blob/master/src/lib.rs).
- Splitting into several crates of a workspace is a different, larger step. matklad recommends a
  flat `crates/` directory with a virtual manifest "for projects in between ten thousand and one
  million lines of code" [source](https://matklad.github.io/2021/08/22/large-rust-workspaces.html).
  Each crate is a unit of separate compilation, so "Without `#[inline]`, even the most trivial of
  functions can't be inlined across the crate boundary"
  [source](https://matklad.github.io/2021/07/09/inline-in-rust.html). Cargo rejects a cycle
  between packages outright ("error: cyclic package dependency")
  `[measured: two path crates depending on each other, cargo metadata]`, so a crate split gives
  a compiler-enforced dependency direction, at a cost the core does not need at 1,700 lines.
- matklad also recommends keeping repository automation in Rust as an `xtask` crate rather than
  shell scripts [source](https://matklad.github.io/2021/08/22/large-rust-workspaces.html), which
  is what `xtask/` already is.

### Recommendation

One package, `src/lib.rs` plus a `src/main.rs` of a dozen lines. Clap types and dispatch live in
the library's `cli` module. Keeping them in the binary is also legitimate (the Book's listing
does, and Cargo does in `src/bin/cargo/cli.rs`), but a binary with its own modules needs its own
directory (`src/bin/orca-term/`) so its files do not sit in `src/` beside library modules; at 92
lines of clap code that is not worth it, and a library `cli` module falls under the same layer
check as everything else. The real gains are:
`pub` regains a meaning (the library's surface) so `unreachable_pub` can police it; doctests run
on domain types; the library structurally cannot reach into `main.rs`. Keep the black-box
`assert_cmd` suites as the primary tests; rust-analyzer's experience is that "Tests which directly
call various API functions are a liability" and its invariant is that tests are data driven
[source](https://rust-analyzer.github.io/book/contributing/architecture.html). Revisit a
`crates/` split only past roughly 10,000 lines or when a second consumer of the library appears.
The xtask stays a workspace member: it is where the new layering check goes.

## 2. Module layout

### Findings

- The Book calls `src/front_of_house.rs` (with children in `src/front_of_house/`) "the most
  idiomatic file paths", and `src/front_of_house/mod.rs` the "older style, still supported path".
  Mixing both for one module is a compiler error; mixing across modules "might be confusing for
  people navigating your project", and "The main downside to the style that uses files named
  *mod.rs* is that your project can end up with many files named *mod.rs*"
  [source](https://doc.rust-lang.org/book/ch07-05-separating-modules-into-different-files.html).
- The Reference: "Prior to `rustc` 1.30, using `mod.rs` files was the way to load a module with
  nested children. It is encouraged to use the new naming convention as it is more consistent, and
  avoids having many files named `mod.rs` within a project."
  [source](https://doc.rust-lang.org/reference/items/modules.html)
- One exception is required: shared helpers for integration tests go in `tests/common/mod.rs`,
  because "Files in subdirectories of the *tests* directory don't get compiled as separate crates
  or have sections in the test output" [source](https://doc.rust-lang.org/book/ch11-03-test-organization.html).
  The repo already does this with `tests/support/mod.rs` `[docs]`.
- Modules exist to "group related definitions together and name why they're related", so a reader
  "would know where to place the code" [source](https://doc.rust-lang.org/book/ch07-02-defining-modules-to-control-scope-and-privacy.html).
- matklad's measure of what an unfamiliar contributor pays: about 2x more time to write a patch,
  and about 10x to find *where* it goes, which an architecture map addresses
  [source](https://matklad.github.io/2021/02/06/ARCHITECTURE.md.html). For coding agents, which
  start every time unfamiliar, the "where does it go" cost is the one to minimise
  `[unverified: extrapolation]`.
- No primary source gives a line limit for modules. Clippy's only size lints are per function
  (`too_many_lines`) and per nesting depth (`excessive_nesting`); see section 7.

### Recommendation

- `foo.rs` + `foo/` everywhere in `src/`; `mod.rs` only under `tests/`.
- A module is named after one glossary noun or one external system. The glossary is already
  enforced by `cargo xtask vocab`, so module names inherit that guard.
- **Vocabulary note.** The glossary term an agent's exchange with the person is called (defined
  between *Agent* and *Turn* in `CONTEXT.md`) is also listed under *Holder*'s *Avoid* line, and
  the lint bans every *Avoid* term repository-wide. So that word cannot appear in this document,
  and a module named after it would fail `cargo xtask vocab` `[measured: cargo xtask vocab on a
  draft of this file]`. The glossary needs a decision (drop it from *Holder*'s list, or waive
  it) before the slice that introduces that module.
- Split a module into `foo.rs` + `foo/` when it holds two concepts a reader would look for
  separately (today: `lane.rs` holds three commands), not at a line count. A soft signal of about
  400 non-test lines is worth a look, but should not become a gate.
- Unit tests stay inline (`#[cfg(test)] mod tests`); integration tests stay per command in
  `tests/` with `tests/support/mod.rs`.

## 3. Layering and dependency direction

### Findings

- *Functional core, imperative shell* (Gary Bernhardt): a core of pure code, and "This functional
  core is surrounded by a shell of imperative code: it manipulates stdin, stdout, the database,
  and the network" [source](https://www.destroyallsoftware.com/screencasts/catalog/functional-core-imperative-shell).
- *Ports and adapters* (Alistair Cockburn): "Allow an application to equally be driven by users,
  programs, automated test or batch scripts, and to be developed and tested in isolation from its
  eventual run-time devices and databases." A port is a purposeful exchange with the outside, expressed as an API; an adapter
  is the technology-specific code on a port [source](https://alistair.cockburn.us/hexagonal-architecture/).
- rust-analyzer applies both in Rust idiom and writes them down as *architecture invariants*,
  often as absences [source](https://rust-analyzer.github.io/book/contributing/architecture.html):
  - "`syntax` crate is completely independent from the rest of rust-analyzer. It knows nothing
    about salsa or LSP." "`base-db` doesn't know about file system and file paths."
  - "core parts of rust-analyzer (`ide`/`hir`) don't interact with the outside world and thus
    can't fail. Only parts touching LSP are allowed to do IO."
  - "`rust-analyzer` is the only crate that knows about LSP and JSON serialization. If you want
    to expose a data structure `X` from ide to LSP, don't make it serializable. Instead, create a
    serializable counterpart in `rust-analyzer` crate and manually convert between the two."
  - It marks *API boundaries* ("rules at the boundary are different") and keeps the boundary API
    "build out of POD types with public fields".
- Its style guide prefers plain functions to objects: "Avoid creating 'doer' objects", and
  "Express function preconditions in types and force the caller to provide them"
  [source](https://rust-analyzer.github.io/book/contributing/style.html).
- matklad's `ARCHITECTURE.md` recipe: a bird's-eye view, a codemap of coarse modules, invariants
  ("especially those expressed as the absence of something"), boundaries ("good boundaries have
  measure zero", so they are invisible in code) and cross-cutting concerns. Keep it short, name
  modules and types without linking them, "Don't try to keep it synchronized with code", revisit
  it a couple of times a year [source](https://matklad.github.io/2021/02/06/ARCHITECTURE.md.html).
- Cargo's `lib.rs` crate docs double as its codemap, and its rule "Each command is a thin wrapper
  around ops" is the operations layer [source](https://github.com/rust-lang/cargo/blob/master/src/lib.rs).
- In Rust idiom, a port does not have to be a trait. A module whose functions take and return
  domain types (`git::worktrees(&PrimaryCheckout) -> Result<Vec<Worktree>>`) is a port with one
  adapter; the test double for it is a real throwaway repository, which `tests/support` already
  builds `[docs]`. A trait earns its place when there are two production implementations or a
  fake is the only way to test (section 4).

### Recommendation

Four layers, named in `ARCHITECTURE.md`, each module assigned to exactly one:

| Layer | Modules (today → proposed) | May import | Must not |
|---|---|---|---|
| **edge** | `cli`, `output` | ops, domain | be imported by anything but `main.rs` (and `cli` → `output`) |
| **ops** | `ops::{lane_new, lane_rm, lane_prune, lane_ls}` (from `lane.rs`, `Fleet::discover`, `Config::load`) | adapters, domain | print, parse argv |
| **adapters** | `git`, `fs`; later `holder`, `sandbox`, `nvim`, `agent::{claude_code, opencode}` | domain | import ops or edge, or each other without a recorded reason |
| **domain** | `lane`, `fleet`, `config`, `state`, `include`, `contract`; later `turn`, `note` and the rest of the glossary | domain | touch the file system, spawn processes, read the environment, print |

Invariants to write in `ARCHITECTURE.md`, as absences: domain modules never do I/O and so can only
fail by rejecting input; only `git` spawns git; only `output` writes stdout and stderr (already
enforced); only `output` knows the JSON shape of the contract; nothing writes on a read path
(already tested).

## 4. Types

### Findings

- C-NEWTYPE: "Newtypes can statically distinguish between different interpretations of an
  underlying type." C-CUSTOM-TYPE: "Core types like `bool`, `u8` and `Option` have many possible
  interpretations"; prefer a call whose arguments are named enum variants to `Widget::new(true, false)`
  [source](https://rust-lang.github.io/api-guidelines/type-safety.html).
- C-VALIDATE ranks static enforcement first: "Choose an argument type that makes invalid inputs
  impossible", before dynamic checks [source](https://rust-lang.github.io/api-guidelines/dependability.html).
- The Book's `Guess` type keeps its field private so "there's no way for a `Guess` to have a
  `value` that hasn't been checked", and a function taking a `Guess` "wouldn't need to do any
  additional checks in its body" [source](https://doc.rust-lang.org/book/ch09-03-to-panic-or-not-to-panic.html).
- *Parse, don't validate*: "a parser is just a function that consumes less-structured input and
  produces more-structured output"; make illegal states unrepresentable and push the parsing to
  the boundary [source](https://lexi-lambda.github.io/blog/2019/11/05/parse-don-t-validate/).
  In Rust the conventional entry point is `FromStr` ("Parse a value from a string"), which
  `str::parse` calls [source](https://doc.rust-lang.org/std/str/trait.FromStr.html), and clap's
  `value_parser!` accepts any `FromStr` type as a fallback, so a newtype can be parsed straight
  from argv [source](https://docs.rs/clap/latest/clap/macro.value_parser.html).
- C-NEWTYPE-HIDE: "A newtype can be used to hide representation details while making precise
  promises to the client." C-STRUCT-PRIVATE: "Making a field public is a strong commitment"
  [source](https://rust-lang.github.io/api-guidelines/future-proofing.html).
- Newtypes are "a zero-cost abstraction - there is no runtime overhead"; their cost is forwarding
  boilerplate, "a 'pass through' method for every method you want to expose"
  [source](https://rust-unofficial.github.io/patterns/patterns/behavioural/newtype.html).
- Enums for state: Clippy's `struct_excessive_bools` explains that many bools in a struct are
  "often a sign that the type is being used to represent a state machine, which is much better
  implemented as an enum", because enums "more easily forbid invalid states"; it is in
  `pedantic`, already enabled here [source](https://rust-lang.github.io/rust-clippy/master/index.html#struct_excessive_bools).
  The Book's state-pattern chapter concludes that encoding states in types makes "invalid states
  now impossible because of the type system"
  [source](https://doc.rust-lang.org/book/ch18-03-oo-design-patterns.html).
- C-COMMON-TRAITS: new types should eagerly implement `Debug`, `Clone`, `PartialEq`, `Eq`, `Hash`,
  `Display`, `Default` and the like where they make sense, because of the orphan rule
  [source](https://rust-lang.github.io/api-guidelines/interoperability.html).
- Traits versus functions: the patterns book notes the Strategy pattern needs no trait in Rust
  ("we don't need to use traits in order to design this pattern in Rust"; closures work)
  [source](https://rust-unofficial.github.io/patterns/patterns/behavioural/strategy.html), and
  rust-analyzer's style guide warns against generic code ("Avoid making a lot of code type
  parametric") [source](https://rust-analyzer.github.io/book/contributing/style.html).

### Recommendation

| Today | Proposed | Why |
|---|---|---|
| `name: &str`, checked by `validate_name` | `LaneName` (`FromStr`; non-empty, one path segment, no leading `-`) | parsed once at argv; git's ref-format check stays in `ops::lane_new` because it needs git |
| `target: &str`, split by `contains('/')` inside `resolve` | `enum LaneTarget { Name(String), Path(PathBuf) }` | the name/path decision becomes a parse, not a branch buried in lookup |
| `primary: &Path`, `dir: &Path`, `lane: &Path` | `PrimaryCheckout(PathBuf)`, canonical by construction | functions that need the primary checkout cannot be handed any directory |
| `admin_dir: &Path` | `AdminDir(PathBuf)` | `state::read` cannot be handed the worktree path by mistake |
| `branch: String`, `start: String` (`"origin/main"`) | `Branch(String)`, `enum StartPoint { Local(Branch), Remote(Branch) }` | no string formatting to tell local from remote |
| `force: bool` | `enum Removal { Safe, Forced }` | C-CUSTOM-TYPE |
| `Result<Result<String, String>>`, `Kept { reason: String }` | `enum KeptReason { DefaultBranch, NotMerged { into }, DeleteFailed(String), NoDefault(String) }` | the client can branch on why a branch was kept |
| `state: State` + `gone: bool` + `detail: Option<String>` | `State::Broken(Broken::Gone { .. } \| Broken::Unreadable { .. })` | nonsense combinations become unrepresentable |
| `Serialize` on `Fleet`/`Lane` with `#[serde(skip)]` | `output::json::{FleetView, LaneView}` built from domain types | rust-analyzer's serializable-counterpart rule; the contract cannot drift with a field rename |

Traits: none today. The git adapter stays plain functions. The first candidate is the agent
interface (ADR 0001 asks for "one internal interface with one adapter each"
`[docs: docs/adr/0001]`); for a closed set of two agents, an `enum Agent { ClaudeCode, Opencode }`
with exhaustive `match` is the simpler first form, and a trait becomes worth it when a fake agent
is needed in tests.

## 5. Errors

### Findings

- anyhow's README: "Use Anyhow if you don't care what error type your functions return, you just
  want it to be easy. This is common in application code. Use thiserror if you are a library
  that wants to design your own dedicated error type(s) so that on failures the caller gets
  exactly the information that you choose." [source](https://github.com/dtolnay/anyhow) anyhow
  supports context ("Attach context to help the person troubleshooting the error understand
  where things went wrong") and downcasting "by value, by shared reference, or by mutable
  reference" [source](https://docs.rs/anyhow/latest/anyhow/) (version 1.0.104, the one locked here).
- thiserror is "a convenient derive macro for the standard library's `std::error::Error` trait";
  it "deliberately does not appear in your public API", so switching to hand-written impls is not
  a breaking change; it offers `#[error("...")]`, `#[from]`, `#[source]` and
  `#[error(transparent)]` [source](https://docs.rs/thiserror/latest/thiserror/) (version 2.0.21).
  Its README draws the same line as anyhow's: typed errors where the caller should get exactly
  the information you choose, "most often" library-like code
  [source](https://github.com/dtolnay/thiserror).
- C-GOOD-ERR: error types implement `std::error::Error`, are `Send` and `Sync`, never `()`, and
  their `Display` is "lowercase without trailing punctuation"
  [source](https://rust-lang.github.io/api-guidelines/interoperability.html). The existing
  messages already follow the last rule `[docs]`.
- The Book's binary-project chapter has `run` return `Result<(), Box<dyn Error>>` and leaves error
  handling to `main` [source](https://doc.rust-lang.org/book/ch12-03-improving-error-handling-and-modularity.html).
- rust-analyzer uses `anyhow::Result` in its application code
  [source](https://rust-analyzer.github.io/book/contributing/style.html) and makes its pure core
  unable to fail at all [source](https://rust-analyzer.github.io/book/contributing/architecture.html).
- ADR 0008: the client refuses a contract it does not speak, and every command that needs a
  missing prerequisite "refuses with the same diagnosis" as `doctor` `[docs: docs/adr/0008]`;
  an older core refuses to write a newer schema; writes hold a per-lane lock (#29)
  `[docs: docs/harness.md]`. These are failures the Lua client will need to tell apart.

### Recommendation

- **Ops, adapters, edge: `anyhow::Result` with `.context()`**, as today. Nobody branches on most
  of these failures; they are shown to a person.
- **Domain parsers: a small `thiserror` enum per parser** (`LaneNameError`, `ConfigError` for
  the YAML layer), because they are produced by pure code, flow into `FromStr`, and are
  unit-tested by variant.
- **Failures the client must branch on: typed, found at the edge.** When the first one lands
  (lock held in #29, schema too new, sandbox missing), define it with `thiserror`, let it travel
  inside the `anyhow::Error` chain with context, and have `main`/`output` `downcast_ref` it into
  a distinct exit code and, under `--json`, a stable error `kind`. Do not convert all of the core
  to typed errors ahead of that need.

## 6. Visibility, size and documentation

### Findings

- Visibility: "If an item is private, it may be accessed by the current module and its
  descendants"; `pub(crate)` "makes an item visible within the current crate"; `pub(super)`
  "visible to the parent module" [source](https://doc.rust-lang.org/reference/visibility-and-privacy.html).
- rustc's `unreachable_pub` (allow by default) "triggers for `pub` items not reachable from other
  crates", and recommends `pub(crate)` for crate-internal items because it "more clearly expresses
  the intent"; it is allow-by-default only "because it will trigger for a large amount of
  existing Rust code" [source](https://doc.rust-lang.org/rustc/lints/listing/allowed-by-default.html).
- Clippy's `redundant_pub_crate` (nursery) flags the opposite, `pub(crate)` inside a private
  module [source](https://rust-lang.github.io/rust-clippy/master/index.html#redundant_pub_crate);
  enabling both makes them contradict each other on the same item `[unverified: inferred from
  the two lint definitions, not run]`.
- rust-analyzer: "Never provide setters"; fields with invariants are private; prefer
  `use crate::foo::bar` to `use super::bar`; "adding an innocent-looking `pub use` is a very
  simple way to break encapsulation" [source](https://rust-analyzer.github.io/book/contributing/style.html).
- Documentation: C-CRATE-DOC (thorough crate docs), C-FAILURE (`# Errors`, `# Panics` sections),
  C-LINK (intra-doc links), C-EXAMPLE, C-QUESTION-MARK (examples use `?`, not `unwrap`)
  [source](https://rust-lang.github.io/api-guidelines/documentation.html). The API guidelines are
  "only guidelines" and "should not in any way be considered a mandate"
  [source](https://rust-lang.github.io/api-guidelines/about.html); they target published library
  APIs, so the repo already, and reasonably, allows `missing_errors_doc` and `missing_panics_doc`
  `[docs: Cargo.toml]`.
- rustdoc's own guide recommends `#![warn(missing_docs)]` to move toward full documentation
  [source](https://doc.rust-lang.org/rustdoc/write-documentation/what-to-include.html); `missing_docs`
  only covers public items [source](https://doc.rust-lang.org/rustc/lints/listing/allowed-by-default.html),
  and Clippy's `missing_docs_in_private_items` (restriction) covers the rest
  [source](https://rust-lang.github.io/rust-clippy/master/index.html#missing_docs_in_private_items).
  On this code it reports 56 items, mostly struct fields and test helpers
  `[measured: cargo clippy -p orca-term --bins -- -W clippy::missing_docs_in_private_items]`.
- rust-analyzer enforces a module doc comment on every file with a tidy test that fails when a
  file's first line is not `//!` [source](https://github.com/rust-lang/rust-analyzer/blob/master/xtask/src/tidy.rs).
  Every module in `src/` already starts with one `[docs]`.
- Function size: `too_many_lines` (pedantic, threshold 100 by default) says long functions "are
  harder to understand"; `excessive_nesting` (complexity, inert until a threshold is set)
  [source](https://rust-lang.github.io/rust-clippy/master/index.html#too_many_lines).

### Recommendation

- `pub(crate)` is the default for every item; `pub` only for what `main.rs` and `tests/` call.
  Enforced by `unreachable_pub`.
- No `pub use` re-exports inside the crate; import from the defining module.
- Every module starts with a `//!` paragraph that says what the module is in glossary words and
  which layer it belongs to. Items get `///` when their name does not say everything (the current
  practice). Do not turn on `missing_docs_in_private_items`: 56 hits, mostly fields, is noise.
- Function size: lower `too-many-lines-threshold` to 60 (just above today's longest function);
  set `excessive-nesting-threshold = 4` (today's maximum). Module size stays a review judgement.

## 7. Mechanical guards

### Clippy and rustc lints

Groups and defaults are from the Clippy lint list
[source](https://rust-lang.github.io/rust-clippy/master/index.html), cross-checked on Clippy 1.97
where marked.

| Lint | Group / default | What it enforces | State here | Proposal |
|---|---|---|---|---|
| `mod_module_files` | restriction / allow (since 1.57) | bans `mod.rs`, "Having multiple module layout styles in a project can be confusing" | off | **warn**. Flags `src/foo/mod.rs`, does not flag `tests/common/mod.rs` `[measured]` |
| `self_named_module_files` | restriction / allow | the opposite: bans `foo.rs` + `foo/` | off | no |
| `module_inception` | style / warn | a module named like its parent | on (via `all`) | keep |
| `too_many_lines` | pedantic / allow | function length, `too-many-lines-threshold` (default 100) | on at 100 (via `pedantic`) | threshold 60 |
| `excessive_nesting` | complexity / warn, inert until `excessive-nesting-threshold` is set | block nesting depth | inert | threshold 4 (0 hits today `[measured]`) |
| `cognitive_complexity` | restriction / allow | "We used to think it measured how hard a method is to understand"; left in restriction "so as to not mislead users"; suggests `excessive_nesting`, `too_many_lines` instead | off | **no** |
| `wildcard_imports`, `enum_glob_use` | pedantic / allow | no `use x::*` | on (via `pedantic`) | keep |
| `fn_params_excessive_bools`, `struct_excessive_bools` | pedantic / allow | bools as hidden enums | on (via `pedantic`) | keep; note they need 3+ bools, so `force: bool` stays a review call |
| `too_many_arguments` | complexity / warn | argument count | on | keep |
| `items_after_statements` | pedantic / allow | items declared mid-block | on | keep |
| `module_name_repetitions` | restriction / allow | `lane::LaneName` stutter | `allow`ed in `Cargo.toml` | **remove the allow**: on 1.97 it is restriction, so it is already off and the line is dead `[measured: fires only under -W clippy::restriction]` |
| `unreachable_pub` (rustc) | allow | `pub` that is not reachable from outside the crate | off | **warn**, after the library split (52 hits today, binary-only) |
| `redundant_pub_crate` | nursery / allow | `pub(crate)` in a private module | off | **no**: contradicts `unreachable_pub` |
| `missing_docs_in_private_items` | restriction / allow | `///` on every private item | off | no (56 hits, mostly fields) |
| `pub_use` | restriction / allow | bans `pub use` | off | optional; cheap if re-exports are to stay banned |
| `disallowed_methods`, `disallowed_types` | style / warn, inert until configured in `clippy.toml` | named paths are forbidden, with a `reason` shown in the diagnostic | inert | **configure** (below) |

`disallowed_methods`/`disallowed_types` turn the purity half of the layering rule into Clippy
diagnostics: list `std::process::Command`, `std::fs::*` functions and `std::fs::File`,
`std::env::var`/`var_os`/`current_dir` in `clippy.toml`, each with a reason, and let each
adapter module opt out with a module-level `#![expect(clippy::disallowed_methods, reason = "...")]`,
the same pattern `output.rs` uses for `print_stdout` today. Verified: with
`std::process::Command::new` disallowed, a domain module is flagged and an adapter module carrying
the `#![expect]` is not `[measured: scratch crate, Clippy 1.97]`. The limit: the lists are
crate-wide, so a domain module could add its own `#![expect]`; the xtask check below closes that.

### Enforcing dependency direction

| Approach | Enforces | Cost | Fit now |
|---|---|---|---|
| **Crate split** (`crates/orca-term-core` without clap, `crates/orca-term` with it) | direction, compiler-enforced (Cargo rejects cycles `[measured]`); a crate's `[dependencies]` bound what it can use | more manifests; cross-crate inlining needs `#[inline]` or LTO [source](https://matklad.github.io/2021/07/09/inline-in-rust.html); matklad's flat layout is pitched at 10k+ lines [source](https://matklad.github.io/2021/08/22/large-rust-workspaces.html) | later |
| **`cargo-modules`** | visualises the module graph; `dependencies --acyclic` fails on cycles, `orphans --deny` fails on unlinked files; no custom allow/deny rules [source](https://github.com/regexident/cargo-modules) | another tool to install and pin in CI | no: the graph is already acyclic, and acyclic is weaker than "domain never imports adapters" |
| **`cargo xtask layers`** (new): a table `module → layer` in the xtask, a scan of `use crate::` and `crate::` paths in `src/**/*.rs`, and a check that `#![expect(clippy::disallowed_*)]` appears only in adapter and edge modules | the exact rule of section 3; fails on a module missing from the table, so every new module is classified on purpose | ~100 lines of Rust, in the crate that already parses `CONTEXT.md` for `vocab`; textual, so `use` aliases or macros could evade it | **yes** |
| **Clippy `disallowed_*`** | purity (no I/O in domain), with reasons in the diagnostics | config only | **yes**, with the xtask check |
| **Review only** | whatever reviewers remember | none up front | no: `docs/harness.md` adopts a rule only with its guard |

The xtask scan is textual by design: rust-analyzer enforces its own repository rules the same
way, with tidy tests that read source files
[source](https://github.com/rust-lang/rust-analyzer/blob/master/xtask/src/tidy.rs). The table
lives in the xtask rather than in `ARCHITECTURE.md`, so the map can stay short and stable, as
matklad advises [source](https://matklad.github.io/2021/02/06/ARCHITECTURE.md.html), while the
check stays exact.

## 8. Design patterns in Rust

### Findings

- The *Rust Design Patterns* book (a community project, `rust-unofficial`, not official Rust
  documentation): "Rust is not object-oriented", so "Rust design patterns vary with respect to
  other traditional object-oriented programming languages"; it applies YAGNI and says the
  Strategy pattern can be replaced by traits, closures or plain functions
  [source](https://rust-unofficial.github.io/patterns/intro.html),
  [source](https://rust-unofficial.github.io/patterns/patterns/index.html),
  [source](https://rust-unofficial.github.io/patterns/patterns/behavioural/strategy.html).
- Idiomatic and useful here:
  - **Newtype**: zero-cost static distinctions (section 4)
    [source](https://rust-unofficial.github.io/patterns/patterns/behavioural/newtype.html).
  - **RAII guards**: "relying on the type system to ensure that access is always mediated by the
    guard object", with cleanup in `Drop`; `MutexGuard` is the std example
    [source](https://rust-unofficial.github.io/patterns/patterns/behavioural/RAII.html). The
    per-lane lock of #29 (on `File::lock`, in std on 1.97 `[docs: docs/adr/0004]`) should be a
    guard type whose lifetime is the critical section, so "write while holding the lock" is
    structural.
  - **Builder** (C-BUILDER): for values with many optional inputs; non-consuming `&mut self`
    builders are preferred [source](https://rust-lang.github.io/api-guidelines/type-safety.html).
    Fits the agent launch (command, sandbox profile, hook settings, `--resume` id) later; not
    needed today.
  - **`Default`** instead of a zero-argument `new` [source](https://rust-unofficial.github.io/patterns/idioms/default.html),
    [source](https://rust-analyzer.github.io/book/contributing/style.html).
  - **Encoding state in enums or types** instead of the OOP State pattern: "Object-oriented
    patterns won't always be the best solution in Rust due to certain features, like ownership"
    [source](https://doc.rust-lang.org/book/ch18-03-oo-design-patterns.html).
- What does not translate:
  - **Inheritance through `Deref`** is an anti-pattern: it "Misuse[s] the `Deref` trait to emulate
    inheritance", is surprising, and gives no real subtyping; use traits or explicit forwarding
    [source](https://rust-unofficial.github.io/patterns/anti_patterns/deref.html).
  - **"Doer" objects and getter/setter pairs**: rust-analyzer says avoid them and "Never provide
    setters" [source](https://rust-analyzer.github.io/book/contributing/style.html).
  - **Interfaces for everything** (one trait per adapter "for testability"): the Strategy page
    and rust-analyzer's warning against type-parametric code point to plain functions and enums
    first; a trait is warranted at two implementations or a needed fake.

## Recommendations for orca-term

### Proposed layout

```text
Cargo.toml                 one package (lib + bin) and the xtask workspace member
ARCHITECTURE.md            bird's-eye view, layers, invariants as absences, codemap of layers
src/
├── main.rs                bin: parse argv, call orca_term::cli::run, map the error to an exit code
├── lib.rs                 //! the codemap in one screen; `mod` declarations only
│
├── cli.rs                 edge: clap types (LaneName parsed by FromStr), dispatch to ops; reads cwd and env
├── output.rs              edge: the only stdout/stderr writer; text rendering; error → exit code
├── output/
│   └── json.rs            edge: serializable views (FleetView, LaneView) and the contract envelope
│
├── ops.rs                 ops: "each command is a thin wrapper around ops"; shared helpers
├── ops/
│   ├── lane_new.rs        ops: from lane::create, plus Config::load and the .worktreeinclude copy
│   ├── lane_rm.rs         ops: from lane::remove
│   ├── lane_prune.rs      ops: from lane::prune
│   └── lane_ls.rs         ops: from Fleet::discover and Lane::read_state
│
├── git.rs                 adapter: the only spawner of git; typed queries (worktrees, default_branch,
│                          has_ref, worktree_add, worktree_remove, admin_dir, check_branch_name)
├── git/
│   └── porcelain.rs       adapter: pure parser of `worktree list --porcelain -z`, unit-tested
├── fs.rs                  adapter: read_optional, write_atomically, copy_file
│
├── lane.rs                domain: LaneName, LaneTarget, Lane, State/Broken, Removal, KeptReason
├── fleet.rs               domain: Fleet from worktree records, gone lanes
├── config.rs              domain: Config, Setting, Env (values only), layer precedence, expand, normalize
├── state.rs               domain: LaneFile, schema rule (refuse to write a newer schema)
├── include.rs             domain: the copy list's rules (stays_inside, NUL-separated parsing)
└── contract.rs            domain: CONTRACT, VERSION
    later: turn.rs, note.rs (domain; see the vocabulary note); holder.rs, sandbox.rs, nvim.rs, lock.rs,
           agent.rs + agent/{claude_code,opencode}.rs (adapters)
tests/                     black-box suites per command; tests/support/mod.rs stays
xtask/src/layers.rs        new: the layering check
```

Rationale, one line each:

- **lib + thin bin**: the Book's binary-project split and the glossary's "library plus a CLI";
  gives `pub` a meaning, doctests, and a library that cannot reach into `main.rs`.
- **One package, no `crates/` yet**: matklad's workspace layout is pitched at 10k+ lines; Clippy
  and the xtask enforce the same rule more cheaply now.
- **`ops/` per command**: Cargo's own rule; one file per command is where an agent looks first,
  and it splits the 374-line `lane.rs` along the lines it already has.
- **`git` with typed queries**: removes six raw `git::probe(&[...])` call sites from domain code
  and makes "only git spawns git" checkable.
- **`output/json.rs` views**: rust-analyzer's serializable-counterpart rule; domain refactors stop
  being contract changes.
- **Glossary-named modules**: the vocabulary lint already guards those names, so the map, the code
  and the glossary share one language.

### Layering rule

`main → edge → ops → adapters → domain`, and `domain → domain` only. Edge may also use domain
types directly; adapters may not import each other without a reason recorded in the xtask table.
Domain code never touches the file system, spawns a process, reads the environment or prints.
*Rationale: the functional core / imperative shell split rust-analyzer writes down as "core
parts ... don't interact with the outside world and thus can't fail".*

### Error strategy

- `anyhow::Result` + `.context()` in ops, adapters and edge. *Rationale: the failures are read by
  a person; anyhow's own guidance for application code.*
- `thiserror` enums for domain parsers (`LaneNameError`, `ConfigError`). *Rationale: pure code,
  unit-tested by variant, feeds `FromStr`.*
- Typed errors for anything the client branches on, downcast at the edge into an exit code and a
  `--json` error `kind`, introduced with the first such case (#29's lock). *Rationale: ADR 0008's
  "refuses with the same diagnosis" needs machine-readable failures; nothing needs them yet.*
- Add `thiserror` (MIT OR Apache-2.0, accepted by `deny.toml` `[unverified: license of its
  proc-macro dependencies not re-checked]`) in the slice that introduces `LaneName`.

### Newtypes and enums to introduce

- `LaneName`: *parsed once from argv via `FromStr`; replaces `validate_name`.*
- `LaneTarget { Name, Path }`: *makes `lane rm`'s name-or-path rule a parse.*
- `PrimaryCheckout`: *canonical by construction; ends `&Path` mix-ups between a directory and the
  primary checkout.*
- `AdminDir`: *state functions cannot receive a worktree path.*
- `Branch` and `StartPoint { Local, Remote }`: *no `format!("origin/{}")` to tell them apart.*
- `Removal { Safe, Forced }`: *C-CUSTOM-TYPE, instead of `force: bool`.*
- `KeptReason`: *replaces `Result<Result<String, String>>` and free-text reasons.*
- `State::Broken(Broken)`: *removes `gone: bool`; illegal combinations become unrepresentable.*

### Guards to add

| Guard | Where | Rationale |
|---|---|---|
| `clippy::mod_module_files = "warn"` | `[workspace.lints.clippy]` | one module style, the one the Book and Reference recommend |
| `unreachable_pub = "warn"` | `[workspace.lints.rust]` | after the split, `pub` means "used by main or tests"; everything else is `pub(crate)` |
| `too-many-lines-threshold = 60` | `clippy.toml` | already active via `pedantic` at 100; 60 keeps functions at today's size |
| `excessive-nesting-threshold = 4` | `clippy.toml` | activates an inert complexity lint at today's maximum depth |
| `disallowed-methods` / `disallowed-types` for `std::process`, `std::fs`, `std::env` | `clippy.toml`, `#![expect(..., reason)]` in adapter and edge modules | domain purity as a compiler diagnostic, mirroring the `print_stdout` pattern |
| remove `module_name_repetitions = "allow"` | `Cargo.toml` | dead line: the lint is in `restriction` on 1.97 |
| `cargo xtask layers` | `xtask`, `just meta`, CI | the import direction, module classification and `#![expect]` placement, which no lint covers |
| every `src/**/*.rs` starts with `//!` naming its layer | folded into `cargo xtask layers` | rust-analyzer's tidy precedent; the first line tells an agent where it is |
| `ARCHITECTURE.md` | repository root | matklad's map: layers, invariants as absences, a coarse codemap; reviewed, not synchronised |
| one row per guard | `docs/harness.md` | the harness rule: a convention is adopted only with its guard |

Not adopted: `cognitive_complexity` (Clippy says it does not measure what its name says),
`redundant_pub_crate` (contradicts `unreachable_pub`), `missing_docs_in_private_items` (56 hits
of noise), `cargo-modules` in CI (checks acyclicity only), a `crates/` split (premature).

### Suggested slice before Lane setup (#29)

1. Library split and lints: `lib.rs`, thin `main.rs`, `pub(crate)`, `unreachable_pub`,
   `mod_module_files`, thresholds, dead allow removed. Behaviour unchanged; the existing suites
   are the guard.
2. Adapters and ops: typed `git` queries, `fs`, `ops/` per command, JSON views in `output`,
   `disallowed_*` lists, `cargo xtask layers`, `ARCHITECTURE.md`, harness rows.
3. Types: the newtypes and enums above, `thiserror` for domain parsers.

## Sources

- The Rust Programming Language: [ch. 7.2](https://doc.rust-lang.org/book/ch07-02-defining-modules-to-control-scope-and-privacy.html),
  [ch. 7.5](https://doc.rust-lang.org/book/ch07-05-separating-modules-into-different-files.html),
  [ch. 9.3](https://doc.rust-lang.org/book/ch09-03-to-panic-or-not-to-panic.html),
  [ch. 11.3](https://doc.rust-lang.org/book/ch11-03-test-organization.html),
  [ch. 12.3](https://doc.rust-lang.org/book/ch12-03-improving-error-handling-and-modularity.html),
  [ch. 18.3](https://doc.rust-lang.org/book/ch18-03-oo-design-patterns.html)
- The Rust Reference: [modules](https://doc.rust-lang.org/reference/items/modules.html),
  [visibility and privacy](https://doc.rust-lang.org/reference/visibility-and-privacy.html)
- The Cargo Book: [package layout](https://doc.rust-lang.org/cargo/guide/project-layout.html),
  [targets](https://doc.rust-lang.org/cargo/reference/cargo-targets.html),
  [cargo doc](https://doc.rust-lang.org/cargo/commands/cargo-doc.html)
- [rustc allowed-by-default lints](https://doc.rust-lang.org/rustc/lints/listing/allowed-by-default.html);
  [rustdoc: what to include](https://doc.rust-lang.org/rustdoc/write-documentation/what-to-include.html),
  [rustdoc command-line arguments](https://doc.rust-lang.org/rustdoc/command-line-arguments.html)
- Rust API Guidelines: [about](https://rust-lang.github.io/api-guidelines/about.html),
  [type safety](https://rust-lang.github.io/api-guidelines/type-safety.html),
  [interoperability](https://rust-lang.github.io/api-guidelines/interoperability.html),
  [future proofing](https://rust-lang.github.io/api-guidelines/future-proofing.html),
  [documentation](https://rust-lang.github.io/api-guidelines/documentation.html),
  [dependability](https://rust-lang.github.io/api-guidelines/dependability.html),
  [naming](https://rust-lang.github.io/api-guidelines/naming.html)
- [Clippy lint list](https://rust-lang.github.io/rust-clippy/master/index.html) (cross-checked on
  Clippy 1.97)
- [std `FromStr`](https://doc.rust-lang.org/std/str/trait.FromStr.html);
  [clap `value_parser!`](https://docs.rs/clap/latest/clap/macro.value_parser.html) (4.6.7)
- [anyhow docs](https://docs.rs/anyhow/latest/anyhow/) and [README](https://github.com/dtolnay/anyhow);
  [thiserror docs](https://docs.rs/thiserror/latest/thiserror/) and [README](https://github.com/dtolnay/thiserror)
- rust-analyzer: [architecture](https://rust-analyzer.github.io/book/contributing/architecture.html),
  [style](https://rust-analyzer.github.io/book/contributing/style.html),
  [`xtask/src/tidy.rs`](https://github.com/rust-lang/rust-analyzer/blob/master/xtask/src/tidy.rs)
- Cargo: [`src/lib.rs` crate docs](https://github.com/rust-lang/cargo/blob/master/src/lib.rs)
- matklad: [ARCHITECTURE.md](https://matklad.github.io/2021/02/06/ARCHITECTURE.md.html),
  [Large Rust Workspaces](https://matklad.github.io/2021/08/22/large-rust-workspaces.html),
  [Inline In Rust](https://matklad.github.io/2021/07/09/inline-in-rust.html)
- [Rust Design Patterns](https://rust-unofficial.github.io/patterns/intro.html) (community book):
  [patterns](https://rust-unofficial.github.io/patterns/patterns/index.html),
  [newtype](https://rust-unofficial.github.io/patterns/patterns/behavioural/newtype.html),
  [RAII guards](https://rust-unofficial.github.io/patterns/patterns/behavioural/RAII.html),
  [strategy](https://rust-unofficial.github.io/patterns/patterns/behavioural/strategy.html),
  [Default](https://rust-unofficial.github.io/patterns/idioms/default.html),
  [Deref polymorphism](https://rust-unofficial.github.io/patterns/anti_patterns/deref.html)
- [Functional Core, Imperative Shell](https://www.destroyallsoftware.com/screencasts/catalog/functional-core-imperative-shell)
  (Gary Bernhardt); [Hexagonal Architecture](https://alistair.cockburn.us/hexagonal-architecture/)
  (Alistair Cockburn); [Parse, don't validate](https://lexi-lambda.github.io/blog/2019/11/05/parse-don-t-validate/)
  (Alexis King)
- [`regexident/cargo-modules`](https://github.com/regexident/cargo-modules)
