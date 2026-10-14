# Architecture

This file is the map of `orca-term`, its core and its Neovim client: where each concept lives and the
rules the code keeps.
Read it before changing the code; update it when a module moves. The words follow the glossary in
`CONTEXT.md`, and the decision behind the shape is ADR 0010 (`docs/adr/`).

## Bird's-eye view

`orca-term` gives a person a fleet of git worktrees, each on its own branch, for parallel coding
agents. The core is a Rust command-line program: each command reads the repository through git and
the file system, decides what to do with pure rules, does it, and prints the result as text for a
person or as JSON for the Neovim client. A worktree's agent (claude, for now) runs inside a
detached process of the core, its holder, which keeps the agent's terminal and screen so that any
terminal can attach to it and detach again (ADR 0009), under claude's native sandbox (ADR 0007).

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

- `cli.rs`: the clap types for every command and flag (`worktree ls|new|setup|rm|prune`,
  `agent start|attach|stop`, and the hidden `agent holder`, the holder process itself), the
  dispatch to `ops`, and the exit code of a command that did its job (1 when a setup failed).
- `output.rs`: the only writer to stdout and stderr; renders the fleet table, the reports of
  `worktree new`, `setup`, `rm` and `prune`, of `agent start` and `stop` (and, on stderr, the
  sandbox's extra writable paths with the config file that set them, and how an attach ended),
  and errors (as text on stderr, or as JSON on stdout with `--json`). `Format` is text or JSON. Before a command that may run a setup starts, `cli`
  has `output` say on stderr what it starts (`creating worktree fix...`); `live_echo` is the
  writer the setup's output is then echoed to while it runs (stderr, or nowhere with `--json`),
  handed by `cli` to the ops module.
- `output/json.rs`: the JSON documents of `--json` (the fleet, `worktree new`, `worktree setup`,
  `worktree rm`, `agent start`, `agent stop` and errors), built as views from domain values, with the `contract`
  and `version` envelope. Each worktree carries `agent` (`"claude-code"` or null) and `holder`
  (whether a holder is alive).

### ops

- `ops.rs`: what several commands share: reading the fleet and deriving each worktree's state,
  agent and holder, finding the worktree a command names, taking a worktree's lock, refusing a
  change under a live holder, writing `worktree.json` (the setup or the agent, keeping the other),
  reading the config files, and running a worktree's setup.
- `ops/worktree_new.rs`: `worktree new`: check the name, resolve the config, add git's worktree,
  lock it, copy the copy list, run the setup.
- `ops/worktree_setup.rs`: `worktree setup`: find the target, lock it (or fail as `busy`), run
  the setup again.
- `ops/worktree_ls.rs`: `worktree ls`: the fleet of the current repository.
- `ops/worktree_rm.rs`: `worktree rm`: find the target, lock it (or fail as `busy`, even with
  `--force`), refuse modified or untracked files without `--force` (the typed `dirty`), decide
  the branch's fate, remove, delete or keep the branch.
- `ops/worktree_prune.rs`: `worktree prune`: `git worktree prune` and what it cleaned.
- `ops/agent_start.rs`: `agent start`: find the target (ready, not the primary checkout), lock it
  (or fail as `busy`), report a holder already alive, probe the sandbox (or fail as
  `no_sandbox`), find claude on PATH and check its version, resolve the extra writable paths,
  pre-create the admin directories, build and check the settings and the command line, write
  `claude-settings.json`, start the holder and wait for its first word, record the agent.
- `ops/agent_attach.rs`: `agent attach`: find the target, refuse when no holder is alive (naming
  `agent start`), connect, hand the terminal to the holder until detach or the agent's end.
- `ops/agent_stop.rs`: `agent stop`: find the target, nothing to do when no holder is alive, else
  ask the holder to stop and wait for `agent.lock` to be released (at most 10 s).
- `ops/agent_holder.rs`: the hidden `agent holder`: the holder process, `adapters/holder/process.rs`.

### adapters

- `adapters/git.rs`: every git process, one function per question (`worktree_list`,
  `default_branch`, `is_merged`, ...) or change (`worktree_add`, `delete_branch`, ...).
- `adapters/fs.rs`: reading and atomically writing files, plain byte copies, creating and listing
  directories, resolving paths through symlinks, finding an executable in the PATH directories,
  and locks: a worktree's `WorktreeLock`, taken without waiting, the holder's `FileLock` on
  `agent.lock`, and the read-only probes `ls` uses for both.
- `adapters/script.rs`: the setup script's process, `bash -e -c <script>` in the worktree, its
  stdout and stderr merged through one pipe and copied to the setup log and the live echo.
- `adapters/env.rs`: the current directory, `HOME`, `XDG_CONFIG_HOME` and the directories of
  `PATH`.
- `adapters/program.rs`: running a program for its `--version`.
- `adapters/holder.rs`: starting a holder as a detached child with a pipe for its first word
  (`ok <pid>` or `error <why>`, 10 s at most), connecting to a holder's socket, and asking it to
  stop. The socket is bound and connected by its bare name after a change of directory into the
  admin directory, since a unix socket address holds about 100 bytes.
- `adapters/holder/process.rs`: the holder process: a session of its own (`setsid`), the
  `flock` on `agent.lock` (retried for half a second, since `ls` probes it), the socket, the PTY
  and the agent on it, `agent.json`; then, until the agent exits, its threads: the main thread
  reaps the agent, the reader pumps the PTY into the scrollback (8 MiB) and to every client's
  bounded queue, the acceptor starts a client thread per connection, each client has a writer
  (replay first, then the queue), and a stop spawns the escalation (hang up, terminate after 3 s,
  kill after 3 more); a write to a client that blocks for 3 s drops that client as stuck, and
  the writers are joined for at most 5 s after the agent exits. A client's first size is "jiggled" (one column narrower, then back, again
  a second later) so the agent redraws for the new terminal. On the agent's exit every client
  gets `x <exit>`, the socket and `agent.json` go, and the process exits, releasing the lock.
