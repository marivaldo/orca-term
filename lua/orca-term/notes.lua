-- orca-term's notes in a review: adding them, drawing them, listing and sending them.
--
-- A note is the person's remark on a line, a range of lines or a whole file of a worktree, kept by
-- the core until it is sent to the worktree's agent (`CONTEXT.md`, **Note**). The client only shows
-- what `note ls` prints and asks the core to change it: it never works a note's line out by itself.
--
-- Notes live in orca-term's review tabs only (`review.lua`), on the right-hand side, the worktree's
-- real file, and in diffview's file panel; there are no buffers of their own. In a review tab, `gn`
-- adds a note (on the cursor's line or the selected lines of the right-hand window, on the whole
-- file under the cursor in the file panel) and `gN` lists the pending notes, to send them all or one
-- at a time, edit or delete them; `:OrcaTerm notes` lists them too, from the worktree's tab
-- (`actions.lua`). Each note is drawn with extmarks in one namespace, at its line in the file on disk
-- now (the core's `current` anchor): a sign on its line (the other lines of a range get a fainter
-- one) and its body below, or below line 1 for a whole-file note; the file panel shows `✎N` after
-- each file with notes.
--
-- The right-hand buffer is the file's own buffer, which `gf` also edits in the worktree's tab, so
-- nothing of ours may stay on it outside the review: leaving a review tab takes the keys and the
-- extmarks off every buffer they were put on, and entering one puts them back. The drawing follows
-- diffview's `User` events and the core's (`live.lua`); the autocommands are made by the first
-- review opened, never at startup.
--
-- This module, `sidebar.lua`, `tabs.lua`, `terminal.lua` and `review.lua` are the only ones that
-- touch windows, tabs and buffers (`cargo xtask client-boundary` checks it). It asks `review.lua`
-- what a review shows, so that diffview is called from there only, and runs the core through
-- `core.lua`.

local core = require('orca-term.core')
local highlights = require('orca-term.highlights')
local live = require('orca-term.live')
local render = require('orca-term.render')
local review = require('orca-term.review')

local M = {}

---The namespace of every extmark a note is drawn with.
M.NAMESPACE = vim.api.nvim_create_namespace('orca-term-notes')

---The question that asks what a note says.
M.PROMPT = 'Note for the agent: '

---What can be done with one note of the list, in the order shown.
M.NOTE_MENU = { 'Send', 'Edit', 'Delete', 'Cancel' }

---How many cells of a note's body the list shows.
M.BODY_CELLS = 60

---The sign of a range's lines after its first, fainter than the note's own.
M.RANGE_SIGN = '┆'

---The autocommand group of the notes.
local GROUP = 'orca-term-notes'

---What `gn` says on the review's base side.
local BASE_SIDE = "notes go on the right-hand side, the worktree's file"

---What the notes hold between calls.
---@class orca_term.NotesState
---@field notes table<string, orca_term.Note[]> each worktree's pending notes, by path, as last listed
---@field touched table<integer, boolean> the buffers holding the notes' extmarks or keys
---@field attached table<integer, boolean> the file panels whose redrawing draws the notes again
---How many background listings started; one that ends after a newer one started is dropped.
---@field lists integer
---@field subscribed boolean whether the notes follow the live fleet, while a review tab is current
local state = { notes = {}, touched = {}, attached = {}, lists = 0, subscribed = false }

local levels = vim.log.levels

---@param message string
local function info(message)
  vim.notify('orca-term: ' .. message, levels.INFO)
end

---@param message string
local function warn(message)
  vim.notify('orca-term: ' .. message, levels.WARN)
end

---Tells the person what failed. The message of an `orca_term.Error` already starts with
---`orca-term: ` and names the fix.
---@param err? orca_term.Error
local function fail(err)
  local message = err and err.message or 'orca-term: the core failed without saying why'
  vim.notify(message, levels.ERROR)
end

---`text` on one line, cut to `width` cells with `…`.
---@param text string
---@param width integer
---@return string
local function cut(text, width)
  local flat = text:gsub('%c', ' ')
  if vim.fn.strdisplaywidth(flat) <= width then
    return flat
  end
  return vim.fn.strcharpart(flat, 0, width - 1) .. '…'
