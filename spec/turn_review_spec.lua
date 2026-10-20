-- Seam 2: reviewing one turn in diffview.nvim, in headless Neovim against the real binary.
-- `:OrcaTerm turns` (and `<Plug>(OrcaTermTurns)`) in a worktree's tab lists the worktree's turns
-- from the core's `turn ls`, newest first, and opens the one picked in diffview on
-- `<begin>..<end>`, its two snapshots, in a turn review of its own: a review tab where the sidebar
-- never shows, where `gf` edits the file in the worktree's tab and `gn` adds no note. The turns are
-- made as claude makes them, by running the real `orca-term hook claude` with claude's payloads
-- on stdin, as tests/support/turns.rs does.
--
-- diffview.nvim comes from .rocks/diffview.nvim, as in spec/review_spec.lua: every spec closes every
-- view and review tab it opened, and deletes the autocommand groups of the review and the notes.

local support = require('support')
local review = require('orca-term.review')
local sidebar = require('orca-term.sidebar')
local tabs = require('orca-term.tabs')

---Where `just lua-rocks` puts diffview.nvim.
local DIFFVIEW = support.root .. '/.rocks/diffview.nvim'

---The session of the payloads written here.
local SESSION = 's-1'

---A turn from before orca-term took snapshots: its start and end lines, with no snapshot ids, as
---tests/turns.rs writes it.
local OLD_TURN = {
  '{"schema":1,"kind":"start","turn":1,"session":"s-0","prompt_id":"p0","at":10,'
    .. '"prompt":"an old prompt"}',
  '{"schema":1,"kind":"end","turn":1,"session":"s-0","prompt_id":"p0","at":20,'
    .. '"outcome":"finished","detail":"ok"}',
}

---diffview's global state, `DiffviewGlobal`, which holds its logger.
---@return table
local function diffview_global()
  -- selene: allow(global_usage)
  return rawget(_G, 'DiffviewGlobal')
end

---The fixture directory of the spec's repository, whose files `after_each` wipes from the buffers.
---@type string?
local fixture_dir = nil

---@class spec.TurnsRepo: spec.Repo
---@field path string the worktree `fix`
---@field admin string its admin directory, where the hooks keep its turns
---@field turns table<integer, { begin: string, ["end"]: string }> the snapshots of turns 2 and 3

---Runs the hook of `payload`'s event in the worktree, as claude does: the real
---`orca-term hook claude --admin <admin> <event>`, the payload on stdin.
---@param repo spec.TurnsRepo
---@param payload table
local function fire(repo, payload)
  local command = {
    support.binary,
    'hook',
    'claude',
    '--admin',
    repo.admin,
    payload.hook_event_name,
  }
  local result =
    vim.system(command, { stdin = vim.json.encode(payload), cwd = repo.path, text = true }):wait()
  assert(result.code == 0, payload.hook_event_name .. ' exited ' .. result.code)
end

---claude's `UserPromptSubmit` of the prompt `prompt_id`, saying `do <prompt_id>`.
---@param prompt_id string
---@return table
local function submit(prompt_id)
  return {
    hook_event_name = 'UserPromptSubmit',
    session_id = SESSION,
    prompt_id = prompt_id,
    prompt = 'do ' .. prompt_id,
  }
end

---claude's `Stop` of the turn of `prompt_id`.
---@param prompt_id string
---@return table
local function stop(prompt_id)
  return {
    hook_event_name = 'Stop',
    session_id = SESSION,
    prompt_id = prompt_id,
    last_assistant_message = 'done',
  }
end

---Appends `line` to the worktree's `file.txt`.
---@param repo spec.TurnsRepo
---@param line string
local function append(repo, line)
  local file = repo.path .. '/file.txt'
  vim.fn.writefile(vim.list_extend(vim.fn.readfile(file), { line }), file)
end

---The turn log's lines, decoded.
---@param repo spec.TurnsRepo
---@return table[]
local function turn_log(repo)
  return vim.tbl_map(vim.json.decode, vim.fn.readfile(repo.admin .. '/turns.jsonl'))
end