- `adapters/terminal.rs`: the terminal of `agent attach`: raw mode (restored on the way out,
  followed by a reset of the screen modes an agent leaves behind), the size sent on connect and on
  every `SIGWINCH`, what is typed forwarded as input (Ctrl-\ detaches unless `--no-detach-key`),
  the holder's output written to stdout. Without a terminal it only moves bytes, and the end of
  its input detaches.

### domain

- `domain/worktree.rs`: a worktree's name rules and path, which worktree a `rm` target means,
  and the branch's fate on removal.
- `domain/branch.rs`: a branch's name, the start point a new branch is created from, and git's
  rules for a valid branch name, mirrored without running git.
- `domain/fleet.rs`: the fleet derived from git's list: the primary checkout apart, names,
  gone worktrees, what a prune cleaned.
- `domain/porcelain.rs`: parsing `git worktree list --porcelain -z`.
- `domain/config.rs`: `orca-term.yaml` parsing (`base`, `scripts.setup`, `sandbox.writable`),
  the precedence of the config files, which checkout supplies the committed file (the primary
  checkout for `base`, the worktree for `scripts.setup` and `sandbox.writable`), `~` and relative
  path expansion.
- `domain/state.rs`: a worktree's state and why it is broken or not ready, derived from
  `worktree.json`, the lock and the setup log; its admin directory and the files in it; and the
  `worktree.json` schema rules and bytes.
- `domain/agent.rs`: the agent a worktree runs (`Agent::ClaudeCode`, recorded as `claude-code`,
  run as `claude`), whether its holder is alive, the holder's record (`agent.json`), a terminal
  size, and the reports of `agent start`, `stop` and `attach`.
