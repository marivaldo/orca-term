---
status: accepted
---

# orca-term is a terminal application, with the person's Neovim beside it as the editor

Map #1 closed with Neovim as orca-term's one client: the sidebar, the status strip, a tab for each
worktree and the agent's terminal all lived inside Neovim. On 2026-11-06 the driver said that was
not what they wanted. orca-term should be "an Orca, as a terminal application": one program in the
terminal, laid out as Orca is (and as VS Code is), with panels arranged as tmux arranges panes. The
second map, [issue #165](https://github.com/marivaldo/orca-term/issues/165), decided how.

- **orca-term draws its own screen** ([#168](https://github.com/marivaldo/orca-term/issues/168)),
  in Rust with ratatui and crossterm, as `orca-term app` in the same binary as the core
  ([#171](https://github.com/marivaldo/orca-term/issues/171)). It calls the core directly; the
  application is a layer of the crate above the operations (ADR 0010).
- **The layout is Orca's** ([#170](https://github.com/marivaldo/orca-term/issues/170)): a side
  menu with the projects and their worktrees on the left, the selected worktree's terminals in the
  centre (`1 agent`, `2 nvim`, `3 shell`, a split of agent and Neovim, and a zoom), a right column
  with the changed files and the pull request's checks, and a status bar. The look is the "Quiet
  v2" design, kept with the prototype on the branch `prototype/terminal-application`
  (`prototype/terminal-application/design/`).
- **Keys go to the focused panel.** A prefix, `Ctrl-a`, arms the application's own keys: `h` and
  `l` move focus, `j` and `k` switch worktree, `z` zooms, `|` splits, `w` opens the picker, `q`
  quits, and `a` sends a literal `Ctrl-a`.
- **Each terminal of the centre is fed from the agent's holder** (ADR 0009). The holder keeps the
  agent alive without any client, so quitting the application stops nothing.
- **Neovim is the editor, the person's own** ([#166](https://github.com/marivaldo/orca-term/issues/166),
  [#172](https://github.com/marivaldo/orca-term/issues/172)). Each worktree has its own Neovim,
  shown in the `2 nvim` tab or a split, or run by the person in a separate terminal window and
  found through the registry. Diff review and notes stay in the Lua client
  ([#169](https://github.com/marivaldo/orca-term/issues/169)); Enter on a changed file opens its
  diff there. Claude Code's own editor protocol links the agent and Neovim through
  coder/claudecode.nvim, with claude started `--ide`.
- **The multiplexer stays out of the architecture.** ADR 0001's rejection of tmux holds: "panels
  like tmux" means behaving like tmux, not running it.

This record supersedes map #1's "the destination ships exactly one client (nvim)". It does not
change any other ruling of the core.

## Considered options

- **tmux or zellij arranged by us**, our side menu in one pane and the agents in the others.
  Rejected: the research ([#167](https://github.com/marivaldo/orca-term/issues/167)) found that
  tools built this way use tmux mainly to keep agents alive, which the holder already does; it
  reopens ADR 0001 and ADR 0009, adds a hard dependency, and splits the look and the keys.
- **Keep Neovim as the one client**, drawing the panels in Neovim's windows. Rejected by the driver:
  it was the result that "não está ficando como eu queria".
- **Replace Neovim whole**, with an editor drawn by the application. Rejected: the driver wants a
  real editor beside the agent, to search files and read diffs, and that editor is their Neovim.
- **A second binary** talking to the core over `--json`. Rejected: two binaries to install and one
  more contract to keep, for the sole gain of isolating ratatui's dependencies.

## Consequences

- The application is delivered in slices (#171), the first being the side menu with real data and
  the agent's terminal attached to its holder. The holder learns an attach that is not bound to the
  caller's stdin, one terminal size for several clients, and the terminal's modes replayed on
  attach.
- `orca-term` with no arguments keeps opening Neovim with the sidebar until the application covers
  what the sidebar does; then it opens the application, and `orca-term open` keeps opening Neovim.
- The parts of the Lua client the application takes over (the sidebar, the status strip, a tab per
  worktree, the agent's terminal inside Neovim) are frozen from 2026-11-06: fixes only, no new
  behaviour. They and their specs are removed once the application covers them.
- ratatui and crossterm join `deny.toml`'s review; the application's screens are tested as
  snapshots on ratatui's `TestBackend`, and one live test runs it in a terminal.
