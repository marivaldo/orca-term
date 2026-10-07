# orca-term

Orchestrate a fleet of parallel CLI coding agents from the terminal, with Neovim as the editor.

Inspired by [Orca](https://github.com/stablyai/orca), but without the Electron app: each agent
runs in its own git worktree, and review happens in the tools already living in the terminal.

## Status

Not built yet. The route is being charted as a [wayfinder](https://aihero.dev/skills-wayfinder)
map in this repo's issues — see the issue labelled `wayfinder:map`.

Nothing here is settled until that map clears.

## Scope

In: parallel agents in isolated worktrees, diff review that can feed comments back to the agent
that wrote the code, Neovim integration over a standalone core.

Out: mobile companion, cloud relay, screenshot-driven design mode, Linear integration,
worktrees over SSH.