- `domain/launch.rs`: claude's launch: the command line (`--settings <file>
  --permission-mode acceptEdits`), the settings file that turns its sandbox on and denies our
  state (ADR 0007), the version floor (2.1.285), the variables stripped from its environment, and
  the guard that refuses a launch bypassing permissions. The forbidden flags and modes are
  listed here and nowhere else.
- `domain/sandbox.rs`: whether claude's sandbox can run here: `sandbox-exec` on macOS, `bwrap`
  and `socat` on Linux, nothing elsewhere; missing means the typed `no_sandbox`.
- `domain/protocol.rs`: the frames between a holder and its clients (a tag byte, a 4-byte
  big-endian length, the payload: input `i`, resize `r`, stop `s`, output `o`, exit `x`), their
  incremental decoder, and the scrollback a holder keeps.
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
- `State` (`domain/state.rs`): `NoAgent`, `Idle`, `SettingUp`, `NotReady(NotReadyReason)` or
  `Broken(BrokenReason)`. A ready worktree is `Idle` when an agent is recorded (whether or not
  its holder is alive, until hooks report turns) and `NoAgent` otherwise. `NotReadyReason` is
  `Interrupted` (a setup recorded as running whose lock nobody holds) or `Failed` with its `Exit`
  and the last line of its log. `BrokenReason` says the directory is missing, git marks it
  prunable, or the state file cannot be read. Broken wins over every other state.
- `AdminDir` (`domain/state.rs`): the directory holding a worktree's state: its `worktree.json`,
  its `setup.log`, its `lock`, and the holder's files `agent.lock`, `agent.json`, `agent.sock` and
  `claude-settings.json`.
- `Agent`, `HolderStatus`, `HolderRecord`, `Size` (`domain/agent.rs`): the agent kind, whether a
  holder is alive (`Alive`/`Gone`, from the probe of `agent.lock`), what `agent.json` holds (pid
  and socket path, volatile), and a terminal size.
- `StartOutcome`, `Started`, `StopOutcome`, `Stopped`, `AttachEnd` (`domain/agent.rs`): what the
  agent commands did and report.
- `Frame`, `Decoder`, `Scrollback` (`domain/protocol.rs`): the holder protocol's messages, their
  decoder and the replay buffer.
- `Platform` (`domain/sandbox.rs`): macOS, Linux or other, which decides the sandbox tools.
- `LockStatus` (`domain/state.rs`): `Held` or `Free`, what the read-only probe of a worktree's
  lock found.
- `SetupRecord` and `Exit` (`domain/setup.rs`): what `worktree.json` records about the setup,
  `Running`, `Succeeded` or `Failed(Exit)`, where `Exit` is a non-zero `Code` or a `Signal`.
- `SetupOutcome`, `SetupReport` and `SetupDone` (`domain/setup.rs`): what running the setup did
  (`NoScript`, `Succeeded`, `Failed`), the report of a command that ran it (the script and its
  source, the log, the state it left), and what `worktree setup` did.
- `TypedError` (`domain/error.rs`): the failures a client tells apart: `Busy` (with a
  `BusyReason`: the lock is held, or the agent is running), `NoSandbox` and `Dirty` (a worktree
  `worktree rm` keeps because it has modified or untracked files).
- `WorktreeLock` (`adapters/fs.rs`): proof that this process holds a worktree's lock. It can
  only be made by taking the lock, and dropping it releases the lock. `FileLock` is the same for
  the holder's `agent.lock`.
- `DetachKey` (`adapters/terminal.rs`): whether Ctrl-\ detaches an attach, or nothing does.

## Invariants

- **The domain is pure.** Nothing in `src/domain/` touches the file system, starts a process,
  reads the environment or prints. It takes strings, bytes and parsed values, and returns values,
  decisions or errors. That is why it is unit-tested without a repository.
- **Only adapters touch the outside world.** Only `adapters/git.rs` starts git, only
  `adapters/script.rs` starts the setup script, only `adapters/holder.rs` starts the holder (and
  only `adapters/holder/process.rs` starts an agent, on a PTY), only `adapters/program.rs` runs a
  program for its version, only `adapters/fs.rs` reads or writes files, and only
  `adapters/env.rs` reads the environment.
- **Only `output` prints**, and only `output/json.rs` knows the JSON shape of the contract.
  Domain types never derive `Serialize` for output. A setup's live echo is a writer `output`
  makes and `cli` hands down, so ops and adapters never name stdout or stderr. Two adapters move
  bytes through stdout without printing a report: the holder writes its one-line first word on
  the pipe `agent start` gave it (then points stdout at `/dev/null`), and `adapters/terminal.rs`
  writes the agent's screen to the attaching terminal.
- **Each command is one ops module**: a short sequence of adapter and domain calls, readable top
  to bottom. Shared steps go in `ops.rs`.
- **Dependencies point down**: main > edge > ops > adapters > domain. A module uses its own layer
  or a lower one, through `crate::` paths, never a higher one.
- **Nothing writes on a read path**: `worktree ls` only reads. It probes a worktree's lock with
  a non-blocking try that it releases at once, reads the setup log only for a failed setup, and
  never rewrites an interrupted setup (ADR 0003: volatile facts are checked on read).
- **Writes to a worktree's state hold its lock.** The functions that write `worktree.json`
  (`ops::write_state`, `ops::write_agent`) take a `&WorktreeLock`, so a write without the lock
  does not compile, and each keeps what the other records: a setup never erases the agent.
  `worktree ls` reads a setup recorded as running once more when its lock is free, since the
  setup records its end before releasing the lock. `worktree new` takes the lock as soon as
  git's worktree exists and holds it until the setup
  ends; `worktree setup` and `worktree rm` take it without waiting and fail as `busy` when it is
  held.
- **A failed setup undoes nothing**: the worktree, its branch and its log stay, and the worktree
  is not ready until `worktree setup` succeeds.
- **One agent process per worktree, and its pid and socket are never trusted on their own**
  (ADR 0003, ADR 0009). The holder holds an exclusive `flock` on `agent.lock` for its whole life;
  `agent.json` (its pid and socket) and `agent.sock` are believed only while that lock is held,
  so a holder killed outright leaves files behind but no false liveness. `agent start` under the
  worktree's lock reports a live holder instead of starting a second one, and the holder itself
  refuses to run when another holds `agent.lock`. `worktree rm` and `worktree setup` fail as
  `busy` while a holder is alive, naming `agent stop`. `agent start` refuses a worktree that is
  not ready, and the primary checkout.
- **The agent is launched only through `domain/launch.rs`.** The command line is
  `claude --settings <admin>/claude-settings.json --permission-mode acceptEdits`, in the worktree,
  with the environment of `agent start` minus `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`,
  `CLAUDE_CODE_SESSION_ID` and `CLAUDE_CODE_CHILD_SESSION` (so an orca-term run from inside a
  Claude Code session never starts a child session). The settings turn claude's sandbox on with
  `failIfUnavailable`, `allowUnsandboxedCommands: false` (which also makes the repository's own
  `.claude/settings*.json` unable to loosen it, on claude 2.1.285 or newer, the floor `agent
  start` checks), every weakening key pinned, the network open, writes denied to every worktree's
  `orca-term` admin directory (each one listed, plus the `*` glob), `refs/orca-term` and
  `orca-term.yaml` in the git common dir, through both the sandbox's `denyWrite` and `Edit(...)`
  deny rules; the only configurable part is `sandbox.writable` in `orca-term.yaml`, extra
  writable paths, the highest config layer replacing the lower ones and printed with its source.
  `launch::check_launch` refuses, by whole argument and parsed value, `--dangerously-skip-permissions`,
  `--allow-dangerously-skip-permissions`, `--auto`, any `--permission-mode` but `acceptEdits`,
  and settings whose `permissions.defaultMode` bypasses, before any holder is started; the
  literals exist in that file alone (`cargo xtask launch-guard`). Without the sandbox's tools on
  PATH (`sandbox-exec`; `bwrap` and `socat`) nothing is launched: the typed `no_sandbox`.
- **The holder is agent-agnostic**: it runs the command line it is given, in the directory it is
  given, and knows nothing of claude. Building that command line is domain code.
- **Errors** are `anyhow` errors with context, written for a person. Domain parsers return small
  `thiserror` enums (`WorktreeNameError`, `BranchNameError`) whose messages a person can read.
  Failures the client must tell apart are `TypedError` cases (`domain/error.rs`), carried inside
  the `anyhow` error and found again by downcasting. Without `--json` an error is printed on
  stderr as `orca-term: <message>`; with `--json` it is a JSON document on stdout:
  `{"contract": 1, "version": "...", "error": {"kind": "...", "message": "..."}}`, where `kind` is
  the typed error's (`busy`, `no_sandbox`, `dirty`) or `other`.
- **Exit codes**: 0 on success; 1 for an untyped failure, and for `worktree new` and
  `worktree setup` when the setup failed (the worktree exists and the normal report says it is
  not ready); 2 for a usage error (clap); 3 for `busy`, a worktree whose lock another process
  holds or whose agent is running; 5 for `no_sandbox`, a machine where claude's sandbox cannot
  run; 6 for `dirty`, a worktree `worktree rm` refuses without `--force` because it has modified
  or untracked files (4 is kept for a later slice).

## Known limits of the agent holder

- **No socket peer check on Linux.** Without claude's optional seccomp helper, a sandboxed
  command may connect to any worktree's `agent.sock` (connecting to a socket on a read-only bind
  mount is allowed) and type into another agent. macOS blocks unix sockets by default. A later
  slice may have the holder refuse peers from another network namespace.
- **The person's own `~/.claude/settings.json` can widen the sandbox**: array keys such as
  `sandbox.excludedCommands`, `sandbox.filesystem.allowWrite`, `permissions.additionalDirectories`
  and `Edit(...)` allow rules merge across scopes, and hooks, MCP servers and status lines run
  unsandboxed. This is accepted as the person's own configuration; the repository's files cannot
  do the same. Managed (organisation) settings override ours.
- **On Linux, `denyWrite` names only the admin directories that exist at launch**: the `*` glob
  is silently skipped there, so a worktree created after the launch is not covered until the
  agent is started again (macOS honours the glob).
- **`git push` over SSH fails inside claude's sandbox on macOS**, even with the network open;
  HTTPS works. The person pushes from outside, or uses HTTPS.
- **A replay may miss early terminal modes**: the scrollback is raw bytes, so a truncated replay
  can start mid escape sequence, and a mode switched on before the kept bytes (bracketed paste,
  mouse, cursor visibility) is not replayed; the jiggle redraws the screen, not the modes. On
  detach the client resets the usual modes.
- **Paths with glob characters** (`*`, `?`, `[`) in the repository's path are read as globs by
  claude's `denyWrite` on macOS.
- **The real sandbox boundary is not exercised by the tests**: a fake claude enforces nothing.
  Seam 1 asserts the generated settings and command line; the boundary with the real claude waits
  for the live checks of #44.

## Guards

Each invariant above is checked, not just written down (see `docs/harness.md`):

- Clippy `disallowed-methods` and `disallowed-types` (`clippy.toml`) reject processes, file
  access and environment reads everywhere; each adapter opts out with a module-level
  `#![expect(..., reason = "...")]`.
- `cargo xtask layers` rejects an upward `crate::` reference, a file whose first line is not a
  `//!` naming its layer (`//! Domain: ...`, `//! Adapter: ...`, `//! Ops: ...`, `//! Edge: ...`,
  `//! Main: ...`), and the Clippy opt-out outside adapters. Its table of modules and layers is
  `MODULES` in `xtask/src/layers.rs`; a new top-level module is added there on purpose.
- `cargo xtask launch-guard` (`xtask/src/launch_guard.rs`) rejects the literals
  `--dangerously-skip-permissions`, `--allow-dangerously-skip-permissions`, `--auto` and
  `bypassPermissions`, as whole tokens outside comments, in every file under `src/`, `lua/` and
  `plugin/` but `src/domain/launch.rs`, where the run-time guard lists them.
- Clippy `print_stdout` and `print_stderr` keep printing in `output`.
- The compiler keeps writes to a worktree's state under its lock: `ops::write_state` and
  `ops::write_agent` take a `&WorktreeLock`, which only taking the lock can make.

## Tests

- **Seam 1, black box**: `tests/worktree_*.rs` and `tests/agent.rs` run the binary against
  throwaway git repositories built by `tests/support/mod.rs`, and check what a person or the
  client sees. `tests/agent.rs` runs it with `PATH` set to one directory
  (`tests/support/agent.rs`) holding a fake `claude` (a bash script that records its arguments,
  directory and environment variable names, echoes what is typed and exits on request), symlinks
  to the real `git` and `bash`, and fake `sandbox-exec`, `bwrap` and `socat`; attaches over
  pipes, where the end of stdin detaches; and stops every holder when a test ends. It needs no
  claude, no sandbox and no network.
- **Unit tests** sit next to the pure code they test, in a `#[cfg(test)] mod tests` at the end of
  each domain and edge module. Adapters keep the few tests that need a real directory.
- **Seam 2, the Neovim client against the real core**: `spec/*_spec.lua` run busted via nlua in
  headless Neovim against the binary `cargo build` makes, in throwaway git repositories built by
  `spec/support.lua`. The sidebar's text and highlights are compared with
  `spec/golden/sidebar.txt` (`UPDATE_GOLDEN=1 just lua-test` rewrites it). See "Neovim client"
  below.
- **Lints** have their own unit tests in `xtask/src/`.

## Neovim client

The Neovim client is Lua at the repository root, beside the crate (ADR 0008), laid out as a
Neovim plugin: Neovim runs `plugin/` at startup and `require` finds the modules in `lua/`. It needs
Neovim 0.11 or later. The client is thin: it runs the core's commands with `--json` and shows
what they print.

- `plugin/orca-term.lua`: defines the `:OrcaTerm <subcommand>` command, with completion of the
  subcommands, and the `<Plug>(OrcaTermSidebar)` map, and nothing else.
- `lua/orca-term/init.lua`: `setup(opts)` and `config`, the settings (so far `binary`, the path to
  the core binary). Calling setup() is optional; `config` holds the defaults until then.
- `lua/orca-term/core.lua`: the link to the core. `find_binary` takes `binary` from setup(), else
  `target/release/orca-term` or `target/debug/orca-term` in the repository the client is loaded
  from, else `orca-term` on PATH. Each command (so far `worktree_ls`) runs the binary with
  `--json`, waits for it, decodes the JSON and refuses a document whose `contract` differs from
  the client's own `CONTRACT`. It returns the document, or `nil` and an error `{ kind, message }`
  whose message names the fix; it never throws.
- `lua/orca-term/fleet.lua`: the attention order, pure: each worktree's rank (1 waiting to 8 no
  agent, from the spec) and the sections *Needs you*, *Running* and *Quiet*, sorted by rank then
  name (recency joins when the core prints it).
