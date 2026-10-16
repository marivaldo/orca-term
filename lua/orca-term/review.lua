-- orca-term's review tabs: a worktree's review shown in diffview.nvim, in a tab of its own.
--
-- This module, `sidebar.lua`, `tabs.lua` and `terminal.lua` are the only ones that touch windows,
-- tabs and buffers (`cargo xtask client-boundary` checks it), and this is the only module that
-- calls diffview.nvim. diffview has no type annotations LuaLS knows, so its modules and views are
-- plain `table`s here, and only the few fields and functions named below are used.
--
-- A review compares the review's base (the commit the worktree forked from, which the core's
-- `worktree review` prints) with the worktree's files on disk: `DiffviewOpen <base>`, run with
-- `-C<worktree>`. It opens in a new tab, which diffview makes with `:tab split` from the worktree's
-- tab, so it has the worktree's tab-local directory too. That is why a review tab must never be
-- taken for a worktree's tab: `tabs.lua` leaves them out, and the sidebar never shows in one.
--
-- A review tab is any tab holding a diffview view, opened by orca-term or by hand, and any tab
-- whose `t:orca_term_review` holds a worktree's path, the mark orca-term puts on the tabs it opens.
-- In orca-term's review tabs (only those), `gf` in diffview's file panel and diff windows edits the
-- file in the worktree's tab instead of diffview's previous tab.
--
-- The autocommands of `gf` are made by the first review opened, never at startup.

local M = {}

---The tab variable that marks orca-term's review tabs, holding the worktree's path.
M.MARK = 'orca_term_review'

---Where to get diffview.nvim, for the message that says the review needs it.
M.DIFFVIEW_URL = 'https://github.com/sindrets/diffview.nvim'

---The autocommand group of `gf`.
local GROUP = 'orca-term-review'

---Whether a review is being opened right now. diffview makes its tab before it registers its
---view, and the sidebar's `TabNewEntered` runs in between: this tells it the new tab is a review.
local opening = false

---`path` with its symlinks resolved, or only normalized when it does not exist, as tabs.lua does.
---@param path string
---@return string
local function resolved(path)
  return vim.uv.fs_realpath(path) or vim.fs.normalize(path)
end

---diffview's main module, loading it on the first call, or `nil` when diffview.nvim is not
---installed.
---@return table?
local function diffview()
  local found, module = pcall(require, 'diffview')
  if found and type(module) == 'table' then
    return module
  end
  return nil
end

---Whether diffview.nvim can be loaded.
---@return boolean
function M.available()
  return diffview() ~= nil
end

---The diffview view in `tab`, or `nil`. diffview is only asked once it is loaded: telling whether
---a tab is a review never loads it.
---@param tab integer
---@return table?
local function view_in(tab)
  local lib = package.loaded['diffview.lib']
  if type(lib) ~= 'table' then
    return nil
  end
  return lib.tabpage_to_view(tab)
end

---The windows diffview laid out in `tab`, its file panel and its diff windows, as a set of window
---ids; empty when `tab` holds no diffview view.
---@param tab integer
---@return table<integer, boolean>
function M.view_windows(tab)
  local found = {}
  local view = view_in(tab)
  if not view then
    return found
  end
  if view.panel and view.panel.winid then
    found[view.panel.winid] = true
  end
  for _, win in ipairs(view.cur_layout and view.cur_layout.windows or {}) do
    if win.id then
      found[win.id] = true
    end
  end
  return found
end

---The worktree path `tab` is orca-term's review of, or `nil` when orca-term did not open it.
---@param tab integer
---@return string?
function M.worktree_of(tab)
  return vim.t[tab][M.MARK]
end

---The worktree path the current tab is orca-term's review of, or `nil`.
---@return string?
function M.current()
  return M.worktree_of(vim.api.nvim_get_current_tabpage())
end

---Whether `tab` is a review tab: it holds a diffview view, orca-term marked it as a review, or it
---is the tab a review is being opened in.
---@param tab integer
---@return boolean
function M.is_review(tab)
  if opening and tab == vim.api.nvim_get_current_tabpage() then
    return true
  end
  return M.worktree_of(tab) ~= nil or view_in(tab) ~= nil
