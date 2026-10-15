-- orca-term's sidebar: the fleet in a left split of every tab, toggled by `:OrcaTerm sidebar`.
--
-- This module, `tabs.lua` and `terminal.lua` are the only ones that touch windows and buffers
-- (`cargo xtask client-boundary` checks it). It reads the fleet through `core.lua`, has
-- `render.lua` turn it into lines and colours, and paints them into one buffer that every tab shows
-- in its own window. Its keys, local to that buffer, move the selection (`j`, `k`), open a worktree
-- (`<CR>`: its tab, and its agent's terminal when it has an agent), create a worktree (`a`) and
-- open the menu of the worktree under the cursor (`.`), all three through `actions.lua`. The
-- selection follows the current tab when it is a worktree's.
--
-- The fleet is read when the sidebar opens, when Neovim gains focus, after an action changes it and
-- when the core says a worktree changed; it is never polled. Opening the sidebar, and Neovim
-- gaining focus while it is open, registers this Neovim with the core (`nvim register`), which then
-- fires `User OrcaTermWorktreeChanged` here after every change to a worktree. That event reads the
-- fleet without waiting, 100 ms later so a burst of events makes one read, one read at a time:
-- events during a read make one more read after it. Neovim unregisters as it exits; closing the
-- sidebar does not, and the events are ignored while it is closed. While a listed worktree is
-- working, one timer turns its glyph through `render.SPINNER` by drawing the fleet already read
-- again, never reading it; the timer stops once no worktree is working or the sidebar closes.
-- Opening never moves the cursor out of the person's window and never resizes their windows beyond
-- the split itself; closing only closes the sidebar's windows. A tab never ends up holding only the
-- sidebar: `:q` on its last other window closes the tab (or quits Neovim) as it would without it.

local actions = require('orca-term.actions')
local core = require('orca-term.core')
local highlights = require('orca-term.highlights')
local render = require('orca-term.render')
local tabs = require('orca-term.tabs')

local M = {}

---The namespace of the sidebar's highlights.
M.NAMESPACE = vim.api.nvim_create_namespace('orca-term-sidebar')

---How often the working glyph turns, in milliseconds.
M.SPIN_MS = 250

---How long after the core says a worktree changed the fleet is read, in milliseconds. The events of
---a burst (a hook for every tool an agent runs) arrive within it and make one read.
M.LIVE_DELAY_MS = 100

---The `User` autocommand the core fires after every change to a worktree.
M.EVENT = 'OrcaTermWorktreeChanged'

---The sidebar buffer's name and filetype.
M.BUFFER_NAME = 'orca-term://sidebar'
M.FILETYPE = 'orca-term-sidebar'

---What the sidebar holds between calls.
---@class orca_term.SidebarState
---@field open boolean whether the sidebar is shown (in every tab)
---@field buf? integer the sidebar's buffer
---@field group? integer the autocommand group, while open
---@field fleet? orca_term.Fleet the fleet last read, `nil` when reading it failed
---@field error? orca_term.Error why the fleet could not be read
---@field selected? string the selected worktree's path
---@field rendered? orca_term.Rendered what the buffer shows
---@field frame integer the spinner frame the working glyph shows
---@field timer? uv.uv_timer_t the timer turning the working glyph, while one is working
---How many reads of the fleet have started; a read that ends after a newer one started is dropped.
---@field reads integer

---@type orca_term.SidebarState
local state = { open = false, frame = 0, reads = 0 }

---Where this Neovim registered with the core: its address and the directory the core ran in.
---@class orca_term.Registration
---@field address string
---@field cwd string

---What the live updates hold between the core's events.
---@class orca_term.LiveState
---@field group? integer their autocommand group, made when the sidebar first opens, never deleted
---@field timer? uv.uv_timer_t the timer waiting `LIVE_DELAY_MS` to read the fleet after an event
---@field reading boolean whether a read started by an event is running
---@field again boolean whether an event came during that read, so one more read follows it
---@field registered orca_term.Registration[] every registration, undone as Neovim exits

---@type orca_term.LiveState
local live = { reading = false, again = false, registered = {} }

---Whether the sidebar is open.
---@return boolean
function M.is_open()
  return state.open
end

---The sidebar's buffer, or `nil` before it is first opened.
---@return integer?
function M.buffer()
  if state.buf and vim.api.nvim_buf_is_valid(state.buf) then
    return state.buf
  end
  return nil
end

---Every window showing the sidebar, in every tab.
---@return integer[]
local function sidebar_windows()
  local found = {}
  for _, win in ipairs(vim.api.nvim_list_wins()) do
    if vim.api.nvim_win_get_buf(win) == state.buf then
      table.insert(found, win)
    end
  end
  return found
end

---The window showing the sidebar in `tab`, if any.
---@param tab integer
---@return integer?
local function window_in(tab)
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(tab)) do
    if vim.api.nvim_win_get_buf(win) == state.buf then
      return win
    end
  end
  return nil
