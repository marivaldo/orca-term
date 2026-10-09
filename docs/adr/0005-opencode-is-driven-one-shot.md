---
status: superseded by 0009, except the opencode plugin
---

# opencode is driven one-shot, with no `opencode serve`

[ADR 0001](0001-the-core-owns-no-agent-process.md) chose to drive `opencode` as an HTTP + SSE
server we are a client of. A reader will expect that, since opencode's server is its richest
surface: typed `session.error`, `question.asked`, and a native turn-to-diff in `PatchPart`. We
reverse that part of ADR 0001. The core drives opencode exactly the way it drives claude-code: **one
process per turn**, `opencode run --format json [-s <session>] </dev/null`. During takeover, an
**opencode plugin** plays the role claude's hooks play.

The reason is that the server was the one long-lived process left in the design, and nothing of
ours could own it. It needed a reuse, port, health and shutdown story. Its environment froze at
start, and it kept emitting events between turns with nobody listening. Its gains, measured, were
too small to pay for that.

Decided in [issue #14](https://github.com/marivaldo/orca-term/issues/14), which holds the
measurements (opencode 1.18.0).

## Considered options

- **One `opencode serve` shared by the machine.** It is technically sound: one server routes many
  worktrees through a `directory` query parameter or the `x-opencode-directory` header (measured).
  Rejected for three reasons:
  - It is a daemon we spawn and nothing of ours outlives.
  - Its tools inherit the **server's** environment, not the client's (measured: with
    `run --attach`, the bash tool saw `$NVIM` unset), which breaks how
    [the core finds nvim](https://github.com/marivaldo/orca-term/issues/13).
  - It saves only about 0.5 s per turn against a model latency of 3 to 9 s.
- **A `serve` per turn.** Rejected: it starts in about 1.4 s, which is slower than one-shot, and
  pays that only to keep the native patch.
- **One-shot `opencode run`.** Chosen. It emits typed NDJSON (`step_start`, `text`, `tool_use`,
  `step_finish`, `error`) with the `sessionID` on every line, and `-s` carries the conversation. It
  exits 1 on error, and permissions are auto-rejected without hanging, the same as `claude -p`.

## Consequences

- **There is no opencode daemon**, so there is nothing to discover, health-check or shut down, and
  no events can be orphaned between turns.
- **No native turn-to-diff for either agent.** `run` emits no patch or diff (the stored `patch`
  part is reachable only through a server), so both adapters derive a turn's diff from git.
- **Weaker error typing.** `run`'s `error` line can carry a generic `UnknownError` where the
  server would have said `ProviderModelNotFoundError`. A failed turn's detail is less precise.
- **The core must close stdin.** With a non-TTY stdin, `run` reads to EOF and hangs forever on an
  open pipe (measured).
- **A core-driven opencode turn never waits.** As with claude, a mid-turn question or permission
  request is only answerable under takeover.
- **Takeover is `:terminal opencode -s <id>`**, with `OPENCODE_CONFIG_DIR` pointing at a plugin
  directory of ours **outside the repo**. Measured in the TUI: the plugin loads, receives
  `session.idle` and the other bus events, and its process inherits `$NVIM`. The plugin hands
  events to the core the way claude's hooks do. Its first load npm-installs `@opencode-ai/plugin`
  into that directory, taking about 12 s.
