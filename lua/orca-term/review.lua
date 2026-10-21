-- orca-term's review tabs: a worktree's review shown in diffview.nvim, in a tab of its own.
--
-- This module, `sidebar.lua`, `detail.lua`, `tabs.lua`, `terminal.lua` and `notes.lua` are the only
-- ones that touch windows, tabs and buffers (`cargo xtask client-boundary` checks it), and this is
-- the only module that calls diffview.nvim. diffview has no type annotations LuaLS knows, so its
-- modules and views are plain `table`s here, and only the few fields and functions named below are
-- used.
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
-- A turn review shows what one turn of the worktree's agent changed: diffview on `<begin>..<end>`,
-- the turn's two snapshots, in a tab of its own marked `t:orca_term_turn` (the worktree's path and
-- the turn's number) instead of `t:orca_term_review`. It is a review tab like the other (the
-- sidebar is not shown there, `tabs.lua` leaves it out, `gf` edits the file in the worktree's tab,
-- removing the worktree closes it), but never the worktree's review: `find` and `d` never go to it,
-- and the notes are not drawn there, since they go on the files on disk.
--
-- The autocommands of `gf` are made by the first review opened, never at startup. The notes drawn
-- in orca-term's review tabs are `notes.lua`'s; this module tells it what a review shows
-- (`shown()`, `file_in_panel()`), so that diffview is called from here only.

local M = {}

---The tab variable that marks orca-term's review tabs, holding the worktree's path.
M.MARK = 'orca_term_review'

---The tab variable that marks orca-term's turn reviews, holding `{ path = <worktree>, number = N }`.
M.TURN_MARK = 'orca_term_turn'

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

---The worktree path the current tab is orca-term's review of, or `nil`. A turn review is not the
---worktree's review: it gives `nil` there too.
---@return string?
function M.current()
  return M.worktree_of(vim.api.nvim_get_current_tabpage())
end

---The turn `tab` is orca-term's turn review of, its worktree's path and the turn's number, or
---`nil` when it is no turn review.
---@param tab integer
---@return { path: string, number: integer }?
function M.turn_of(tab)
  return vim.t[tab][M.TURN_MARK]
end

---The worktree path `tab` is a review of, orca-term's review or one of its turn reviews; `nil`
---for any other tab, a diffview opened by hand included.
---@param tab integer
---@return string?
local function reviewed_in(tab)
  local turn = M.turn_of(tab)
  return M.worktree_of(tab) or (turn and turn.path)
end

---The worktree path the current tab is a review of, orca-term's review or one of its turn
---reviews, or `nil`.
---@return string?
function M.reviewed()
  return reviewed_in(vim.api.nvim_get_current_tabpage())
end

---Whether `tab` is a review tab: it holds a diffview view, orca-term marked it as a review or a
---turn review, or it is the tab a review is being opened in.
---@param tab integer
---@return boolean
function M.is_review(tab)
  if opening and tab == vim.api.nvim_get_current_tabpage() then
    return true
  end
  return reviewed_in(tab) ~= nil or view_in(tab) ~= nil
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

---The open turn review of turn `number` of the worktree at `path`: a tab orca-term marked as that
---turn's review, whose diffview view is still there. `nil` when there is none.
---@param path string
---@param number integer
---@return integer?
function M.find_turn(path, number)
  local wanted = resolved(path)
  for _, tab in ipairs(vim.api.nvim_list_tabpages()) do
    local turn = M.turn_of(tab)
    if turn and turn.path == wanted and turn.number == number and view_in(tab) then
      return tab
    end
  end
  return nil
end

---The buffers of orca-term's turn review in the current tab, its diff windows' and its file
---panel's, on which `notes.lua` puts the `gn` that says notes are added from the worktree's review;
---`nil` when the current tab is no turn review.
---@return integer[]?
function M.turn_buffers()
  local tab = vim.api.nvim_get_current_tabpage()
  if not M.turn_of(tab) then
    return nil
  end
  local buffers = {}
  for win in pairs(M.view_windows(tab)) do
    if vim.api.nvim_win_is_valid(win) then
      table.insert(buffers, vim.api.nvim_win_get_buf(win))
    end
  end
  return buffers