end

---Puts the cursor of every sidebar window on the selected worktree's name.
local function follow_selection()
  local rendered = state.rendered
  if not rendered then
    return
  end
  local line = rendered.firsts[rendered.selected or ''] or 1
  for _, win in ipairs(sidebar_windows()) do
    vim.api.nvim_win_set_cursor(win, { line, 0 })
  end
end

---Writes the rendered lines and their colours into the buffer.
local function paint()
  local buf = state.buf
  local rendered = state.rendered
  if not (buf and rendered) then
    return
  end
  vim.bo[buf].modifiable = true
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, rendered.lines)
  vim.bo[buf].modifiable = false
  vim.api.nvim_buf_clear_namespace(buf, M.NAMESPACE, 0, -1)
  for _, span in ipairs(rendered.spans) do
    vim.api.nvim_buf_set_extmark(buf, M.NAMESPACE, span.line - 1, span.first, {
      end_col = span.last,
      hl_group = span.group,
    })
  end
  follow_selection()
end

---Writes again only the lines that differ from `before`, then the colours, leaving every
---cursor where it is. The spinner changes glyphs only, so the lines keep their number and their
---byte lengths; anything else is painted whole.
---@param before orca_term.Rendered
local function repaint(before)
  local buf = state.buf
  local rendered = state.rendered
  if not (buf and rendered) then
    return
  end
  if #before.lines ~= #rendered.lines then
    paint()
    return
  end
  vim.bo[buf].modifiable = true
  for index, line in ipairs(rendered.lines) do
    if line ~= before.lines[index] then
      vim.api.nvim_buf_set_lines(buf, index - 1, index, false, { line })
    end
  end
  vim.bo[buf].modifiable = false
  vim.api.nvim_buf_clear_namespace(buf, M.NAMESPACE, 0, -1)
  for _, span in ipairs(rendered.spans) do
    vim.api.nvim_buf_set_extmark(buf, M.NAMESPACE, span.line - 1, span.first, {
      end_col = span.last,
      hl_group = span.group,
    })
  end
end

---Stops the timer turning the working glyph, if it runs, and goes back to the first frame.
local function stop_spinning()
  if state.timer then
    state.timer:stop()
    state.timer:close()
    state.timer = nil
  end
  state.frame = 0
end

---One turn of the working glyph: the next frame of the fleet already read, painted without moving
---any cursor or the selection. The fleet is not read again.
local function spin()
  local before = state.rendered
  if not (state.timer and state.fleet and before) then
    return
  end
  state.frame = state.frame + 1
  state.rendered = render.fleet(state.fleet, state.selected, nil, state.frame)
  repaint(before)
end

---Starts the timer turning the working glyph while the sidebar is open and a worktree of the
---fleet last read is working, and stops it otherwise. There is one timer at most.
local function update_spinning()
  local wanted = state.open and state.fleet ~= nil and render.spins(state.fleet)
  if not wanted then
    stop_spinning()
    return
  end
  if state.timer then
    return
  end
  local timer = assert(vim.uv.new_timer())
  timer:start(M.SPIN_MS, M.SPIN_MS, vim.schedule_wrap(spin))
  state.timer = timer
end

---Whether the working glyph is turning: the sidebar is open and a worktree is working.
---@return boolean
function M.spinning()
  return state.timer ~= nil
end

---Renders the fleet last read, keeping the selection, and paints it.
local function redraw()
  if state.fleet then
    state.rendered = render.fleet(state.fleet, state.selected, nil, state.frame)
    state.selected = state.rendered.selected
  else
    local message = state.error and state.error.message or 'orca-term: the fleet was not read'
    state.rendered = render.error(message)
  end
  paint()
  update_spinning()
end

---Reads the fleet of the current directory's repository from the core, then redraws.
local function refresh()
  state.reads = state.reads + 1
  state.fleet, state.error = core.worktree_ls(vim.fn.getcwd())
  redraw()