end

---The open review tab of the worktree at `path`: a tab orca-term marked as its review, whose
---diffview view is still there. `nil` when there is none.
---@param path string
---@return integer?
function M.find(path)
  local wanted = resolved(path)
  for _, tab in ipairs(vim.api.nvim_list_tabpages()) do
    if M.worktree_of(tab) == wanted and view_in(tab) then
      return tab
    end
  end
  return nil
end

---Whether `win` is a window to edit a file in: not floating, not the sidebar, not an agent's
---terminal.
---@param win integer
---@return boolean
local function editable(win)
  if vim.api.nvim_win_get_config(win).relative ~= '' then
    return false
  end
  local buf = vim.api.nvim_win_get_buf(win)
  if buf == require('orca-term.sidebar').buffer() then
    return false
  end
  return vim.b[buf][require('orca-term.terminal').MARK] == nil
end

---Edits the file at `file` in the current tab, on line `line` when given: in the current window
---when it is editable, else in the tab's first editable window. When the tab holds only the sidebar
---and the agent's terminal, the file opens in a new window split beside the terminal, as `:vsplit`
---does ('splitright' says on which side), so the agent stays in sight.
---@param file string an absolute path
---@param line? integer
local function edit_here(file, line)
  local target = nil
  if editable(vim.api.nvim_get_current_win()) then
    target = vim.api.nvim_get_current_win()
  else
    for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
      if editable(win) then
        target = win
        break
      end
    end
  end

  local command = target and 'edit ' or 'vsplit '
  if target then
    vim.api.nvim_set_current_win(target)
  end
  -- `:edit` fails when the window's buffer has unsaved changes and 'hidden' is off.
  local edited, problem = pcall(function()
    vim.cmd(command .. vim.fn.fnameescape(file))
  end)
  if not edited then
    vim.notify('orca-term: ' .. tostring(problem):gsub('^Vim%(%a+%):', ''), vim.log.levels.ERROR)
    return
  end
  if line then
    local last = vim.api.nvim_buf_line_count(0)
    vim.api.nvim_win_set_cursor(0, { math.min(line, last), 0 })
  end
end

---`gf` where orca-term did not open the review, as on a buffer diffview shares with another tab:
---diffview's own `gf` in a diffview view, Neovim's `gf` anywhere else.
---@param view? table the diffview view of the current tab
local function fallback_gf(view)
  if view then
    require('diffview.actions').goto_file_edit()
    return
  end
  local done, problem = pcall(vim.cmd.normal, { args = { 'gf' }, bang = true })
  if not done then
    vim.notify(tostring(problem):gsub('^Vim%(%a+%):', ''), vim.log.levels.ERROR)
  end
end

---`gf` in orca-term's review tab: goes to the worktree's tab (making it when missing) and edits the
---file under the cursor there, by its absolute path, in a window that is not the sidebar nor the
---agent's terminal. On the right-hand window, the worktree's own file, the cursor lands on the same
---line. A file the change deleted is not on disk: that is told, and nothing opens.
local function goto_file()
  local tab = vim.api.nvim_get_current_tabpage()
  local path = M.worktree_of(tab)
  local view = view_in(tab)
  if not (path and view and view.infer_cur_file) then
    fallback_gf(view)
    return
  end

  -- The file of the panel's line under the cursor, or the file the diff windows show; `nil` on a
  -- directory of the panel's tree or a line that is no file.
  local file = view:infer_cur_file()
  if not file then
    return
  end
  local absolute = file.absolute_path
  if vim.fn.filereadable(absolute) == 0 then
    local message = ('orca-term: %s is not on disk: the change deleted it'):format(file.path)
    vim.notify(message, vim.log.levels.WARN)
    return
  end

  -- The right-hand window, the worktree's file, is the main window of diffview's layout.
  local line = nil
  local main = view.cur_layout and view.cur_layout:get_main_win()
  if main and file == view.cur_entry and vim.api.nvim_get_current_win() == main.id then
    line = vim.api.nvim_win_get_cursor(main.id)[1]
  end

  require('orca-term.tabs').go_to(path)
  edit_here(absolute, line)
