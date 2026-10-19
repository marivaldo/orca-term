-- orca-term's detail float: everything about one worktree the sidebar has no room for.
--
-- This module, `sidebar.lua`, `tabs.lua`, `terminal.lua`, `review.lua` and `notes.lua` are the only
-- ones that touch windows and buffers (`cargo xtask client-boundary` checks it). `K` in the sidebar
-- (`sidebar.lua`) asks the core for the worktree's detail (`worktree detail`, through `core.lua`)
-- without waiting, and shows the answer, laid out by `render.lua` (`render.detail`), in a float
-- just right of the sidebar at the cursor's row: the state and the agent, the branch and its drift,
-- the whole error, the latest turn's denials and the end of the setup's log.
--
-- The float is a hover: the cursor stays in the sidebar. It closes when the sidebar's cursor
-- moves, when the person goes to any window but the sidebar or the float, on `K` again, on `q` or
-- `<Esc>` in the float, and when the sidebar closes. There is one at most. An answer that comes
-- after any of these is dropped, so a slow core never opens a float the person moved away from.

local core = require('orca-term.core')
local render = require('orca-term.render')

local M = {}

---The namespace of the float's highlights.
M.NAMESPACE = vim.api.nvim_create_namespace('orca-term-detail')

---The float's width in screen cells, its border left out: fitted to what it shows, between these.
M.MIN_WIDTH = 40
M.MAX_WIDTH = 80

---The float buffer's filetype.
M.FILETYPE = 'orca-term-detail'

---What the detail holds between calls.
---@class orca_term.DetailState
---@field request integer the number of the latest request, so an older answer is known
---@field pending? integer the request waiting for the core's answer
---@field cursor? integer[] the sidebar's cursor then, `{ row, col }`
---@field win? integer the float
---@field group? integer the autocommand group, while a request waits or the float is open
---@field answered integer how many answers came back, shown or dropped; for the specs

---@type orca_term.DetailState
local state = { request = 0, answered = 0 }

---The float, while it is open.
---@return integer?
function M.window()
  if state.win and vim.api.nvim_win_is_valid(state.win) then
    return state.win
  end
  return nil
end

---How many answers of the core came back so far, whether shown or dropped. The specs wait on it.
---@return integer
function M.answered()
  return state.answered
end

---Closes the float and forgets the request waiting for an answer, if any.
function M.close()
  state.pending = nil
  if state.group then
    vim.api.nvim_del_augroup_by_id(state.group)
    state.group = nil
  end
  local win = M.window()
  state.win = nil
  if win then
    pcall(vim.api.nvim_win_close, win, true)
  end
end

---Watches what closes the float, from the moment `K` asks the core: the sidebar's cursor moving
---off where it was, going to a window that is neither the sidebar nor the float, and the sidebar's
---window closing.
---@param sidebar integer
local function watch(sidebar)
  local group = vim.api.nvim_create_augroup('orca-term-detail', { clear = true })
  state.group = group
  vim.api.nvim_create_autocmd('CursorMoved', {
    group = group,
    buffer = vim.api.nvim_win_get_buf(sidebar),
    desc = "orca-term: close the worktree's detail when the sidebar's cursor moves",
    callback = function()
      -- CursorMoved may come once for a move made before it was watched: only a cursor that left
      -- where `K` was pressed closes the float.
      local win = vim.api.nvim_get_current_win()
      if win ~= sidebar or not vim.deep_equal(vim.api.nvim_win_get_cursor(win), state.cursor) then
        M.close()
      end
    end,
  })
  vim.api.nvim_create_autocmd('WinEnter', {
    group = group,
    desc = "orca-term: close the worktree's detail when leaving the sidebar",
    callback = function()
      local win = vim.api.nvim_get_current_win()
      if win ~= sidebar and win ~= state.win then
        M.close()
      end
    end,
  })
  vim.api.nvim_create_autocmd('WinClosed', {
    group = group,
    pattern = tostring(sidebar),
    desc = "orca-term: close the worktree's detail with the sidebar's window",
    callback = M.close,
  })
end

