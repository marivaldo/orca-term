-- Seam 2: notes in the review, in headless Neovim against the real binary and diffview.nvim. In
-- orca-term's review tab, `gn` adds a note through the core's `note add` (on the cursor's line or
-- the selected lines of the right-hand window, on the whole file in the file panel), each note is
-- drawn with extmarks at its line in the file on disk now, the file panel counts them, and `gN` (or
-- `:OrcaTerm notes`) lists them to send them all or one, edit or delete them; sending types them
-- into the worktree's agent, the fake claude, which records what it reads. Nothing of the notes
-- stays on the file's buffer outside the review.
--
-- diffview.nvim comes from .rocks/diffview.nvim, as in spec/review_spec.lua: every spec closes every
-- view and review tab it opened, and deletes the autocommand groups of the review and the notes.

local support = require('support')
local notes = require('orca-term.notes')
local sidebar = require('orca-term.sidebar')
local tabs = require('orca-term.tabs')

---Where `just lua-rocks` puts diffview.nvim.
local DIFFVIEW = support.root .. '/.rocks/diffview.nvim'

---diffview's global state, `DiffviewGlobal`, which holds its logger.
---@return table
local function diffview_global()
  -- selene: allow(global_usage)
  return rawget(_G, 'DiffviewGlobal')
end

---The fixture directory of the spec's repository, whose files `after_each` wipes from the buffers.
---@type string?
local fixture_dir = nil

---@class spec.NotesRepo: spec.AgentRepo
---@field path string the worktree `fix`

---A repository whose worktree `fix` (made by the core) has a commit of its own (`added.txt`, five
---lines) and an uncommitted change (`tracked.txt`); with `agent`, its agent (the fake claude) is
---started. Neovim is in the primary checkout, sidebar open.
---@param agent boolean
---@return spec.NotesRepo
local function notes_repo(agent)
  support.load_client()
  local repo = support.agent_repo() --[[@as spec.NotesRepo]]
  fixture_dir = repo.tmp
  vim.fn.writefile({ 'one' }, repo.root .. '/tracked.txt')
  support.git(repo, { 'add', '.' })
  support.git(repo, { 'commit', '--quiet', '-m', 'base' })

  support.run({ support.binary, 'worktree', 'new', 'fix' }, repo.root)
  repo.path = support.worktree_path(repo, 'fix')
  if agent then
    support.run({ support.binary, 'agent', 'start', 'fix' }, repo.root)
  end
  vim.fn.writefile({ 'a1', 'a2', 'a3', 'a4', 'a5' }, repo.path .. '/added.txt')
  support.run({ 'git', 'add', 'added.txt' }, repo.path)
  support.run({ 'git', 'commit', '--quiet', '-m', 'own' }, repo.path)
  vim.fn.writefile({ 'one', 'two' }, repo.path .. '/tracked.txt')

  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  return repo
end

---The sidebar's window in the current tab.
---@return integer
local function sidebar_window()
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_buf(win) == sidebar.buffer() then
      return win
    end
  end
  error('no sidebar in the current tab')
end

---The sidebar's name line of the worktree `name`.
---@param name string
---@return string
local function sidebar_row(name)
  for _, line in ipairs(vim.api.nvim_buf_get_lines(assert(sidebar.buffer()), 0, -1, false)) do
    if line:find(' ' .. name, 1, true) then
      return line
    end
  end
  error('no sidebar line for ' .. name)
end

---Whether `buf` holds a buffer-local map of `lhs` in `mode` made by orca-term.
---@param buf integer
---@param mode string
---@param lhs string
---@return boolean
local function has_our_map(buf, mode, lhs)
  for _, map in ipairs(vim.api.nvim_buf_get_keymap(buf, mode)) do
    if map.lhs == lhs and tostring(map.desc):find('^orca%-term') then
      return true
    end
  end
  return false
end

---@class spec.Review
---@field tab integer the review tab
---@field view table diffview's view
---@field main integer the right-hand window, showing `added.txt`
---@field base integer the left-hand window, the review's base
---@field buf integer the buffer of `added.txt`

