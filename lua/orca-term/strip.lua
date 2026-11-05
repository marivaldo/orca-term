-- orca-term's status strip: how many worktrees are in each state, and how many are unseen, for a
-- statusline, so the fleet's health shows while the sidebar is closed. This is the every-state
-- fleet of spec/golden/sidebar.txt:
--
--   ?1 ●1 ●1 !1 △1 ◐1 ◌1 ●2 ○2 ·2 •4
--
-- With projects on the project list it counts the worktrees of every project together (#156);
-- with an empty list, those of the current directory's repository.
--
-- `render.strip` decides what it shows; this module keeps it up to date and hands it to a plain
-- statusline (`statusline()`), to lualine (lua/lualine/components/orca-term.lua, through
-- `items()`) and to whoever builds their own (`counts()`). It never adds itself to a statusline,
-- tabline or winbar: the person places it.
--
-- Nothing happens until one of those functions is first called. That call only schedules the
-- start, because a statusline is drawn where Neovim allows no change (textlock): the start then
-- defines the colours, subscribes to the live fleet as `strip` (which registers this Neovim with
-- the core, so `User OrcaTermWorktreeChanged` reaches it) and reads the fleet without waiting.
-- After that, the fleets the live fleet keeps are turned into the strip once after every read, and
-- the statuslines are drawn again. Drawing a statusline only returns what was kept: it never waits
-- and never starts a process. The strip stays subscribed for the rest of the session, whether the
-- sidebar is open or not.
--
-- This module touches no window, tab or buffer and starts no process (`cargo xtask
-- client-boundary` checks it): it reads the fleet through `live.lua`.

local highlights = require('orca-term.highlights')
local live = require('orca-term.live')
local render = require('orca-term.render')

local M = {}

---The name the strip subscribes to the live fleet under.
M.SUBSCRIBER = 'strip'

---What the strip holds between calls.
---@class orca_term.StripState
---@field scheduled boolean whether the start was scheduled, so it is scheduled once
---@field started boolean whether the start ran
---@field items orca_term.StripItem[] the strip of the fleets last kept
---@field counts orca_term.Counts the counts of the fleets last kept
---@field line string the strip as a statusline expression

---The name of the strip's autocommand group.
local GROUP = 'orca-term-strip'

---The strip as it is before its start.
---@return orca_term.StripState
local function new_state()
  return {
    scheduled = false,
    started = false,
    items = {},
    counts = { states = {}, unseen = 0 },
    line = '',
  }
end

local state = new_state()

---`items` as a statusline expression: each item as `%#Group#text`, then `%*` (back to the
---statusline's own colour), with a space between items. No item is the empty string.
---@param items orca_term.StripItem[]
---@return string
local function statusline_of(items)
  local parts = {}
  for _, item in ipairs(items) do
    table.insert(parts, ('%%#%s#%s%%*'):format(item.group, item.text))
  end
  return table.concat(parts, ' ')
end

---Draws every statusline and the tabline again, and has lualine draw its own when it is loaded,
---so they show the strip just kept.
---
---Neovim draws only for a UI: with none attached (`nvim --headless`, `nvim -l`, an embedded Neovim
---before its UI attaches) nobody sees a statusline, and a redraw there writes what the screen
---would show, such as the mode message `-- INSERT --`, straight to the terminal. So the
---statuslines are drawn again only when a UI is attached. lualine is told either way: it only
---updates the statusline options, which a UI draws when it attaches. A redraw that fails (where
---Neovim allows none) is skipped rather than raised into the live fleet's callback: the next one
---catches up.
local function redraw()
  if #vim.api.nvim_list_uis() > 0 then
    pcall(vim.cmd.redrawstatus, { bang = true })
    pcall(vim.cmd.redrawtabline)
  end
  if package.loaded.lualine then
    pcall(function()
      require('lualine').refresh()
    end)
  end
end

---Every worktree the strip counts: those of every fleet the live fleet keeps, every project's
---when the project list has some. A fleet that could not be read has none.
---@return orca_term.Worktree[]
local function kept_worktrees()
  local worktrees = {}
  for _, fleet in ipairs(live.fleets()) do
    vim.list_extend(worktrees, fleet.worktrees)
  end
  return worktrees
end

---Called by the live fleet after every read it keeps: keeps the strip of the fleets kept and draws
---the statuslines again. A fleet that could not be read shows nothing: the strip never shows an
---error.
local function on_fleet()
  local shown = state.line
  -- `render.strip` and `render.counts` read only a fleet's worktrees.
  local counted = { worktrees = kept_worktrees() }
  state.items = render.strip(counted)
  state.counts = render.counts(counted)
  state.line = statusline_of(state.items)
  -- Most reads change nothing the strip shows (an agent's activity, say): nothing to draw then.
  if state.line ~= shown then
    redraw()
  end
end

---Starts the strip, once: defines the colours (again after every `:colorscheme`), subscribes to
---the live fleet, shows the fleets already kept (when the sidebar read them) and reads them again
---without waiting. A change of the current directory reads again, since with an empty project list
---the strip shows the fleet of `getcwd()`.
local function start()
  if state.started then
    return
  end
  state.started = true
  highlights.define()
  local group = vim.api.nvim_create_augroup(GROUP, { clear = true })
  vim.api.nvim_create_autocmd('ColorScheme', {
    group = group,
    desc = "orca-term: define the status strip's colours again",
    callback = highlights.define,
  })
  vim.api.nvim_create_autocmd('DirChanged', {
    group = group,
    pattern = { 'global', 'tabpage' },
    desc = 'orca-term: read the fleet of the new directory for the status strip',
    callback = function()
      live.read_soon()
    end,
  })

  live.subscribe(M.SUBSCRIBER, on_fleet)
  on_fleet()
  live.read_soon()
end

---Schedules the start the first time it is called; never waits. Safe inside a statusline.
local function ensure_started()
  if state.scheduled then
    return
  end
  state.scheduled = true
  vim.schedule(function()
    -- `_reset()` may have come in between: then the start is no longer wanted.
    if state.scheduled then
      start()
    end
  end)
end

---The strip as a statusline expression, such as
---`%#OrcaTermStateWaiting#?1%* %#OrcaTermUnseen#•2%*`, or the empty string when there is no
---worktree, the fleet could not be read or was not read yet. It counts every project's worktrees
---when the project list has projects.
---Use it as `%{%v:lua.require'orca-term.strip'.statusline()%}`. The first call starts the strip.
---@return string
function M.statusline()
  ensure_started()
  return state.line
end

---The strip's items in order, each a text and its highlight group: what lualine draws. The first
---call starts the strip.
---@return orca_term.StripItem[]
function M.items()
  ensure_started()
  return state.items
end

---How many worktrees are in each state, and how many are unseen, in the fleets last kept (every
---project's when the project list has projects): for a statusline of one's own.
---`{ states = {}, unseen = 0 }` before the first read. The first call starts the strip.
---@return orca_term.Counts
function M.counts()
  ensure_started()
  return state.counts
end

---Forgets the strip, as if it never started: unsubscribes it from the live fleet, deletes its
---autocommands and drops what it kept, and a start still scheduled does nothing. For the specs,
---which share one Neovim; the next call starts the strip again.
function M._reset()
  live.unsubscribe(M.SUBSCRIBER)
  pcall(vim.api.nvim_del_augroup_by_name, GROUP)
  state = new_state()
end

return M