end

---What orca-term's review in the current tab shows, for the notes drawn on it (`notes.lua`).
---@class orca_term.ReviewShown
---@field worktree string the worktree's path
---The right-hand window, when it shows the worktree's file on disk: the window, its buffer (the
---file's own buffer, which `gf` edits too) and the file's path, relative to the worktree's root
---@field file? { win: integer, buf: integer, path: string }
---@field bases integer[] the buffers of the other diff windows, the review's base
---@field panel? integer the file panel's buffer, when it is loaded
---@field rows { path: string, row: integer }[] each file the panel shows, on its 0-based line

---The lines of the file panel holding a file, from the components diffview rendered: a file
---inside a folded directory takes no line and is left out.
---@param panel table diffview's file panel
---@return { path: string, row: integer }[]
local function panel_rows(panel)
  local rows = {}
  local root = panel.components and panel.components.comp
  if not root then
    return rows
  end
  root:deep_some(function(comp)
    if comp.name == 'file' and comp.context and comp.lend > comp.lstart then
      table.insert(rows, { path = comp.context.path, row = comp.lstart })
    end
    return false
  end)
  return rows
end

---The right-hand window `main` of a review's layout, when it shows the worktree's file on disk
---(diffview's `LOCAL` revision), with its buffer and the file's path; `nil` while it shows anything
---else, such as the empty buffer of a file the change deleted.
---@param main table diffview's window
---@return { win: integer, buf: integer, path: string }?
local function file_on_disk(main)
  local file = main.file
  if not (file and file.bufnr and main.id and vim.api.nvim_win_is_valid(main.id)) then
    return nil
  end
  local local_rev = require('diffview.vcs.rev').RevType.LOCAL
  if file.rev.type ~= local_rev or vim.api.nvim_win_get_buf(main.id) ~= file.bufnr then
    return nil
  end
  return { win = main.id, buf = file.bufnr, path = file.path }
end

---What orca-term's review in the current tab shows: its worktree, the right-hand window's file, the
---base side's buffers and the file panel. `nil` when the current tab is not orca-term's review (a
---diffview opened by hand is not), or its view has no layout yet.
---@return orca_term.ReviewShown?
function M.shown()
  local tab = vim.api.nvim_get_current_tabpage()
  local path = M.worktree_of(tab)
  local view = view_in(tab)
  local layout = view and view.cur_layout
  if not (path and view and layout) then
    return nil
  end
  local main = layout:get_main_win()
  local bases = {}
  for _, win in ipairs(layout.windows or {}) do
    if win ~= main and win.id and vim.api.nvim_win_is_valid(win.id) then
      table.insert(bases, vim.api.nvim_win_get_buf(win.id))
    end
  end
  local panel = view.panel
  local panel_buf = panel and panel.bufid
  if panel_buf and not vim.api.nvim_buf_is_loaded(panel_buf) then
    panel_buf = nil
  end
  return {
    worktree = path,
    file = main and file_on_disk(main),
    bases = bases,
    panel = panel_buf,
    rows = panel and panel_rows(panel) or {},
  }
end

---The path (relative to the worktree's root) of the file under the cursor in the file panel of the
---current tab's review, or `nil` on a directory or a line that is no file.
---@return string?
function M.file_in_panel()
  local view = view_in(vim.api.nvim_get_current_tabpage())
  if not (view and view.infer_cur_file) then
    return nil
  end
  local file = view:infer_cur_file()
  return file and file.path
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

---`gf` in orca-term's review tab or turn review: goes to the worktree's tab (making it when
---missing) and edits the file under the cursor there, by its absolute path, in a window that is not
---the sidebar nor the agent's terminal. From the right-hand window the cursor lands on the same
---line. A file not on disk (the change, or a later one, deleted it) is told, and nothing opens.
local function goto_file()
  local tab = vim.api.nvim_get_current_tabpage()
  local path = reviewed_in(tab)
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

---After a diffview event in orca-term's review tab or turn review (or the one being opened), puts
---orca-term's `gf` on the current buffer, the diff window diffview just filled, and on the view's
---file panel. diffview sets its own buffer-local `gf` on a diff buffer each time it shows it, just
---before `DiffviewDiffBufWinEnter`, so setting ours on that event, every time, keeps ours on top.
local function on_diffview_event()
  local tab = vim.api.nvim_get_current_tabpage()
  if not (opening or reviewed_in(tab)) then
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

---Opens diffview on `revisions` (a commit, or a range `A..B`) of the worktree at `path`, in the new
---tab diffview makes, and returns that tab; or `nil` and an error naming `what` when diffview
---failed or opened no view.
---@param dv table diffview's main module
---@param path string
---@param revisions string
---@param what string what is being opened, for the error
---@return integer? tab
---@return orca_term.Error? err
local function open_diffview(dv, path, revisions, what)
  local before = vim.api.nvim_get_current_tabpage()
  opening = true
  -- diffview reads `-C`'s value through expand(), which would expand a `$`, `%` or `#` in the path:
  -- fnameescape() keeps it literal. The arguments are a list, so spaces need nothing more.
  local opened, problem = pcall(dv.open, { '-C' .. vim.fn.fnameescape(path), revisions })
  opening = false

  local view = view_in(vim.api.nvim_get_current_tabpage())
  if not opened or not view or view.tabpage == before then
    local why = opened and 'it opened no view' or tostring(problem)
    return nil,
      {
        kind = 'failed',
        message = ('orca-term: diffview could not open %s: %s'):format(what, why),
      }
  end
  return view.tabpage, nil
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

  local notes = require('orca-term.notes')
  local existing = M.find(path)
  if existing then
    vim.api.nvim_set_current_tabpage(existing)
    dv.emit('refresh_files')
    notes.refresh()
    return nil
  end

  ensure_autocmds()
  notes.start()
  local tab, err = open_diffview(dv, path, base, 'the review of ' .. path)
  if not tab then
    return err
  end
  vim.t[tab][M.MARK] = resolved(path)
  on_diffview_event()
  notes.refresh()
  return nil
end

---Opens the turn review of `turn` of the worktree at `path`, from the worktree's tab (the caller has
---gone there): diffview with `-C<path> <begin>..<end>` in a new tab, marked as that turn's review,
---so it shows exactly what the turn changed. When that turn's review is already open, goes to it.
---Returns an error when diffview is not installed or opened no view. The turn must have both
---snapshots.
---@param path string
---@param turn orca_term.Turn
---@return orca_term.Error? err
function M.open_turn(path, turn)
  local dv = diffview()
  if not dv then
    return {
      kind = 'no_diffview',
      message = 'orca-term: the review needs diffview.nvim (' .. M.DIFFVIEW_URL .. ')',
    }
  end

  local notes = require('orca-term.notes')
  local existing = M.find_turn(path, turn.number)
  if existing then
    vim.api.nvim_set_current_tabpage(existing)
    notes.draw()
    return nil
  end

  ensure_autocmds()
  notes.start()
  local range = assert(turn.begin) .. '..' .. assert(turn['end'])
  local what = ('turn %d of %s'):format(turn.number, path)
  local tab, err = open_diffview(dv, path, range, what)
  if not tab then
    return err
  end
  vim.t[tab][M.TURN_MARK] = { path = resolved(path), number = turn.number }
  on_diffview_event()
  notes.draw()
  return nil
end

---Closes every review tab of the worktree at `path`, its turn reviews included, as after the
---worktree is removed: diffview closes its own view and tab, as `:DiffviewClose` does (opening an
---empty tab first when it was the last one).
---@param path string
function M.close_all(path)
  local wanted = resolved(path)
  for _, tab in ipairs(vim.api.nvim_list_tabpages()) do
    if vim.api.nvim_tabpage_is_valid(tab) and reviewed_in(tab) == wanted then
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