end

---Reads the fleet again and redraws, when the sidebar is open; does nothing otherwise.
function M.refresh()
  if state.open then
    refresh()
  end
end

---Selects the worktree whose directory is the current tab's, when there is one. In any other tab,
---such as the primary checkout's, the selection stays where it was.
local function follow_tab()
  if not state.fleet then
    return
  end
  for _, worktree in ipairs(state.fleet.worktrees) do
    if tabs.is_current(worktree.path) then
      if state.selected ~= worktree.path then
        state.selected = worktree.path
        redraw()
      end
      return
    end
  end
end

---The worktree on the cursor's line, or `nil` on the header, a heading or a blank line.
---@return orca_term.Worktree?
local function worktree_under_cursor()
  local line = vim.api.nvim_win_get_cursor(0)[1]
  local path = state.rendered and state.rendered.paths[line]
  if not (path and state.fleet) then
    return nil
  end
  for _, worktree in ipairs(state.fleet.worktrees) do
    if worktree.path == path then
      return worktree
    end
  end
  return nil
end

---The primary checkout's path, where the core runs for the actions; the current directory when the
---fleet could not be read.
---@return string
local function primary_path()
  return state.fleet and state.fleet.primary.path or vim.fn.getcwd()
end

---`<CR>`: selects the worktree under the cursor and opens it: its tab, and its agent's terminal.
local function open_worktree()
  local worktree = worktree_under_cursor()
  if not worktree then
    return
  end
  state.selected = worktree.path
  redraw()
  actions.open(worktree, primary_path())
end

---`.`: the menu of the worktree under the cursor.
local function worktree_menu()
  local worktree = worktree_under_cursor()
  if worktree then
    actions.menu(worktree, primary_path())
  end
end

