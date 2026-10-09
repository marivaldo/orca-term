---
status: accepted
---

# A lane runs its agent's interactive TUI, held by a detached process of ours

Opening a lane shows the agent's own screen, as Orca does: Claude Code or opencode, interactive, in
a Neovim terminal that fills the lane's tab, where the person types directly. This is the **only**
way a lane's agent runs. The core no longer drives headless one-shot turns (`claude -p`,
`opencode run`), so what [ADR 0001](0001-the-core-owns-no-agent-process.md) and
[ADR 0005](0005-opencode-is-driven-one-shot.md) chose for driving agents is reversed, and what the
map called takeover becomes the normal case.

The agent must survive Neovim quitting, as it survives Orca quitting. The `orca-term` binary
therefore **holds each agent's PTY in a detached process, one per lane**, in the manner of dtach.
That process lives exactly as long as the agent, and is not a daemon serving many lanes. Neovim's
terminal only attaches to it, and reattaching replays the scrollback. If the holder is gone, after
a reboot for instance, opening the lane starts the agent again on its current session with
`--resume`.

Every agent runs inside the OS sandbox of [ADR 0007](0007-core-driven-turns-run-in-an-os-sandbox.md),
with everything allowed inside it, so the person is not asked for permissions inside the lane and
the agent still cannot write outside it. On a machine without the sandbox, the lane refuses to
start its agent.

Status, turn bounds and snapshots come from hooks injected per launch (`claude --settings` and the
opencode plugin through `OPENCODE_CONFIG_DIR`), as takeover had designed: a turn starts when the
person submits a prompt and ends when the agent stops, and
[ADR 0006](0006-turns-are-bounded-by-tree-snapshots-under-a-shared-ref.md) bounds it unchanged.

Decided in [issue #26](https://github.com/marivaldo/orca-term/issues/26).

## Considered options

- **Keep headless one-shot turns as the primary mode, with the TUI only under takeover.**
  Rejected: selecting a running lane would show a summary of the agent instead of the agent, which
  is the opposite of the Orca experience this project follows.
- **Interactive by default, with headless `p` turns kept for lanes whose screen is closed.**
  Rejected: two ways to run a turn means two ways to bound it, report its status and sandbox it.
  Headless can return later if it is missed.
- **Let tmux hold the PTYs.** Rejected: it makes tmux a hard dependency, which ADR 0001 had already
  refused, for something one small process of ours does.
- **Let the agent die with Neovim and resume its session on reopen.** Rejected: closing the editor
  would interrupt work in progress, unlike Orca.
- **Orca's Yolo/Manual toggle, or the agent's own permission settings.** Rejected for the reasons
  in ADR 0007: nothing confines the agent to its worktree, and with many lanes the person is not
  watching every screen to answer prompts.

## Consequences

- **The core now owns PTYs**, one holder process per running agent. It still runs no daemon: each
  holder is independent, and state stays on disk, in git's admin dir, as
  [ADR 0003](0003-lane-state-lives-in-gits-worktree-admin-dir.md) requires. The holder's pid and
  socket are volatile facts, written but never trusted.
- **Takeover disappears as a concept.** There is no exclusive handover, since nothing else drives
  the lane. The `T` and `p` keys go away. `⏎` opens the agent's terminal and focuses it, and the
  review moves to `d`.
- **Notes are pasted into the agent's terminal**, in Orca's format, as a prompt the person sends.
- **Resuming a past session restarts the agent in its holder** with `--resume <id>`. Creating a lane
  starts its agent.
- **Denials the agents report** remain a count on the finished turn, read from the hooks.
- **The harness must test the holder:** detaching and reattaching with the scrollback intact,
  surviving Neovim's exit, cold restart with `--resume`, and the sandbox boundary for each agent
  under the interactive launch.