end

---Where a note is now, as the list shows it: `path:N`, `path:S-E` or `path`.
---@param note orca_term.Note
---@return string
local function where(note)
  local current = note.current
  if current.scope == 'line' then
    return ('%s:%d'):format(note.path, current.line)
  elseif current.scope == 'lines' then
    return ('%s:%d-%d'):format(note.path, current.start, current['end'])
  end
  return note.path
end

---A note as the list shows it: where it is now, then its body on one line, cut.
---@param note orca_term.Note
---@return string
function M.item(note)
  return where(note) .. '  ' .. cut(note.body, M.BODY_CELLS)
end

---A note's body as virtual lines: the first after `✎ ` (`✎ file: ` for a whole-file note), the
---others under it.
---@param body string
---@param whole_file? boolean
---@return [string, string][][]
local function body_lines(body, whole_file)
  local lines = {}
  local first = render.NOTE .. (whole_file and ' file: ' or ' ')
  for index, text in ipairs(vim.split(body, '\n', { plain = true })) do
    local prefix = index == 1 and first or '  '
    local flat = text:gsub('%c', ' ')
    table.insert(lines, { { prefix .. flat, 'OrcaTermNote' } })
  end
  return lines
end

---Draws `note` on the file's buffer `buf` at its line in the file on disk now: a sign on its first
---line, a fainter one on the other lines of a range, and its body below its last line. A whole-file
---note (one whose line is gone too) is drawn below line 1, after `✎ file: `, with no sign: above
---line 1, Neovim would show it only once the window is scrolled up past the top.
---@param buf integer
---@param note orca_term.Note
local function draw_note(buf, note)
  local current = note.current
  if current.scope == 'file' then
    vim.api.nvim_buf_set_extmark(buf, M.NAMESPACE, 0, 0, {
      virt_lines = body_lines(note.body, true),
    })
    return
  end
  -- The buffer may be behind the file on disk for a moment: a line past its end is drawn on its
  -- last line.
  local count = vim.api.nvim_buf_line_count(buf)
  local first = math.min(current.line or current.start or 1, count)
  local last = math.min(current.line or current['end'] or first, count)
  vim.api.nvim_buf_set_extmark(buf, M.NAMESPACE, first - 1, 0, {
    sign_text = render.NOTE,
    sign_hl_group = 'OrcaTermNote',
  })
  for row = first, last - 1 do
    vim.api.nvim_buf_set_extmark(buf, M.NAMESPACE, row, 0, {
      sign_text = M.RANGE_SIGN,
      sign_hl_group = 'OrcaTermNote',
    })
  end
  vim.api.nvim_buf_set_extmark(buf, M.NAMESPACE, last - 1, 0, {
    virt_lines = body_lines(note.body),
  })
end

---Draws `✎N` at the end of each line of the file panel `buf` whose file has notes.
---@param buf integer
---@param rows { path: string, row: integer }[] the panel's file lines
---@param notes orca_term.Note[]
local function draw_panel(buf, rows, notes)
  vim.api.nvim_buf_clear_namespace(buf, M.NAMESPACE, 0, -1)
  local counts = {}
  for _, note in ipairs(notes) do
    counts[note.path] = (counts[note.path] or 0) + 1
  end
  for _, row in ipairs(rows) do
    if counts[row.path] then
      vim.api.nvim_buf_set_extmark(buf, M.NAMESPACE, row.row, 0, {
        virt_text = { { render.NOTE .. counts[row.path], 'OrcaTermNote' } },
        virt_text_pos = 'eol',
      })
    end
  end
end

local add_here, add_selected, list_current

---The keys the notes put on a review's buffers: mode, key, function and description.
---@type { [1]: string, [2]: string, [3]: fun(), [4]: string }[]
local KEYS = {
  {
    'n',
    'gn',
    function()
      add_here()
    end,
    'orca-term: add a note for the agent on this line, or on the whole file in the file panel',
  },
  {
    'x',
    'gn',
    function()
      add_selected()
    end,
    'orca-term: add a note for the agent on the selected lines',
  },
  {
    'n',
    'gN',
    function()
      list_current()
    end,
    "orca-term: list the worktree's pending notes, to send, edit or delete them",
  },
}

