# orca-term

Orchestrate a fleet of parallel CLI coding agents from the terminal, with Neovim as the editor.

Inspired by [Orca](https://github.com/stablyai/orca), but without the Electron app: each agent
runs in its own git worktree, and review happens in the tools already living in the terminal.

> **Not affiliated with Orca or Stably.** orca-term is an independent open-source project. It is
> not made, endorsed or supported by Stably or by the Orca team, and it shares no code with Orca.
> "Orca" is used here only to credit the project that inspired it.

## Status

Early. The route was charted as a [wayfinder](https://aihero.dev/skills-wayfinder) map in this
repo's issues (the issue labelled `wayfinder:map`) and synthesised into a spec. It is now being
built in small slices, each a GitHub issue under that spec.

Today: `orca-term worktree new|ls|setup|rm|prune` manage the worktrees of the current repository,
and `orca-term agent start|attach|stop <worktree>` run claude in one of them, inside claude's own
sandbox, held by a detached process that any terminal can attach to. `orca-term note
add|ls|edit|rm|send <worktree>` attach notes to a worktree's lines or files and paste them into its
agent as one prompt, in Orca's format.

In Neovim, `:OrcaTerm sidebar` shows the fleet in every tab. Each worktree gets a tab of its own,
pointed at it with `:tcd`: `<CR>` on a sidebar row goes there, `a` creates a worktree, `.` runs
its setup again, copies its path or removes it (always asking first), `u` toggles its unseen
mark `•`, which a worktree gets when its agent's turn ends, and `d` opens its review.
`:OrcaTerm jump` picks a worktree, `:OrcaTerm worktree N` goes to the Nth and `:OrcaTerm review`
opens the review of the worktree whose tab you are in; bind keys to `<Plug>(OrcaTermSidebar)`,
`<Plug>(OrcaTermJump)`, `<Plug>(OrcaTermReview)` and `<Plug>(OrcaTermWorktree1)` to
`<Plug>(OrcaTermWorktree9)`, since the client sets no global key.

## Review

The review shows what a worktree changed since it forked from the default branch: the commit it
forked from (`git merge-base <default branch> HEAD`) against its files on disk, so its commits and
its uncommitted changes show together, and the right-hand side is the worktree's real files. Files
git does not track yet are not listed. It needs
[diffview.nvim](https://github.com/sindrets/diffview.nvim); without it, `d` only says so.

`d` on a worktree in the sidebar (or `:OrcaTerm review` in its tab) goes to the worktree's tab and
opens the review from there, in a tab of its own where the sidebar is not shown; `d` again goes
back to that tab and refreshes it. Opening the review clears the worktree's unseen mark. In the
review, `gf` on a file (in the file list or in the diff) edits it in the worktree's tab, on the
same line when the cursor is in the right-hand side; when that tab shows only the agent's
terminal, the file opens beside it. Close the review with `:DiffviewClose`, or `:tabclose`.

## Status strip

The status strip shows the fleet's health while the sidebar is closed: how many worktrees are in
each state, then how many are unseen, such as `?1 ◐2 ●3 •2`. orca-term never adds it to your
statusline, tabline or winbar: you place it, and pad it, where you want. It shows nothing when the
repository has no worktree, outside a git repository and before the fleet is first read. It
updates when the core says a worktree changed, as the sidebar does.

In a plain statusline, as an expression:

```lua
vim.o.statusline = "%f %= %{%v:lua.require'orca-term.strip'.statusline()%} %l:%c"
```

With [lualine](https://github.com/nvim-lualine/lualine.nvim), as the `orca-term` component (each
count keeps the section's background):

```lua
require('lualine').setup({ sections = { lualine_x = { 'orca-term', 'filetype' } } })
```

With [LazyVim](https://www.lazyvim.org), add it to the front of `lualine_x` (orca-term must be on
the runtimepath when lualine starts, so do not lazy-load it):

```lua
{
  'nvim-lualine/lualine.nvim',
  opts = function(_, opts)
    table.insert(opts.sections.lualine_x, 1, 'orca-term')
  end,
}
```

The glyphs are the sidebar's, in the order of its attention: `?` waiting for you, `●` failed (in
red), `●` interrupted (muted), `!` broken, `△` not ready, `◐` working, `◌` setting up, `●` finished
(in green), `○` idle, `·` no agent, then `•` unseen. Each takes the colour of its
`OrcaTermState*` group, and `•` of `OrcaTermUnseen`.

To build a strip of your own, `require('orca-term.strip').counts()` gives the counts of the fleet
last read: `{ states = { { state = 'waiting', count = 1 }, ... }, unseen = 2 }`, in the same order,
leaving out the states no worktree is in.

## Development

Needs Rust 1.97 and [just](https://github.com/casey/just). `just check` runs every check CI runs;
see `docs/harness.md` for the tools it expects and the rules it enforces. `just hooks` installs the
local git hooks (needs [lefthook](https://github.com/evilmartians/lefthook)).

### Tools for the Neovim client

`just lua`, part of `just check`, formats, lints and type-checks the Lua and runs its specs in
headless Neovim. It needs Neovim 0.11 or later, [StyLua](https://github.com/JohnnyMorganz/StyLua),
[selene](https://github.com/Kampfkarren/selene),
[lua-language-server](https://github.com/LuaLS/lua-language-server) and
[LuaRocks](https://luarocks.org) with LuaJIT. Set them up once; on macOS with Homebrew:

```sh
brew install neovim luajit stylua selene lua-language-server luarocks
just lua-rocks   # installs busted, nlua, lualine.nvim and diffview.nvim, pinned, into .rocks/ (ignored by git)
```

On Linux, CI shows the way: the tools come from their GitHub releases at the versions pinned in
`.github/workflows/check.yml`, LuaRocks and the Lua 5.1 headers from the distribution
(`luarocks liblua5.1-0-dev` on Ubuntu), then `just lua-rocks`.

## Scope

In: parallel agents in isolated worktrees, diff review that can feed comments back to the agent
that wrote the code, Neovim integration over a standalone core.

Out: mobile companion, cloud relay, screenshot-driven design mode, Linear integration,
worktrees over SSH.