- `lua/orca-term/render.lua`: the sidebar's look, pure: turns the fleet (or an error), the
  selected worktree's path and a width (36) into lines, highlight spans and the worktree each line
  belongs to. It draws prototype variant D: the header with the primary checkout, the sections, a
  glyph and a second line per worktree, the rounded card around the selected one. No line reaches
  past the width.
- `lua/orca-term/highlights.lua`: the highlight groups (`OrcaTermHeader`, `OrcaTermMuted`,
  `OrcaTermBorder`, `OrcaTermError` and one `OrcaTermState*` per state), defined as default links
  to `Title`, `Comment`, `FloatBorder` and `Diagnostic*` so a person's own definition wins.
- `lua/orca-term/sidebar.lua`: `:OrcaTerm sidebar`. One scratch buffer, `orca-term://sidebar`,
  shown in a 36-column left split of every tab while open (and of every tab made later). It reads
  the fleet through `core.lua` on open and on `FocusGained`, paints what `render.lua` returns, and
  keeps the selection (a worktree's path) across refreshes. Its only keys, `j` and `k`, are local
  to its buffer. A tab is never left holding only the sidebar: on `QuitPre` for the tab's last
  other window it closes the tab's sidebar first, and on `WinClosed` it closes a tab left with
  only the sidebar (unless it is the last tab).

Its rules:

- **Only `core.lua` talks to the core.** It is the only Lua file that starts a process; every
  other module calls it. `cargo xtask client-boundary` refuses a process started anywhere else in
  `lua/` or `plugin/`.
- **Only `sidebar.lua` touches windows and buffers; `fleet.lua` and `render.lua` are pure.**
  The order and the look are plain functions of the fleet, tested without a window; the sidebar
  only paints them. `cargo xtask client-boundary` refuses a window or buffer call anywhere else,
  and any `vim.api` or `vim.cmd` in the pure modules.
- **The client never computes state.** The fleet, each worktree's state and every decision come
  from the core's JSON; the client renders them. No logic is bound to the Neovim process.
- **No global keymap.** Every key the client adds is local to one of its buffers;
  `spec/plugin_spec.lua` checks that loading the client defines no global keymap.