---Puts the notes' keys on `buf`, local to it; the visual `gn` only when `visual`.
---@param buf integer
---@param visual boolean
local function map_keys(buf, visual)
  for _, key in ipairs(KEYS) do
    if visual or key[1] ~= 'x' then
      vim.keymap.set(key[1], key[2], key[3], {
        buffer = buf,
        nowait = true,
        silent = true,
        desc = key[4],
      })
    end
  end
end

---Takes the notes' keys and extmarks off `buf`, leaving any other key alone.
---@param buf integer
local function forget(buf)
  if not vim.api.nvim_buf_is_valid(buf) then
    return
  end
  vim.api.nvim_buf_clear_namespace(buf, M.NAMESPACE, 0, -1)
  for _, key in ipairs(KEYS) do
    for _, map in ipairs(vim.api.nvim_buf_get_keymap(buf, key[1])) do
      if map.lhs == key[2] and map.desc == key[4] then
        vim.api.nvim_buf_del_keymap(buf, key[1], key[2])
      end
    end
  end
end

---Follows the live fleet while a review tab is current, so the notes are listed again when the
---core says a worktree changed (which a note command does), and stops following it otherwise.
---@param active boolean
local function follow_live(active)
  if active and not state.subscribed then
    state.subscribed = true
    live.subscribe('notes', function()
      M.refresh()
    end)
  elseif not active and state.subscribed then
    state.subscribed = false
    live.unsubscribe('notes')
  end
end

---Takes the notes' keys and extmarks off every buffer they are on, and stops following the live
---fleet: the current tab is being left, or is no review.
local function forget_all()
  for buf in pairs(state.touched) do
    forget(buf)
  end
  state.touched = {}
  follow_live(false)
end

---Draws the notes again whenever diffview redraws the file panel `buf`, which replaces its lines
---and so moves the extmarks; it stops once the panel no longer shows notes.
---@param buf integer
local function attach_panel(buf)
  if state.attached[buf] then
    return
  end
  state.attached[buf] = vim.api.nvim_buf_attach(buf, false, {
    on_lines = function()
      if not state.touched[buf] then
        state.attached[buf] = nil
        return true
      end
      vim.schedule(M.draw)
    end,
    on_detach = function()
      state.attached[buf] = nil
    end,
  }) or nil
end

---Draws the notes last listed in the current tab when it is orca-term's review: on the right-hand
---window's file, in the file panel, and the keys on those and on the base side's buffers. Every
---other buffer they were on loses them, so in any other tab nothing of the notes is left.
function M.draw()
  local shown = review.shown()
  local keep = {}
  if shown then
    local notes = state.notes[shown.worktree] or {}
    if shown.file then
      local buf = shown.file.buf
      vim.api.nvim_buf_clear_namespace(buf, M.NAMESPACE, 0, -1)
      for _, note in ipairs(notes) do
        if note.path == shown.file.path then
          draw_note(buf, note)
        end
      end
      map_keys(buf, true)
      keep[buf] = true
    end
    for _, buf in ipairs(shown.bases) do
      vim.api.nvim_buf_clear_namespace(buf, M.NAMESPACE, 0, -1)
      map_keys(buf, true)
      keep[buf] = true
    end
    if shown.panel then
      draw_panel(shown.panel, shown.rows, notes)
      map_keys(shown.panel, false)
      attach_panel(shown.panel)
      keep[shown.panel] = true
    end
  end
  for buf in pairs(state.touched) do
    if not keep[buf] then
      forget(buf)
    end
  end
  state.touched = keep
  follow_live(shown ~= nil)
end

---Lists the pending notes of the current review tab's worktree again through the core, without
---waiting, then draws them. In any other tab it only takes the notes off. A listing that ends
---after a newer one started is dropped, and so is a failed one (the notes drawn stay).
function M.refresh()
  local path = review.current()
  if not path then
    M.draw()
    return
  end
  state.lists = state.lists + 1
  local this_list = state.lists
  core.note_ls(path, path, function(listed)
    if this_list ~= state.lists then
      return
    end
    if listed then
      state.notes[path] = listed.notes
    end
    M.draw()
  end)