---Moves the selection `step` worktrees down (1) or up (-1), stopping at the first and the last.
---@param step integer
local function move(step)
  local order = state.rendered and state.rendered.order or {}
  if #order == 0 then
    return
  end
  local index = 1
  for i, path in ipairs(order) do
    if path == state.selected then
      index = i
    end
  end
  index = math.max(1, math.min(#order, index + step))
  state.selected = order[index]
  redraw()
end

---The buffer named `orca-term://sidebar`, left by an earlier load of this module, if any.
---@return integer?
local function named_buffer()
  for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    if vim.api.nvim_buf_get_name(buf) == M.BUFFER_NAME then
      return buf
    end
  end
  return nil
end

---The sidebar's buffer, made the first time: a scratch buffer with the sidebar's keys. A buffer
---left by an earlier load of this module (a plugin manager reloading it) is taken over.
---@return integer
local function ensure_buffer()
  local existing = M.buffer()
  if existing then
    return existing
  end
  local buf = named_buffer()
  if not buf then
    buf = vim.api.nvim_create_buf(false, true)
    vim.api.nvim_buf_set_name(buf, M.BUFFER_NAME)
  end
  vim.bo[buf].buftype = 'nofile'
  vim.bo[buf].bufhidden = 'hide'
  vim.bo[buf].swapfile = false
  vim.bo[buf].modifiable = false
  vim.bo[buf].filetype = M.FILETYPE

  -- The sidebar's keys, local to its buffer.
  local function map(lhs, rhs, desc)
    vim.keymap.set('n', lhs, rhs, { buffer = buf, nowait = true, desc = 'orca-term: ' .. desc })
  end
  map('j', function()
    move(1)
  end, 'select the next worktree')
  map('k', function()
    move(-1)
  end, 'select the previous worktree')
  map('<CR>', open_worktree, "go to the worktree's tab and show its agent")
  map('a', function()
    actions.create(primary_path())
  end, 'create a worktree')
  map('.', worktree_menu, 'the actions of the worktree under the cursor')

  state.buf = buf
  return buf
end

---Shows the sidebar in `tab`, at the far left, unless it is already there. The person's current
---window stays current.
---@param tab integer
local function show_in(tab)
  local buf = ensure_buffer()
  if window_in(tab) then
    return
  end
  local win
  -- `win = -1` splits the whole tab, as `:topleft vsplit` does. nvim_win_call runs it in `tab`
  -- without entering it. Giving the width keeps 'equalalways' from resizing the other windows.
  vim.api.nvim_win_call(vim.api.nvim_tabpage_get_win(tab), function()
    win = vim.api.nvim_open_win(buf, false, { split = 'left', win = -1, width = render.WIDTH })
  end)
  local options = vim.wo[win]
  options.winfixwidth = true
  options.number = false
  options.relativenumber = false
  options.signcolumn = 'no'
  options.foldcolumn = '0'
  options.statuscolumn = ''
  options.list = false
  options.wrap = false
  options.spell = false
  options.cursorline = false
  follow_selection()
end

---Closes one sidebar window. The last window of a tab cannot be closed, so it shows an empty
---buffer instead.
---@param win integer
local function hide(win)
  local tab = vim.api.nvim_win_get_tabpage(win)
  if #vim.api.nvim_tabpage_list_wins(tab) == 1 then
    vim.api.nvim_win_call(win, function()
      vim.cmd.enew()
    end)
    return
  end
  -- With 'equalalways', closing a window makes the others the same width; 'eadirection' set to
  -- `ver` keeps the widths the person chose, so the sidebar's columns go to its neighbour.
  local eadirection = vim.o.eadirection
  vim.o.eadirection = 'ver'
  pcall(vim.api.nvim_win_close, win, true)
  vim.o.eadirection = eadirection
end

---The windows of `tab` other than the sidebar's, leaving out floating windows.
---@param tab integer
---@return integer[]
local function other_windows(tab)
  local found = {}
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(tab)) do
    local floating = vim.api.nvim_win_get_config(win).relative ~= ''
    if vim.api.nvim_win_get_buf(win) ~= state.buf and not floating then
      table.insert(found, win)
    end
  end
  return found
end

---Before `:q` (`:wq`, `ZZ`) on the last window of the tab that is not the sidebar, closes the
---tab's sidebar, so the `:q` closes the tab, or quits Neovim in the last tab, as it would without
---the sidebar.
local function before_quit()
  local current = vim.api.nvim_get_current_win()
  local tab = vim.api.nvim_get_current_tabpage()
  local others = other_windows(tab)
  local sidebar = window_in(tab)
  if sidebar and #others == 1 and others[1] == current then
    vim.api.nvim_win_close(sidebar, true)
  end
end

---After a window closes without `:q` (`<C-w>c`, `:close`), closes its tab when only the sidebar is
---left in it. The last tab is left as it is: quitting Neovim for the person would be worse.
---@param closed integer the window being closed, still in the layout
local function after_close(closed)
  if not vim.api.nvim_win_is_valid(closed) then
    return
  end
  local tab = vim.api.nvim_win_get_tabpage(closed)
  -- WinClosed runs before the window leaves the layout: look once it is gone.
  vim.schedule(function()
    if not vim.api.nvim_tabpage_is_valid(tab) or #vim.api.nvim_list_tabpages() == 1 then
      return
    end
    if window_in(tab) and #other_windows(tab) == 0 then
      vim.cmd.tabclose(vim.api.nvim_tabpage_get_number(tab))
    end
  end)
end

---Reads the fleet after an event, set below: it and `read_after_event` start each other.
---@type fun()
local read_live

---Reads the fleet `LIVE_DELAY_MS` from now, while the sidebar is open: unless a read is already
---waiting, which the events of a burst share, or running, which one more read then follows.
local function read_after_event()
  if not state.open then
    return
  end
  if live.reading then
    live.again = true
    return
  end
  live.timer = live.timer or assert(vim.uv.new_timer())
  if not live.timer:is_active() then
    live.timer:start(M.LIVE_DELAY_MS, 0, vim.schedule_wrap(read_live))
  end
end

---Reads the fleet without waiting, then redraws as `refresh()` does. A read that ends after the
---sidebar closed, or after a newer read started (the sidebar opened again, Neovim gained focus), is
---dropped. When an event came during the read, one more read follows.
read_live = function()
  if not state.open then
    return
  end
  state.reads = state.reads + 1
  local this_read = state.reads
  live.reading = true
  core.worktree_ls_async(vim.fn.getcwd(), function(fleet, err)
    live.reading = false
    if state.open and state.reads == this_read then
      state.fleet, state.error = fleet, err
      redraw()
    end
    if live.again then
      live.again = false
      read_after_event()
    end
  end)
end

---Where the core can reach this Neovim: `v:servername` when it is the absolute path of a unix
---socket, the only kind of address the core takes; `nil` otherwise (no server, or a TCP address).
---@return string?
local function server_address()
  local address = vim.v.servername
  if type(address) == 'string' and vim.startswith(address, '/') then
    return address
  end
  return nil
end

---Registers this Neovim with the core, without waiting, from the primary checkout of the fleet
---last read, and remembers it for `unregister()`. A failure is silent: the sidebar then updates on
---focus and after actions only. It runs again on `FocusGained`, as the core forgets a Neovim it
---could not reach (one suspended long enough stops answering); registering again changes nothing.
local function register()
  local address = server_address()
  if not address then
    return
  end
  local cwd = primary_path()
  core.nvim_register(address, cwd, function(_, err)
    if err then
      return
    end
    for _, registration in ipairs(live.registered) do
      if registration.address == address and registration.cwd == cwd then
        return
      end
    end
    table.insert(live.registered, { address = address, cwd = cwd })
  end)
end

---Undoes every registration, waiting for each (1 second at most). Errors are ignored: Neovim is
---exiting, and the core forgets an address it cannot reach anyway.
local function unregister()
  for _, registration in ipairs(live.registered) do
    core.nvim_unregister(registration.address, registration.cwd)
  end
  live.registered = {}
end

---Makes the autocommands of the live updates, the first time the sidebar opens. Their group
---outlives the sidebar: closing it unregisters nothing, and `read_after_event` ignores the events
---while it is closed.
local function ensure_live_autocmds()
  if live.group then
    return
  end
  live.group = vim.api.nvim_create_augroup('orca-term-live', { clear = true })
  vim.api.nvim_create_autocmd('User', {
    group = live.group,
    pattern = M.EVENT,
    desc = 'orca-term: read the fleet again, a worktree changed',
    callback = function()
      read_after_event()
    end,
  })
  vim.api.nvim_create_autocmd('VimLeavePre', {
    group = live.group,
    desc = 'orca-term: tell the core this Neovim is gone',
    callback = function()
      unregister()
    end,
  })
end

---Shows the sidebar in every tab, and in every tab made later, until it is closed.
function M.open()
  highlights.define()
  ensure_buffer()
  state.open = true
  refresh()
  follow_tab()
  for _, tab in ipairs(vim.api.nvim_list_tabpages()) do
    show_in(tab)
  end
  ensure_live_autocmds()
  register()

  local group = vim.api.nvim_create_augroup('orca-term-sidebar', { clear = true })
  state.group = group
  vim.api.nvim_create_autocmd({ 'TabNewEntered', 'TabEnter' }, {
    group = group,
    desc = 'orca-term: show the sidebar in this tab',
    callback = function()
      show_in(vim.api.nvim_get_current_tabpage())
    end,
  })
  vim.api.nvim_create_autocmd('TabEnter', {
    group = group,
    desc = 'orca-term: select the worktree whose tab this is',
    callback = follow_tab,
  })
  vim.api.nvim_create_autocmd('DirChanged', {
    group = group,
    pattern = 'tabpage',
    desc = "orca-term: select the worktree this tab's directory became",
    callback = follow_tab,
  })
  vim.api.nvim_create_autocmd('QuitPre', {
    group = group,
    desc = "orca-term: let :q on the tab's last window close the tab, not leave the sidebar",
    callback = before_quit,
  })
  vim.api.nvim_create_autocmd('WinClosed', {
    group = group,
    desc = 'orca-term: close a tab left with only the sidebar',
    callback = function(args)
      after_close(tonumber(args.match) or -1)
    end,
  })
  vim.api.nvim_create_autocmd('FocusGained', {
    group = group,
    desc = 'orca-term: read the fleet again, and register with the core again',
    callback = function()
      refresh()
      register()
    end,
  })
  vim.api.nvim_create_autocmd('ColorScheme', {
    group = group,
    desc = "orca-term: define the sidebar's colours again",
    callback = highlights.define,
  })
end

---Closes the sidebar in every tab.
function M.close()
  state.open = false
  stop_spinning()
  -- A read waiting after an event is dropped; one running is dropped as it ends.
  if live.timer then
    live.timer:stop()
  end
  live.again = false
  if state.group then
    vim.api.nvim_del_augroup_by_id(state.group)
    state.group = nil
  end
  if not M.buffer() then
    return
  end
  for _, win in ipairs(sidebar_windows()) do
    hide(win)
  end
end

---Shows the sidebar in every tab, or hides it from every tab.
function M.toggle()
  if state.open then
    M.close()
  else
    M.open()
  end
end

return M
