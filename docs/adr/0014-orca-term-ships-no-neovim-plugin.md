---
status: accepted
---

# orca-term ships no Neovim plugin; the core opens a diff in Neovim over its socket

ADR 0012 kept the Lua client for the diff review and the notes
([#169](https://github.com/marivaldo/orca-term/issues/169)): Enter on a changed file in the
application's right column asked the worktree's Neovim to open the file in the client's review,
diffview.nvim in a tab of its own. On 2026-11-11 the driver decided the terminal application is
where orca-term goes on, code review included: "acho que até código vamos seguir baseado neste app
novo, podemos remover esse que usava o neovim". Asked whether to remove only what the application
replaced or the whole client, they chose the whole client: "não vamos usar isso mais" (we will
not use it any more), and the sidebar becomes obsolete ([#194](https://github.com/marivaldo/orca-term/issues/194)).

- **The Lua client is removed whole**: `lua/`, `plugin/`, `spec/`, the lazy.nvim build hook
  `build.lua`, the Lua tooling (StyLua, selene, LuaLS, busted, the `lua` CI jobs) and the guards
  that only covered it (the client boundary lint). orca-term is the core plus the terminal
  application. A person installs the binary (the release assets, or `cargo install --path .`), not
  a plugin.
- **The person's Neovim stays the editor**, in the application's `2 nvim` tab or in a window of its
  own, with no orca-term plugin in it.
- **Enter on a changed file still opens its diff in that Neovim, through the core.** The core finds
  the merge base and reads the file there with git, then sends every Neovim registered in the
  repository one msgpack-RPC request, `nvim_exec_lua` with code of its own
  (`src/domain/nvim/open_diff.lua`) and those facts as arguments. In a Neovim that works in the
  worktree the code opens a new tab with the file's version at the merge base beside the file on
  disk, both under Neovim's built-in `:diffthis`. The code loads no module and starts no process
  (`cargo xtask nvim-lua`), so it runs in a Neovim with no plugin at all.
- **Kept in the core**: the Neovim registry, `:checktime` at the end of a turn and the doctor's
  claudecode.nvim check, all plain RPC; and the `note` commands, so notes are added from the
  command line until the application has its own.
- **The worktree-changed notice goes.** After every write to a worktree's state the core fired
  `User OrcaTermWorktreeChanged` in every registered Neovim; only the client's sidebar listened.

This record reverses ADR 0012's "Diff review and notes stay in the Lua client", and replaces
[#180](https://github.com/marivaldo/orca-term/issues/180), which would have opened the review
without the sidebar.

## Considered options

- **Remove only what the application replaced** (the sidebar, the status strip, a tab per worktree,
  the agent's terminal inside Neovim), keeping the review and the notes in Lua. Rejected by the
  driver: the client would no longer be used.
- **Open the diff with diffview.nvim, sent by the core.** Rejected: it would make a plugin the
  person must install a requirement again. Built-in `:diffthis` needs nothing.
- **Have the Neovim run git** (`:read !git show`, or a Lua `vim.system`). Rejected: the core
  already runs git and resolves the merge base the right column counts against; handing the
  Neovim the content keeps one place that runs git, and the diff the same as the column's counts.
- **Keep the worktree-changed notice** for whoever may listen. Rejected: nothing listens, and every
  write paid for a round of RPC to each Neovim.

## Consequences

- `CONTEXT.md` no longer calls Neovim a client of the core.
- `README.md` installs orca-term without lazy.nvim, and `ARCHITECTURE.md` maps the core and the
  application only.
- The commands only the client used (`worktree review`, `worktree detail`, `turn diff`,
  `turn blame`, `pr compose`, ...) stay: they are the core's command-line surface, and the
  application may use them.
- A diff tab is Neovim's own: the person closes it with `:tabclose`; asking again for the same file
  replaces its earlier tab.