---Opens the review of `fix` with `d` in the sidebar and waits until its right-hand window shows
---`added.txt` with the notes' `gn` on it.
---@param repo spec.NotesRepo
---@return spec.Review
local function open_review(repo)
  local win = sidebar_window()
  vim.api.nvim_set_current_win(win)
  for index, line in ipairs(vim.api.nvim_buf_get_lines(assert(sidebar.buffer()), 0, -1, false)) do
    if line:find(' fix', 1, true) then
      vim.api.nvim_win_set_cursor(win, { index, 0 })
    end
  end
  vim.cmd('normal d')
  local added = repo.path .. '/added.txt'
  local review = {}
  support.wait_for(function()
    local view = require('diffview.lib').get_current_view()
    if not (view and view.cur_layout and view.cur_entry) then
      return false
    end
    local main = view.cur_layout:get_main_win()
    local buf = vim.api.nvim_win_get_buf(main.id)
    if vim.api.nvim_buf_get_name(buf) ~= added or not has_our_map(buf, 'n', 'gn') then
      return false
    end
    review = {
      tab = vim.api.nvim_get_current_tabpage(),
      view = view,
      main = main.id,
      base = view.cur_layout.a.id,
      buf = buf,
    }
    return true
  end, "the review to show added.txt with the notes' gn")
  return review
end

---The notes of `fix` as the core lists them.
---@param repo spec.NotesRepo
---@return orca_term.Note[]
local function listed_notes(repo)
  local out = support.run({ support.binary, 'note', 'ls', 'fix', '--json' }, repo.root)
  return vim.json.decode(out).notes
end

---Adds a note to `fix` through the core, as another client would.
---@param repo spec.NotesRepo
---@param args string[] the path, then the flags, such as `{ 'added.txt', '--line', '3' }`
---@param body string
local function add_by_hand(repo, args, body)
  local command = { support.binary, 'note', 'add', 'fix' }
  vim.list_extend(command, args)
  table.insert(command, '--body=' .. body)
  support.run(command, repo.root)
end

---What the notes drew on `buf`: every sign as `<line>:<sign>` (1-based), every virtual line as
---`<line>:<text>` (`<line>^:` for one above it), sorted.
---@param buf integer
---@return string[]
local function drawn(buf)
  local found = {}
  local marks = vim.api.nvim_buf_get_extmarks(buf, notes.NAMESPACE, 0, -1, { details = true })
  for _, mark in ipairs(marks) do
    local line, details = mark[2] + 1, mark[4] or {}
    if details.sign_text then
      table.insert(found, ('%d:%s'):format(line, vim.trim(details.sign_text)))
    end
    for _, chunks in ipairs(details.virt_lines or {}) do
      local above = details.virt_lines_above and '^' or ''
      table.insert(found, ('%d%s:%s'):format(line, above, chunks[1][1]))
    end
    for _, chunk in ipairs(details.virt_text or {}) do
      table.insert(found, ('%d:%s'):format(line, chunk[1]))
    end
  end
  table.sort(found)
  return found
end

---Waits until the notes drew exactly `wanted` on `buf`.
---@param buf integer
---@param wanted string[]
local function wait_drawn(buf, wanted)
  table.sort(wanted)
  local ok, why = vim.wait(20000, function()
    return vim.deep_equal(drawn(buf), wanted)
  end, 20)
  assert.are.same(wanted, drawn(buf))
  assert(ok, 'vim.wait: ' .. tostring(why))
end

---The 1-based line of the file panel holding `name`.
---@param review spec.Review
---@param name string
---@return integer
local function panel_line(review, name)
  local lines = vim.api.nvim_buf_get_lines(review.view.panel.bufid, 0, -1, false)
  for index, line in ipairs(lines) do
    if line:find(name, 1, true) then
      return index
    end
  end
  error('no panel line for ' .. name)
end

---Presses `keys` (through maps) in window `win`.
---@param win integer
---@param keys string
local function press(win, keys)
  vim.api.nvim_set_current_win(win)
  vim.cmd('normal ' .. vim.keycode(keys))
end

---Waits until the fake claude of `repo` has read `count` lines or more, and returns them all.
---@param repo spec.NotesRepo
---@param count integer
---@return string[]
local function claude_read(repo, count)
  support.wait_for(function()
    return #support.claude_input(repo) >= count
  end, 'the fake claude to read the prompt')
  return support.claude_input(repo)
end