---Where the float goes, and how big it is, for content `width` cells wide and `height` lines high:
---just right of the sidebar at its cursor's row, moved up or left as much as it takes to stay in
---the editor. `row` and `col` are relative to the sidebar's window.
---@param sidebar integer
---@param width integer
---@param height integer
---@return { row: integer, col: integer, width: integer, height: integer }
local function placement(sidebar, width, height)
  local position = vim.api.nvim_win_get_position(sidebar)
  local editor_rows = vim.o.lines - vim.o.cmdheight
  height = math.max(1, math.min(height, math.floor(editor_rows / 2)))
  -- The border takes one cell on each side.
  local cursor_line = vim.api.nvim_win_call(sidebar, vim.fn.winline)
  local top = position[1] + cursor_line - 1
  top = math.max(0, math.min(top, editor_rows - height - 2))
  local left = position[2] + vim.api.nvim_win_get_width(sidebar)
  left = math.max(0, math.min(left, vim.o.columns - width - 2))
  return { row = top - position[1], col = left - position[2], width = width, height = height }
end

---Opens the float showing `detail` next to the sidebar window `sidebar`, without entering it.
---@param sidebar integer
---@param detail orca_term.Detail
local function show(sidebar, detail)
  -- As wide as the room right of the sidebar allows, the border's two cells left out; with less
  -- room than `MIN_WIDTH`, the float covers part of the sidebar rather than get narrower.
  local right_of_sidebar = vim.api.nvim_win_get_position(sidebar)[2]
    + vim.api.nvim_win_get_width(sidebar)
  local room = vim.o.columns - right_of_sidebar - 2
  local widest = math.max(1, math.min(M.MAX_WIDTH, math.max(room, M.MIN_WIDTH), vim.o.columns - 2))
  local rendered = render.detail(detail, widest)
  local longest = 0
  for _, line in ipairs(rendered.lines) do
    longest = math.max(longest, vim.fn.strdisplaywidth(line))
  end
  -- One cell on the right, as on the left.
  local width = math.min(math.max(longest + 1, M.MIN_WIDTH), widest)

  local buf = vim.api.nvim_create_buf(false, true)
  vim.bo[buf].bufhidden = 'wipe'
  vim.bo[buf].filetype = M.FILETYPE
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, rendered.lines)
  vim.bo[buf].modifiable = false
  for _, span in ipairs(rendered.spans) do
    vim.api.nvim_buf_set_extmark(buf, M.NAMESPACE, span.line - 1, span.first, {
      end_col = span.last,
      hl_group = span.group,
    })
  end
  for _, lhs in ipairs({ 'q', '<Esc>' }) do
    vim.keymap.set('n', lhs, M.close, {
      buffer = buf,
      nowait = true,
      desc = "orca-term: close the worktree's detail",
    })
  end

  local where = placement(sidebar, width, #rendered.lines)
  state.win = vim.api.nvim_open_win(buf, false, {
    relative = 'win',
    win = sidebar,
    row = where.row,
    col = where.col,
    width = where.width,
    height = where.height,
    style = 'minimal',
    border = 'rounded',
    title = ' ' .. detail.worktree.name .. ' ',
  })
end

---`K` in the sidebar: shows the detail of `worktree`, under the cursor of the sidebar window
---`sidebar`, asking the core from `cwd` (the primary checkout); or closes the float when one is
---open or on its way. A failure is told with `vim.notify`.
---@param sidebar integer
---@param worktree? orca_term.Worktree `nil` when the cursor is on no worktree
---@param cwd string
function M.toggle(sidebar, worktree, cwd)
  if M.window() or state.pending then
    M.close()
    return
  end
  if not worktree then
    return
  end
  state.request = state.request + 1
  local request = state.request
  state.pending = request
  state.cursor = vim.api.nvim_win_get_cursor(sidebar)
  watch(sidebar)
  core.worktree_detail(worktree.path, cwd, function(detail, err)
    state.answered = state.answered + 1
    -- An answer for a request closed meanwhile, or replaced by another, is dropped.
    if state.pending ~= request then
      return
    end
    state.pending = nil
    if not (detail and vim.api.nvim_win_is_valid(sidebar)) then
      M.close()
      if err then
        vim.notify(err.message, vim.log.levels.ERROR)
      end
      return
    end
    show(sidebar, detail)
  end)
end

return M