end

---After a note changed: the notes are listed and drawn again, and the sidebar reads the fleet
---again, for its `✎N`.
local function changed()
  M.refresh()
  require('orca-term.sidebar').refresh()
end

---Asks what the note says, then attaches it through the core to the file at `path` of the worktree
---at `worktree`, on `lines` (the first and the last), or on the whole file when `lines` is `nil`.
---An empty or cancelled answer adds nothing; a refusal of the core is told.
---@param worktree string
---@param path string
---@param lines? integer[]
local function ask_and_add(worktree, path, lines)
  vim.ui.input({ prompt = M.PROMPT }, function(input)
    local body = vim.trim(input or '')
    if body == '' then
      return
    end
    core.note_add(worktree, path, lines, body, worktree, function(written, err)
      if not written then
        fail(err)
        return
      end
      changed()
    end)
  end)
end

---Adds a note on the lines `first` to `last` of the file the right-hand window shows. The core
---anchors a note on the file on disk, so a buffer with unsaved changes is refused.
---@param shown orca_term.ReviewShown
---@param first integer
---@param last integer
local function add_on_lines(shown, first, last)
  local file = assert(shown.file)
  if vim.bo[file.buf].modified then
    warn(
      ('%s has unsaved changes; write it first (:w), notes go on the file on disk'):format(
        file.path
      )
    )
    return
  end
  ask_and_add(shown.worktree, file.path, { first, last })
end

---`gn` in normal mode in a review: a note on the cursor's line in the right-hand window, a note on
---the whole file under the cursor in the file panel; on the base side it only says where notes go.
add_here = function()
  local shown = review.shown()
  if not shown then
    return
  end
  if shown.file and vim.api.nvim_get_current_win() == shown.file.win then
    local line = vim.api.nvim_win_get_cursor(shown.file.win)[1]
    add_on_lines(shown, line, line)
  elseif shown.panel and vim.api.nvim_get_current_buf() == shown.panel then
    local path = review.file_in_panel()
    if not path then
      warn('put the cursor on a file to add a note on the whole file')
      return
    end
    ask_and_add(shown.worktree, path, nil)
  else
    warn(BASE_SIDE)
  end
end

---`gn` in visual mode in a review: a note on the selected lines of the right-hand window (one line
---selected is a note on that line); on the base side it only says where notes go.
add_selected = function()
  local first, last = vim.fn.line('v'), vim.fn.line('.')
  vim.cmd.normal({ args = { vim.keycode('<Esc>') }, bang = true })
  local shown = review.shown()
  if not shown then
    return
  end
  if shown.file and vim.api.nvim_get_current_win() == shown.file.win then
    add_on_lines(shown, math.min(first, last), math.max(first, last))
  else
    warn(BASE_SIDE)
  end
end

---Sends the notes `ids` (every pending note when empty) of the worktree at `path`, named `name`,
---to its agent through the core, then tells how many went. With no running agent, it says how to
---start it; the notes stay pending.
---@param path string
---@param name string
---@param ids integer[]
local function send(path, name, ids)
  core.note_send(path, ids, path, function(sent, err)
    if sent then
      local count = #sent.sent
      info(('sent %d %s to %s'):format(count, count == 1 and 'note' or 'notes', sent.name))
      changed()
    elseif err and err.kind == 'no_holder' then
      fail({
        kind = err.kind,
        message = (
          'orca-term: %s has no running agent to send notes to; press <CR> on it in the sidebar '
          .. 'to start it, then send again (the notes stay pending)'
        ):format(name),
      })
    else
      fail(err)
    end
  end)
end

---Asks what the note says now, its body offered to change, and replaces it through the core. An
---empty, cancelled or unchanged answer changes nothing.
---@param path string the worktree's path
---@param note orca_term.Note
local function edit(path, note)
  vim.ui.input({ prompt = M.PROMPT, default = note.body }, function(input)
    local body = vim.trim(input or '')
    if body == '' or body == note.body then
      return
    end
    core.note_edit(path, note.id, body, path, function(written, err)
      if not written then
        fail(err)
        return
      end
      changed()
    end)
  end)
