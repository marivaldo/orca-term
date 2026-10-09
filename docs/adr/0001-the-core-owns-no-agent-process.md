---
status: accepted, partially superseded by 0005 and 0009
---

# The core owns no agent process

A reasonable reader will assume this project holds the agents it orchestrates: the prior art it
resembles (`stablyai/orca`) drives agents through an extensive `node-pty` layer, and "orchestrator"
normally implies owning the processes. It does the opposite. The core owns **no PTY and runs no
daemon of its own**: it is a library plus a CLI, invoked once per turn, with nvim as the only
long-lived process. Agents are driven through their own structured surfaces — `claude-code` as a
one-shot `claude -p --resume <uuid>` process per turn, `opencode` as an HTTP + SSE server we are a
client of — behind one internal interface with one adapter each.

Decided in [issue #6](https://github.com/marivaldo/orca-term/issues/6), which holds the measurements.

## Considered options

- **Own the PTYs in a daemon of ours.** Rejected: it would duplicate, for one agent, the daemon
  Claude Code already ships (`~/.claude/daemon/roster.json`, owning PTYs over authed unix sockets),
  and be pure overhead for `opencode`, which needs no PTY owned at all.
- **Let a multiplexer own them** (a tmux pane per agent). Rejected: tmux's one decisive advantage
  over kitty was surviving detach, and that advantage protects a long-running agent process. Under
  one-shot drive no such process exists — the conversation survives as a `sessionId`, not as a PTY
  — so tmux buys nothing it is needed for while adding a hard dependency.
- **Drive `claude-code` as a live background session** (`claude --bg` + `claude --resume`).
  Rejected on two measured grounds: `--bg` is refused by the per-directory workspace-trust gate,
  which every new worktree trips, and a piped orchestrator cannot inject into a live background
  session at all — `--bg --resume` on a running session forks a copy instead.
- **Uniformity across both agents**, either as a lowest-common-denominator one-shot or via ACP.
  Rejected: the first discards `opencode`'s SSE, typed errors and `PatchPart`, the only native
  turn-to-diff in either agent; the second would route the primary agent through a third-party
  bridge no primary source confirmed. The map's agent horizon is closed at two agents, so two
  adapters is a bounded cost.

## Consequences

- **PTY handling is not a requirement of the core**, which removes it as a criterion for choosing
  the core's language, and makes process startup latency a first-order one instead.
- **Durable on-disk state is mandatory.** Nothing of ours survives a turn, so "keep it in the
  daemon's memory" is not available, and concurrent short-lived writers need atomic writes.
- **Between turns there is no live process to ask** what the fleet is doing: `claude agents --json`
  lists only live sessions. Any view of fleet state reads what was persisted.
- **Takeover is exclusive, by necessity.** Resuming one session in two places interleaves both
  parties' messages into one transcript, which would corrupt the turn attribution the review loop
  depends on. So while the human holds a session, the core does not drive it.
- **`opencode serve` becomes a daemon we start but do not outlive**, which needs its own
  reuse-and-health story.

> **Superseded in part by [ADR 0005](0005-opencode-is-driven-one-shot.md)**: `opencode` is no
> longer driven through `opencode serve` but one-shot, with `opencode run` per turn, so the
> server, its daemon story and the "uniformity rejected" option above no longer apply to it.
