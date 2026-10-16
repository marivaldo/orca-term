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
mark `•`, which a worktree gets when its agent's turn ends, and `d` opens its review. A row
shows `✎N` when the worktree has notes waiting to be sent.
`:OrcaTerm jump` picks a worktree, `:OrcaTerm worktree N` goes to the Nth, `:OrcaTerm review`
opens the review of the worktree whose tab you are in and `:OrcaTerm notes` lists its notes; bind
keys to `<Plug>(OrcaTermSidebar)`, `<Plug>(OrcaTermJump)`, `<Plug>(OrcaTermReview)`,
`<Plug>(OrcaTermNotes)` and `<Plug>(OrcaTermWorktree1)` to `<Plug>(OrcaTermWorktree9)`, since the
client sets no global key.

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

## Notes

A note is a remark for the worktree's agent on a line, a range of lines or a whole file of its
review. Notes wait until you send them; sending types them into the agent as one prompt, in Orca's
format (`File:`, then `Line:`, `Lines:` or `Scope: file`, then `User comment:`), and consumes them.

In the review (and only there), on the right-hand side, the worktree's file:

- `gn` adds a note on the cursor's line, asking what it says (`Note for the agent: `); in visual
  mode, on the selected lines. `gn` on a file in the file list adds a note on the whole file. The
  left-hand side is the review's base: `gn` there only says notes go on the right. Write the file
  first: notes go on the file on disk, so a buffer with unsaved changes is refused.
- Each note shows as `✎` beside its line (a range's other lines get `┆`), its text below it, at its
  line in the file on disk now: when the agent adds lines above it, it moves down; when its line is
  changed or deleted, it becomes a note on the whole file, shown above line 1. The file list shows
  `✎N` after each file with notes.
- `gN` (or `:OrcaTerm notes`, also from the worktree's tab) lists the notes: `Send all (N)` first,
  then each note, which you can send alone, edit or delete (deleting asks nothing, as in Orca).

Sending needs the worktree's agent running; when it is not, press `<CR>` on the worktree in the
sidebar to start it, then send again. The notes stay pending until then.

Limits:

- Notes show only in the review orca-term opens (`d`), not in a diffview opened by hand, nor when
  the file is edited in the worktree's tab.
- A note on the whole file is drawn above line 1, which Neovim shows only once the window is
  scrolled up past line 1 (`<C-y>` on line 1).
- `gn` works on the files the review lists; a file git does not track is not listed, but a note
  added on it with `orca-term note add` shows in the list and is sent with the others.

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