end

---Removes the note through the core, without asking first, as Orca does.
---@param path string the worktree's path
---@param note orca_term.Note
local function delete(path, note)
  core.note_rm(path, note.id, path, function(removed, err)
    if not removed then
      fail(err)
      return
    end
    changed()
  end)
end

---Asks what to do with one note of the list: send it alone, edit it, delete it, or nothing.
---@param path string the worktree's path
---@param name string the worktree's name
---@param note orca_term.Note
local function note_menu(path, name, note)
  vim.ui.select(M.NOTE_MENU, { prompt = M.item(note) }, function(choice)
    if choice == M.NOTE_MENU[1] then
      send(path, name, { note.id })
    elseif choice == M.NOTE_MENU[2] then
      edit(path, note)
    elseif choice == M.NOTE_MENU[3] then
      delete(path, note)
    end
  end)
end

---Lists the pending notes of the worktree at `path` through the core, without waiting, and lets the
---person pick: first `Send all (N)`, then each note (`M.item`), which asks what to do with it. With
---no pending notes it says so.
---@param path string
function M.list(path)
  core.note_ls(path, path, function(listed, err)
    if not listed then
      fail(err)
      return
    end
    state.notes[path] = listed.notes
    M.draw()
    if #listed.notes == 0 then
      info('no pending notes in ' .. listed.name)
      return
    end
    local items = { { label = ('Send all (%d)'):format(#listed.notes) } }
    for _, note in ipairs(listed.notes) do
      table.insert(items, { note = note })
    end
    local options = {
      prompt = 'Notes for ' .. listed.name,
      format_item = function(item)
        return item.label or M.item(item.note)
      end,
    }
    vim.ui.select(items, options, function(choice)
      if choice and choice.note then
        note_menu(path, listed.name, choice.note)
      elseif choice then
        send(path, listed.name, {})
      end
    end)
  end)
end

---`gN` in a review: the list of the reviewed worktree's pending notes.
list_current = function()
  local path = review.current()
  if path then
    M.list(path)
  end
end

---Makes the notes' autocommands and highlight groups, on the first review opened: diffview's
---events draw the notes again (a diff window filled, the review entered), leaving the review takes
---them off, and a file reloaded or written in the review lists them again, as its lines moved.
function M.start()
  highlights.define()
  if vim.fn.exists('#' .. GROUP) == 1 then
    return
  end
  local group = vim.api.nvim_create_augroup(GROUP, { clear = true })
  vim.api.nvim_create_autocmd('User', {
    group = group,
    pattern = { 'DiffviewDiffBufRead', 'DiffviewDiffBufWinEnter', 'DiffviewViewClosed' },
    desc = 'orca-term: draw the notes of the review again',
    callback = function()
      M.draw()
    end,
  })
  vim.api.nvim_create_autocmd('User', {
    group = group,
    pattern = 'DiffviewViewEnter',
    desc = 'orca-term: draw the notes of the review entered, and list them again',
    callback = function()
      M.draw()
      M.refresh()
    end,
  })
  vim.api.nvim_create_autocmd('User', {
    group = group,
    pattern = 'DiffviewViewLeave',
    desc = 'orca-term: take the notes off the buffers of the review left',
    callback = forget_all,
  })
  vim.api.nvim_create_autocmd({ 'FileChangedShellPost', 'BufWritePost' }, {
    group = group,
    desc = 'orca-term: list the notes again, the file under them changed on disk',
    callback = function(args)
      if state.touched[args.buf] then
        M.refresh()
      end
    end,
  })
  vim.api.nvim_create_autocmd('ColorScheme', {
    group = group,
    desc = "orca-term: define the notes' highlight groups again",
    callback = highlights.define,
  })
end

---Forgets the notes: takes them off every buffer, stops following the live fleet and deletes the
---autocommands, for the specs.
function M._reset()
  forget_all()
  state.notes = {}
  pcall(vim.api.nvim_del_augroup_by_name, GROUP)
end

return M