end

---Puts orca-term's `gf` on `buf`, local to it, over diffview's own.
---@param buf integer
local function map_gf(buf)
  if not vim.api.nvim_buf_is_valid(buf) then
    return
  end
  vim.keymap.set('n', 'gf', goto_file, {
    buffer = buf,
    nowait = true,
    silent = true,
    desc = "orca-term: edit the file under the cursor in its worktree's tab",
  })
end

---After a diffview event in orca-term's review tab (or the one being opened), puts orca-term's `gf`
---on the current buffer, the diff window diffview just filled, and on the view's file panel.
---diffview sets its own buffer-local `gf` on a diff buffer each time it shows it, just before
---`DiffviewDiffBufWinEnter`, so setting ours on that event, every time, keeps ours on top.
local function on_diffview_event()
  local tab = vim.api.nvim_get_current_tabpage()
  if not (opening or M.worktree_of(tab)) then
    return
  end
  map_gf(vim.api.nvim_get_current_buf())
  local view = view_in(tab)
  if view and view.panel and view.panel.bufid then
    map_gf(view.panel.bufid)
  end
end

---Makes the autocommands of `gf` once, on the first review opened.
local function ensure_autocmds()
  if vim.fn.exists('#' .. GROUP) == 1 then
    return
  end
  local group = vim.api.nvim_create_augroup(GROUP, { clear = true })
  vim.api.nvim_create_autocmd('User', {
    group = group,
    pattern = { 'DiffviewViewOpened', 'DiffviewViewEnter', 'DiffviewDiffBufWinEnter' },
    desc = "orca-term: gf in a review edits the file in the worktree's tab",
    callback = on_diffview_event,
  })
end

---Opens the review of the worktree at `path` against `base`, from the worktree's tab (the caller
---has gone there). When the worktree's review tab is already open, goes to it and has diffview
---read the changes again instead of opening a second one. Otherwise opens diffview with
---`-C<path> <base>` in a new tab and marks it as the worktree's review. Returns an error when
---diffview is not installed or opened no view.
---@param path string
---@param base string the review's base, a full commit id
---@return orca_term.Error? err
function M.open(path, base)
  local dv = diffview()
  if not dv then
    return {
      kind = 'no_diffview',
      message = 'orca-term: the review needs diffview.nvim (' .. M.DIFFVIEW_URL .. ')',
    }
  end

  local existing = M.find(path)
  if existing then
    vim.api.nvim_set_current_tabpage(existing)
    dv.emit('refresh_files')
    return nil
  end

  ensure_autocmds()
  local before = vim.api.nvim_get_current_tabpage()
  opening = true
  -- diffview reads `-C`'s value through expand(), which would expand a `$`, `%` or `#` in the path:
  -- fnameescape() keeps it literal. The arguments are a list, so spaces need nothing more.
  local opened, problem = pcall(dv.open, { '-C' .. vim.fn.fnameescape(path), base })
  opening = false

  local view = view_in(vim.api.nvim_get_current_tabpage())
  if not opened or not view or view.tabpage == before then
    local why = opened and 'it opened no view' or tostring(problem)
    return {
      kind = 'failed',
      message = ('orca-term: diffview could not open the review of %s: %s'):format(path, why),
    }
  end
  vim.t[view.tabpage][M.MARK] = resolved(path)
  on_diffview_event()
  return nil
end

---Closes every review tab of the worktree at `path`, as after the worktree is removed: diffview
---closes its own view and tab, as `:DiffviewClose` does (opening an empty tab first when it was the
---last one).
---@param path string
function M.close_all(path)
  local wanted = resolved(path)
  for _, tab in ipairs(vim.api.nvim_list_tabpages()) do
    if vim.api.nvim_tabpage_is_valid(tab) and M.worktree_of(tab) == wanted then
      local view = view_in(tab)
      if view then
        view:close()
        require('diffview.lib').dispose_view(view)
      elseif #vim.api.nvim_list_tabpages() > 1 then
        vim.cmd('tabclose! ' .. vim.api.nvim_tabpage_get_number(tab))
      end
    end
  end
end

return M