describe('notes', function()
  setup(function()
    vim.opt.runtimepath:append(DIFFVIEW)
    require('diffview').setup({ use_icons = false })
    diffview_global().logger.outfile = vim.fn.tempname()
  end)

  teardown(function()
    local logger = diffview_global().logger
    if logger.batch_handle then
      logger.batch_handle.close()
      logger.batch_handle = nil
    end
    pcall(vim.api.nvim_del_augroup_by_name, 'diffview_nvim')
    pcall(vim.api.nvim_del_augroup_by_name, 'diffview_panels')
    vim.opt.runtimepath:remove(DIFFVIEW)
  end)

  after_each(function()
    local lib = require('diffview.lib')
    for _, view in ipairs(vim.list_slice(lib.views)) do
      pcall(view.close, view)
      lib.dispose_view(view)
    end
    notes._reset()
    pcall(vim.api.nvim_del_augroup_by_name, 'orca-term-review')
    support.reset_editor()
    for _, buf in ipairs(vim.api.nvim_list_bufs()) do
      local name = vim.api.nvim_buf_get_name(buf)
      if fixture_dir and vim.startswith(name, fixture_dir .. '/') then
        pcall(vim.api.nvim_buf_delete, buf, { force = true })
      end
    end
    fixture_dir = nil
    support.cleanup()
  end)

  it('gn adds a note on a line, on the selected lines, and on a whole file', function()
    local repo = notes_repo(false)
    local review = open_review(repo)
    local ui = support.stub_ui({ input = '  on a line  ' })

    vim.api.nvim_win_set_cursor(review.main, { 2, 0 })
    press(review.main, 'gn')
    wait_drawn(review.buf, { '2:✎', '2:✎ on a line' })
    assert.are.same({ notes.PROMPT }, ui.prompts)

    ui = support.stub_ui({ input = 'a range' })
    vim.api.nvim_win_set_cursor(review.main, { 3, 0 })
    press(review.main, 'Vjgn')
    wait_drawn(review.buf, { '2:✎', '2:✎ on a line', '3:✎', '4:┆', '4:✎ a range' })
    assert.are.equal('n', vim.fn.mode())

    -- One line selected is a note on that line.
    ui = support.stub_ui({ input = 'one selected' })
    vim.api.nvim_win_set_cursor(review.main, { 5, 0 })
    press(review.main, 'Vgn')
    support.wait_for(function()
      return #listed_notes(repo) == 3
    end, 'the third note')

    ui = support.stub_ui({ input = 'the whole file' })
    local panel = review.view.panel
    vim.api.nvim_set_current_win(panel.winid)
    vim.api.nvim_win_set_cursor(panel.winid, { panel_line(review, 'tracked.txt'), 0 })
    press(panel.winid, 'gn')
    support.wait_for(function()
      return #listed_notes(repo) == 4
    end, 'the whole-file note')

    local anchors = vim.tbl_map(function(note)
      return { note.path, note.anchor, note.body }
    end, listed_notes(repo))
    assert.are.same({
      { 'added.txt', { scope = 'line', line = 2 }, 'on a line' },
      { 'added.txt', { scope = 'lines', start = 3, ['end'] = 4 }, 'a range' },
      { 'added.txt', { scope = 'line', line = 5 }, 'one selected' },
      { 'tracked.txt', { scope = 'file' }, 'the whole file' },
    }, anchors)
    assert.are.same({}, ui.notes)

    -- The file panel counts each file's notes at the end of its line.
    local added_row, tracked_row =
      panel_line(review, 'added.txt'), panel_line(review, 'tracked.txt')
    wait_drawn(panel.bufid, { added_row .. ':✎3', tracked_row .. ':✎1' })
    -- And so does the sidebar's row, in the worktree's tab.
    vim.api.nvim_set_current_tabpage((assert(tabs.find_all(repo.path)[1])))
    support.wait_for(function()
      return sidebar_row('fix'):find(' ✎4', 1, true) ~= nil
    end, 'the sidebar to count the notes')
  end)

  it('adds nothing on the base side, on unsaved changes, or with an empty answer', function()
    local repo = notes_repo(false)
    local review = open_review(repo)

    local ui = support.stub_ui({ input = 'never asked' })
    press(review.base, 'gn')
    assert.are.same({}, ui.prompts)
    assert.are.same({
      {
        message = "orca-term: notes go on the right-hand side, the worktree's file",
        level = vim.log.levels.WARN,
      },
    }, ui.notes)

    ui = support.stub_ui({ input = '   ' })
    press(review.main, 'gn')
    assert.are.same({ notes.PROMPT }, ui.prompts)

    vim.api.nvim_buf_set_lines(review.buf, 0, 0, false, { 'unsaved' })
    ui = support.stub_ui({ input = 'never asked' })
    press(review.main, 'gn')
    assert.are.same({}, ui.prompts)
    assert.are.same({
      {
        message = 'orca-term: added.txt has unsaved changes; write it first (:w), '
          .. 'notes go on the file on disk',
        level = vim.log.levels.WARN,
      },
    }, ui.notes)

    vim.wait(300)
    assert.are.same({}, listed_notes(repo))
  end)

  it('draws each note where its line is on disk now, and a gone line as the whole file', function()
    local repo = notes_repo(false)
    add_by_hand(repo, { 'added.txt', '--line', '3' }, 'third')
    local review = open_review(repo)
    wait_drawn(review.buf, { '3:✎', '3:✎ third' })

    -- Two lines written above it, on disk: the note moves down with its line.
    local added = repo.path .. '/added.txt'
    vim.fn.writefile({ 'n1', 'n2', 'a1', 'a2', 'a3', 'a4', 'a5' }, added)
    vim.cmd('checktime')
    wait_drawn(review.buf, { '5:✎', '5:✎ third' })

    -- Its line deleted: the note goes as the whole file, drawn above line 1.
    vim.fn.writefile({ 'n1', 'n2', 'a1', 'a2', 'a4', 'a5' }, added)
    vim.cmd('checktime')
    wait_drawn(review.buf, { '1^:✎ third' })
  end)

  it('draws again when the core says a note changed', function()
    local repo = notes_repo(false)
    local review = open_review(repo)
    add_by_hand(repo, { 'added.txt', '--line', '1' }, 'from elsewhere')
    -- The core's event reads the live fleet again; this Neovim is not registered, so read it here.
    require('orca-term.live').read()
    wait_drawn(review.buf, { '1:✎', '1:✎ from elsewhere' })
  end)

  it('gN sends every note to the agent and consumes them', function()
    local repo = notes_repo(true)
    add_by_hand(repo, { 'added.txt', '--line', '3' }, 'first')
    add_by_hand(repo, { 'tracked.txt' }, 'say "hi"')
    local review = open_review(repo)
    wait_drawn(review.buf, { '3:✎', '3:✎ first' })
    local ui = support.stub_ui({ select = { 'Send all (2)' } })

    press(review.main, 'gN')
    support.wait_for(function()
      return #ui.notes > 0
    end, 'the send to be told')

    assert.are.same({ 'Notes for fix' }, ui.prompts)
    assert.are.same({ 'Send all (2)', 'added.txt:3  first', 'tracked.txt  say "hi"' }, ui.shown[1])
    assert.are.same({
      { message = 'orca-term: sent 2 notes to fix', level = vim.log.levels.INFO },
    }, ui.notes)
    -- Orca's format, typed as one bracketed paste and then Enter.
    local typed = table.concat(claude_read(repo, 7), '\n')
    local prompt = table.concat({
      'File: added.txt',
      'Line: 3',
      'User comment: "first"',
      '',
      'File: tracked.txt',
      'Scope: file',
      'User comment: "say \\"hi\\""',
    }, '\n')
    assert(typed:find(prompt, 1, true), typed)
    assert.are.same({}, listed_notes(repo))
    wait_drawn(review.buf, {})
    wait_drawn(review.view.panel.bufid, {})
    vim.api.nvim_set_current_tabpage((assert(tabs.find_all(repo.path)[1])))
    support.wait_for(function()
      return not sidebar_row('fix'):find('✎', 1, true)
    end, "the sidebar's ✎ to go")
  end)

  it('sends one note alone, edits one and deletes one', function()
    local repo = notes_repo(true)
    add_by_hand(repo, { 'added.txt', '--line', '1' }, 'send me')
    add_by_hand(repo, { 'added.txt', '--lines', '2-3' }, 'edit me')
    add_by_hand(repo, { 'added.txt', '--line', '5' }, 'delete me')
    local review = open_review(repo)
    wait_drawn(review.buf, {
      '1:✎',
      '1:✎ send me',
      '2:✎',
      '3:┆',
      '3:✎ edit me',
      '5:✎',
      '5:✎ delete me',
    })

    local ui = support.stub_ui({ select = { 'added.txt:1  send me', 'Send' } })
    press(review.main, 'gN')
    support.wait_for(function()
      return #ui.notes > 0
    end, 'the send to be told')
    assert.are.same(notes.NOTE_MENU, ui.shown[2])
    assert.are.same({ 'Notes for fix', 'added.txt:1  send me' }, ui.prompts)
    assert.are.same({
      { message = 'orca-term: sent 1 note to fix', level = vim.log.levels.INFO },
    }, ui.notes)
    local typed = table.concat(claude_read(repo, 3), '\n')
    assert(typed:find('File: added.txt\nLine: 1\nUser comment: "send me"', 1, true), typed)
    assert.are.same(
      { 'edit me', 'delete me' },
      vim.tbl_map(function(note)
        return note.body
      end, listed_notes(repo))
    )

    ui = support.stub_ui({ input = 'edited', select = { 'added.txt:2-3  edit me', 'Edit' } })
    press(review.main, 'gN')
    wait_drawn(review.buf, { '2:✎', '3:┆', '3:✎ edited', '5:✎', '5:✎ delete me' })
    assert.are.same({ 'edit me' }, ui.defaults)

    ui = support.stub_ui({ select = { 'added.txt:5  delete me', 'Delete' } })
    press(review.main, 'gN')
    wait_drawn(review.buf, { '2:✎', '3:┆', '3:✎ edited' })
    assert.are.same(
      { 'edited' },
      vim.tbl_map(function(note)
        return note.body
      end, listed_notes(repo))
    )
    assert.are.same({}, ui.notes)
  end)

  it(
    ':OrcaTerm notes lists them from the worktree, and says how to start a stopped agent',
    function()
      local repo = notes_repo(false)
      local ui = support.stub_ui({})

      vim.cmd('OrcaTerm notes')
      assert.are.same({
        {
          message = "orca-term: the notes need a worktree's tab or its review; go to one first",
          level = vim.log.levels.ERROR,
        },
      }, ui.notes)

      vim.cmd('OrcaTerm worktree 1')
      ui = support.stub_ui({})
      vim.cmd('OrcaTerm notes')
      support.wait_for(function()
        return #ui.notes > 0
      end, 'no pending notes to be told')
      assert.are.same({
        { message = 'orca-term: no pending notes in fix', level = vim.log.levels.INFO },
      }, ui.notes)

      add_by_hand(repo, { 'added.txt', '--line', '2' }, 'waiting')
      ui = support.stub_ui({ select = { 'Send all (1)' } })
      vim.cmd('normal ' .. vim.keycode('<Plug>(OrcaTermNotes)'))
      support.wait_for(function()
        return #ui.notes > 0
      end, 'the refusal')
      assert.are.same({
        {
          message = 'orca-term: fix has no running agent to send notes to; press <CR> on it in the '
            .. 'sidebar to start it, then send again (the notes stay pending)',
          level = vim.log.levels.ERROR,
        },
      }, ui.notes)
      assert.are.equal(1, #listed_notes(repo))
    end
  )

  it("leaves nothing on the file's buffer outside the review, and puts it back there", function()
    local repo = notes_repo(false)
    add_by_hand(repo, { 'added.txt', '--line', '2' }, 'only in the review')
    local review = open_review(repo)
    wait_drawn(review.buf, { '2:✎', '2:✎ only in the review' })

    -- gf edits the same buffer in the worktree's tab.
    vim.api.nvim_win_set_cursor(review.main, { 2, 0 })
    press(review.main, 'gf')
    assert.are.equal(tabs.find_all(repo.path)[1], vim.api.nvim_get_current_tabpage())
    assert.are.equal(review.buf, vim.api.nvim_get_current_buf())
    vim.cmd('normal! ix')
    for _, key in ipairs({ { 'n', 'gn' }, { 'x', 'gn' }, { 'n', 'gN' } }) do
      assert(not has_our_map(review.buf, key[1], key[2]), key[1] .. ' ' .. key[2])
    end
    assert.are.same({}, drawn(review.buf))
    vim.cmd('silent! edit!')

    -- Back in the review, the notes and their keys are there again.
    vim.api.nvim_set_current_tabpage(review.tab)
    wait_drawn(review.buf, { '2:✎', '2:✎ only in the review' })
    assert.is_true(has_our_map(review.buf, 'n', 'gn'))
  end)
end)