---A repository whose worktree `fix` (made by the core) has four turns, as tests/turns.rs builds
---them: turn 1 from before the snapshots, turn 2 finished (it adds `turn two` to `file.txt` and
---`new.txt`), turn 3 interrupted by the prompt of turn 4 (it adds `turn three`), turn 4 open. A
---second worktree, `quiet`, has no turns. Neovim is in the primary checkout, sidebar open.
---@return spec.TurnsRepo
local function turns_repo()
  support.load_client()
  local repo = support.repo() --[[@as spec.TurnsRepo]]
  fixture_dir = repo.tmp
  vim.fn.writefile({ 'base' }, repo.root .. '/file.txt')
  support.git(repo, { 'add', '.' })
  support.git(repo, { 'commit', '--quiet', '-m', 'base' })
  support.run({ support.binary, 'worktree', 'new', 'fix' }, repo.root)
  support.run({ support.binary, 'worktree', 'new', 'quiet' }, repo.root)
  repo.path = support.worktree_path(repo, 'fix')
  repo.admin = vim.trim(support.run({
    'git',
    '-C',
    repo.path,
    'rev-parse',
    '--path-format=absolute',
    '--git-path',
    'orca-term',
  }))

  vim.fn.writefile(OLD_TURN, repo.admin .. '/turns.jsonl')
  fire(repo, submit('p1'))
  append(repo, 'turn two')
  vim.fn.writefile({ 'new' }, repo.path .. '/new.txt')
  fire(repo, stop('p1'))
  fire(repo, submit('p2'))
  append(repo, 'turn three')
  fire(repo, submit('p3'))

  repo.turns = {}
  for _, line in ipairs(turn_log(repo)) do
    if line.kind == 'end' and line.begin then
      repo.turns[line.turn] = { begin = line.begin, ['end'] = line['end'] }
    end
  end
  assert(repo.turns[2] and repo.turns[3], 'turns 2 and 3 have their snapshots')

  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  return repo
end

---The items the list of turns of `fix` shows, newest first.
local ITEMS = {
  '4  working  do p3',
  '3  interrupted  do p2',
  '2  finished  do p1',
  '1  finished  an old prompt',
}

---The sidebar's window in `tab` (the current tab when `nil`), or `nil` when it is not shown there.
---@param tab? integer
---@return integer?
local function sidebar_window(tab)
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(tab or 0)) do
    if vim.api.nvim_win_get_buf(win) == sidebar.buffer() then
      return win
    end
  end
  return nil
end

---diffview's view in the current tab, or `nil`.
---@return table?
local function current_view()
  return require('diffview.lib').get_current_view()
end

---The paths of the files the view lists, sorted.
---@param view table
---@return string[]
local function listed_files(view)
  local paths = {}
  for _, file in view.files:iter() do
    table.insert(paths, file.path)
  end
  table.sort(paths)
  return paths
end

---Every review tab: tabs holding a diffview view or marked by orca-term.
---@return integer[]
local function review_tabs()
  return vim.tbl_filter(review.is_review, vim.api.nvim_list_tabpages())
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

---Goes to the worktree's tab, runs `:OrcaTerm turns` there and picks `item`.
---@param repo spec.TurnsRepo
---@param item string
---@return spec.Ui
local function pick_turn(repo, item)
  tabs.go_to(repo.path)
  local ui = support.stub_ui({ select = { item } })
  vim.cmd('OrcaTerm turns')
  return ui
end

