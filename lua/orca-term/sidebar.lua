-- orca-term's sidebar: the fleet in a left split of every tab, toggled by `:OrcaTerm sidebar`.
--
-- This is the only module that touches windows and buffers (`cargo xtask client-boundary` checks
-- it). It reads the fleet through `core.lua`, has `render.lua` turn it into lines and colours, and
-- paints them into one buffer that every tab shows in its own window.
--
-- The fleet is read when the sidebar opens and when Neovim gains focus; there is no timer. Opening
-- never moves the cursor out of the person's window and never resizes their windows beyond the
-- split itself; closing only closes the sidebar's windows. A tab never ends up holding only the
-- sidebar: `:q` on its last other window closes the tab (or quits Neovim) as it would without it.

local core = require('orca-term.core')
local highlights = require('orca-term.highlights')
local render = require('orca-term.render')

local M = {}

---The namespace of the sidebar's highlights.
M.NAMESPACE = vim.api.nvim_create_namespace('orca-term-sidebar')

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

---@type orca_term.SidebarState
local state = { open = false }

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

---Renders the fleet last read, keeping the selection, and paints it.
local function redraw()
  if state.fleet then
    state.rendered = render.fleet(state.fleet, state.selected)
    state.selected = state.rendered.selected
  else
    local message = state.error and state.error.message or 'orca-term: the fleet was not read'
    state.rendered = render.error(message)
  end
  paint()
end

---Reads the fleet of the current directory's repository from the core, then redraws.
local function refresh()
  state.fleet, state.error = core.worktree_ls(vim.fn.getcwd())
  redraw()
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

  -- The sidebar's only keys, local to its buffer.
  vim.keymap.set('n', 'j', function()
    move(1)
  end, { buffer = buf, nowait = true, desc = 'orca-term: select the next worktree' })
  vim.keymap.set('n', 'k', function()
    move(-1)
  end, { buffer = buf, nowait = true, desc = 'orca-term: select the previous worktree' })

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

---Shows the sidebar in every tab, and in every tab made later, until it is closed.
function M.open()
  highlights.define()
  ensure_buffer()
  state.open = true
  refresh()
  for _, tab in ipairs(vim.api.nvim_list_tabpages()) do
    show_in(tab)
  end

  local group = vim.api.nvim_create_augroup('orca-term-sidebar', { clear = true })
  state.group = group
  vim.api.nvim_create_autocmd({ 'TabNewEntered', 'TabEnter' }, {
    group = group,
    desc = 'orca-term: show the sidebar in this tab',
    callback = function()
      show_in(vim.api.nvim_get_current_tabpage())
    end,
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
    desc = 'orca-term: read the fleet again',
    callback = refresh,
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