---Waits until the current tab is a turn review whose view lists `count` files, its right-hand
---window filled, with orca-term's `gf` and `gn` on it, and returns the view.
---@param count integer
---@return table view
local function wait_for_turn_review(count)
  support.wait_for(function()
    local view = current_view()
    if not (view and view.cur_layout and view.cur_entry and #listed_files(view) == count) then
      return false
    end
    local buf = vim.api.nvim_win_get_buf(view.cur_layout:get_main_win().id)
    return has_our_map(buf, 'n', 'gf') and has_our_map(buf, 'n', 'gn')
  end, 'the turn review to open and list its files')
  return assert(current_view())
end

---Has diffview read the view's files again and waits until it has, so that no git process of its
---own still runs in the worktree when a spec removes it. diffview reads them as a view opens, a
---moment after it lists them; its reads are debounced, so this read runs after any pending one.
---@param view table
local function settle(view)
  local done = false
  view:update_files(function()
    done = true
  end)
  support.wait_for(function()
    return done
  end, 'diffview to read the files again')
end

---The files `git diff --name-only <begin> <end>` names, sorted.
---@param repo spec.TurnsRepo
---@param bounds { begin: string, ["end"]: string }
---@return string[]
local function changed_between(repo, bounds)
  local names = vim.split(
    vim.trim(support.git(repo, { 'diff', '--name-only', bounds.begin, bounds['end'] })),
    '\n'
  )
  table.sort(names)
  return names
end

describe('a turn review', function()
  -- These leave 'columns' as it is, as spec/review_spec.lua does: a redraw after it changed can
  -- crash this Neovim, which has no UI.
  setup(function()
    vim.opt.runtimepath:append(DIFFVIEW)
    require('diffview').setup({ use_icons = false })
    -- diffview logs to a file under stdpath('cache'): Neovim's own temporary directory outlives
    -- every fixture.
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
    pcall(vim.api.nvim_del_augroup_by_name, 'orca-term-review')
    require('orca-term.notes')._reset()
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

  it(
    "lists the turns newest first and opens one on its begin and end, outside the worktree's tab",
    function()
      local repo = turns_repo()
      local ui = pick_turn(repo, '2  finished  do p1')
      local view = wait_for_turn_review(2)

      assert.are.same({ 'Turns of fix' }, ui.prompts)
      assert.are.same({ ITEMS }, ui.shown)
      assert.are.same({
        { message = 'orca-term: turn 2 of fix', level = vim.log.levels.INFO },
      }, ui.notes)

      -- Exactly `git diff <begin> <end>` of turn 2.
      local bounds = repo.turns[2]
      assert.are.equal(repo.path, view.adapter.ctx.toplevel)
      assert.are.equal(bounds.begin, view.left.commit)
      assert.are.equal(bounds['end'], view.right.commit)
      assert.are.same({ 'file.txt', 'new.txt' }, listed_files(view))
      assert.are.same(changed_between(repo, bounds), listed_files(view))

      local turn_tab = vim.api.nvim_get_current_tabpage()
      assert.are.same({ path = repo.path, number = 2 }, review.turn_of(turn_tab))
      assert(review.worktree_of(turn_tab) == nil, "a turn review is not the worktree's review")
      assert.is_nil(review.find(repo.path))
      assert(sidebar_window(turn_tab) == nil, 'no sidebar in the turn review')
      local worktree_tab = assert(tabs.find_all(repo.path)[1])
      assert.are_not.equal(turn_tab, worktree_tab)
      assert.are.same({ worktree_tab }, tabs.find_all(repo.path))
      assert(sidebar_window(worktree_tab), "the sidebar shows in the worktree's tab")
    end
  )

  it('of an interrupted turn says its end was not observed', function()
    local repo = turns_repo()
    local ui = pick_turn(repo, '3  interrupted  do p2')
    local view = wait_for_turn_review(1)

    assert.are.equal(repo.turns[3].begin, view.left.commit)
    assert.are.equal(repo.turns[3]['end'], view.right.commit)
    assert.are.same({ 'file.txt' }, listed_files(view))
    assert.are.same({
      {
        message = 'orca-term: turn 3 of fix: its end was not observed (it was interrupted), so '
          .. 'this diff may include edits made after the interruption',
        level = vim.log.levels.WARN,
      },
    }, ui.notes)
  end)

  it('says why a turn cannot be reviewed, or that there are none, and opens nothing', function()
    local repo = turns_repo()
    local count = #vim.api.nvim_list_tabpages()
    local ui = support.stub_ui({})
    vim.cmd('OrcaTerm turns')
    assert.are.same({
      {
        message = "orca-term: the turns need a worktree's tab or its review; go to one first",
        level = vim.log.levels.ERROR,
      },
    }, ui.notes)

    for _, case in ipairs({
      { ITEMS[1], 'orca-term: turn 4 has not ended yet' },
      { ITEMS[4], 'orca-term: turn 1 has no snapshots' },
    }) do
      ui = pick_turn(repo, case[1])
      support.wait_for(function()
        return #ui.notes > 0
      end, 'the reason for ' .. case[1])
      assert.are.same({ { message = case[2], level = vim.log.levels.WARN } }, ui.notes)
    end
    assert.are.same({}, review_tabs())

    tabs.go_to(support.worktree_path(repo, 'quiet'))
    ui = support.stub_ui({})
    vim.cmd('normal ' .. vim.keycode('<Plug>(OrcaTermTurns)'))
    support.wait_for(function()
      return #ui.notes > 0
    end, 'no turns to be said')
    assert.are.same({
      { message = 'orca-term: no turns in worktree quiet', level = vim.log.levels.INFO },
    }, ui.notes)
    assert.are.same({}, ui.shown)
    -- Only the worktrees' tabs were made: the first tab, `fix`'s and `quiet`'s.
    assert.are.equal(count + 2, #vim.api.nvim_list_tabpages())
    assert.are.same({}, review_tabs())
  end)

  it('goes back to a turn review already open, from its own tab too', function()
    local repo = turns_repo()
    pick_turn(repo, '2  finished  do p1')
    wait_for_turn_review(2)
    local turn_tab = vim.api.nvim_get_current_tabpage()
    local count = #vim.api.nvim_list_tabpages()

    local ui = pick_turn(repo, '2  finished  do p1')
    support.wait_for(function()
      return vim.api.nvim_get_current_tabpage() == turn_tab
    end, 'the turn review to become current again')
    assert.are.equal(count, #vim.api.nvim_list_tabpages())
    assert.are.same(
      { { message = 'orca-term: turn 2 of fix', level = vim.log.levels.INFO } },
      ui.notes
    )

    -- From the turn review itself, the list is the same worktree's.
    ui = support.stub_ui({ select = { '3  interrupted  do p2' } })
    vim.cmd('OrcaTerm turns')
    wait_for_turn_review(1)
    assert.are.same({ ITEMS }, ui.shown)
    assert.are.same({ path = repo.path, number = 3 }, review.turn_of(0))
    assert.are.equal(count + 1, #vim.api.nvim_list_tabpages())
  end)

  it("gf edits the file in the worktree's tab", function()
    local repo = turns_repo()
    pick_turn(repo, '2  finished  do p1')
    local view = wait_for_turn_review(2)
    local panel = view.panel
    vim.api.nvim_set_current_win(panel.winid)
    local found = false
    for index, line in ipairs(vim.api.nvim_buf_get_lines(panel.bufid, 0, -1, false)) do
      if line:find('new.txt', 1, true) then
        vim.api.nvim_win_set_cursor(panel.winid, { index, 0 })
        found = true
      end
    end
    assert(found, 'the panel lists new.txt')
    vim.cmd('normal gf')

    assert.are.equal(tabs.find_all(repo.path)[1], vim.api.nvim_get_current_tabpage())
    assert.are.equal(repo.path .. '/new.txt', vim.api.nvim_buf_get_name(0))
    assert.are_not.equal(sidebar.buffer(), vim.api.nvim_get_current_buf())
  end)

  it("gn adds no note and says where notes go, and d still opens the worktree's review", function()
    local repo = turns_repo()
    pick_turn(repo, '2  finished  do p1')
    local view = wait_for_turn_review(2)
    local turn_tab = vim.api.nvim_get_current_tabpage()
    local ui = support.stub_ui({ input = 'a note' })

    local main = view.cur_layout:get_main_win().id
    for _, keys in ipairs({ 'gn', 'Vgn' }) do
      vim.api.nvim_set_current_win(main)
      vim.cmd('normal ' .. keys)
    end
    vim.api.nvim_set_current_win(view.panel.winid)
    vim.cmd('normal gn')
    local said = {
      message = "orca-term: notes go on the worktree's files: add them from its review (d)",
      level = vim.log.levels.WARN,
    }
    assert.are.same({ said, said, said }, ui.notes)
    assert.are.same({}, ui.prompts)
    local listed =
      vim.json.decode(support.run({ support.binary, 'note', 'ls', 'fix', '--json' }, repo.root))
    assert.are.same({}, listed.notes)

    -- d in the sidebar goes to the worktree's review, a tab of its own, never to the turn review.
    vim.cmd('tabfirst')
    local win = assert(sidebar_window())
    vim.api.nvim_set_current_win(win)
    for index, line in ipairs(vim.api.nvim_buf_get_lines(assert(sidebar.buffer()), 0, -1, false)) do
      if line:find(' fix', 1, true) then
        vim.api.nvim_win_set_cursor(win, { index, 0 })
      end
    end
    vim.cmd('normal d')
    support.wait_for(function()
      return review.current() == repo.path and current_view() ~= nil
    end, "the worktree's review to open")
    local review_tab = vim.api.nvim_get_current_tabpage()
    assert.are_not.equal(turn_tab, review_tab)
    assert.are.equal(review_tab, review.find(repo.path))
    local base = vim.trim(support.git(repo, { 'rev-parse', 'main' }))
    assert.are.equal(base, assert(current_view()).left.commit)
    assert.are.same({ path = repo.path, number = 2 }, review.turn_of(turn_tab))
  end)

  it('closes with the worktree removed from the . menu', function()
    local repo = turns_repo()
    pick_turn(repo, '2  finished  do p1')
    settle(wait_for_turn_review(2))
    pick_turn(repo, '3  interrupted  do p2')
    settle(wait_for_turn_review(1))
    assert.are.equal(2, #review_tabs())

    vim.cmd('tabfirst')
    local win = assert(sidebar_window())
    vim.api.nvim_set_current_win(win)
    for index, line in ipairs(vim.api.nvim_buf_get_lines(assert(sidebar.buffer()), 0, -1, false)) do
      if line:find(' fix', 1, true) then
        vim.api.nvim_win_set_cursor(win, { index, 0 })
      end
    end
    -- The turns left changes in the worktree: removing it asks once more before discarding them.
    local ui = support.stub_ui({ select = { 'Remove worktree', 'Remove', 'Remove anyway' } })
    vim.cmd('normal .')
    support.wait_for(function()
      return #ui.notes > 0 and #review_tabs() == 0
    end, 'the worktree and its turn reviews to go')

    assert.are.same({}, require('diffview.lib').views)
    assert.are.equal(0, vim.fn.isdirectory(repo.path))
    assert.truthy(ui.notes[1].message:find('removed worktree fix', 1, true))
  end)
end)
