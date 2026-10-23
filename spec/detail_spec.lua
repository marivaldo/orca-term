-- Seam 2: the detail float, in headless Neovim against the real binary. `K` in the sidebar asks the
-- core for the worktree's detail (`worktree detail`) without waiting, and shows it in a float just
-- right of the sidebar, the cursor staying in the sidebar: the state and the agent, the branch and
-- its drift, the whole error, the latest turn's denials (replayed from a recording of claude's
-- hooks) and the end of the setup's log. The float closes when the sidebar's cursor moves, when
-- the person leaves the sidebar, on `K` again and on `q` in it; there is one at most, and an answer
-- that comes after the person moved away is dropped.
--
-- Headless Neovim runs no main loop between the spec's own calls, so `CursorMoved` never comes by
-- itself: the specs move the cursor, then fire it as Neovim would.

local support = require('support')
local detail = require('orca-term.detail')
local sidebar = require('orca-term.sidebar')

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

---Every floating window of the current tab.
---@return integer[]
local function floats()
  local found = {}
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_config(win).relative ~= '' then
      table.insert(found, win)
    end
  end
  return found
end

---Puts the sidebar's cursor on the name line of the worktree `name` and makes the sidebar the
---current window, as the person does before pressing `K`.
---@param name string
local function cursor_on(name)
  local win = sidebar_window()
  vim.api.nvim_set_current_win(win)
  local lines = vim.api.nvim_buf_get_lines(assert(sidebar.buffer()), 0, -1, false)
  for index, line in ipairs(lines) do
    if line:find(' ' .. name .. ' ', 1, true) or vim.endswith(line, ' ' .. name) then
      vim.api.nvim_win_set_cursor(win, { index, 0 })
      return
    end
  end
  error('no sidebar line for ' .. name)
end

---Moves the sidebar's cursor to `line`, then fires `CursorMoved` as Neovim's main loop would.
---@param line integer
local function move_cursor(line)
  local win = sidebar_window()
  vim.api.nvim_win_set_cursor(win, { line, 0 })
  vim.api.nvim_exec_autocmds('CursorMoved', { buffer = sidebar.buffer() })
end

---Presses `K` in the sidebar on the worktree `name` and waits for the float.
---@param name string
---@return integer float
---@return string[] lines
local function open_detail(name)
  cursor_on(name)
  vim.cmd('normal K')
  support.wait_for(function()
    return detail.window() ~= nil
  end, 'the detail float of ' .. name)
  local float = assert(detail.window())
  return float, vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(float), 0, -1, false)
end

---The text each highlight of the float covers, as `<group> <text>`.
---@param float integer
---@return string[]
local function highlighted(float)
  local buf = vim.api.nvim_win_get_buf(float)
  local found = {}
  local marks = vim.api.nvim_buf_get_extmarks(buf, detail.NAMESPACE, 0, -1, { details = true })
  for _, mark in ipairs(marks) do
    local row, col, details = mark[2], mark[3], mark[4] or {}
    local line = vim.api.nvim_buf_get_lines(buf, row, row + 1, false)[1]
    table.insert(found, details.hl_group .. ' ' .. line:sub(col + 1, details.end_col))
  end
  return found
end

---`lines` joined with every space taken out: what wrapping cannot change.
---@param lines string[]
---@return string
local function squeezed(lines)
  return (table.concat(lines):gsub('%s', ''))
end

---A repository whose worktree `fix` (made by the core) has a finished turn whose three Write calls
---claude's permission settings refused, played from a recording; Neovim is in the primary checkout,
---sidebar open.
---@return spec.AgentRepo
local function denied_repo()
  support.load_client()
  local repo = support.agent_repo()
  support.run({ support.binary, 'worktree', 'new', 'fix' }, repo.root)
  support.run({ support.binary, 'agent', 'start', 'fix' }, repo.root)
  support.play_hooks(repo, support.worktree_path(repo, 'fix'), 'tui-deny-multi-2.1.303.log')
  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  return repo
end

describe('the detail float', function()
  after_each(function()
    detail.close()
    support.reset_editor()
    support.cleanup()
  end)

  it(
    'shows the state, the drift and the denials next to the sidebar, the cursor staying there',
    function()
      local repo = denied_repo()
      local denied = repo.tmp .. '/denied'
      local win = sidebar_window()
      cursor_on('fix')
      local cursor = vim.api.nvim_win_get_cursor(win)

      local float, lines = open_detail('fix')
      assert.are.same({
        ' ● finished · claude-code',
        ' fix  up to date with main',
        '',
        ' Denied',
      }, vim.list_slice(lines, 1, 4))
      -- One denial per call, each after `⊘`; the long paths wrap under their text, never cut.
      local calls = vim.tbl_filter(function(line)
        return vim.startswith(line, ' ⊘ ')
      end, lines)
      assert.are.equal(3, #calls)
      assert.is_nil(table.concat(lines):find('…', 1, true))
      local expected = ''
      for _, file in ipairs({ 'a', 'b', 'c' }) do
        expected = expected .. '⊘Write:' .. denied .. '/' .. file .. '.txt'
      end
      assert.are.equal(expected, squeezed(vim.list_slice(lines, 5)))
      local marks = highlighted(float)
      assert.are.equal(3, #vim.tbl_filter(function(mark)
        return mark == 'OrcaTermDenied ⊘'
      end, marks))
      assert.is_true(vim.tbl_contains(marks, 'OrcaTermStateFinished finished'))

      -- A hover: the cursor stays in the sidebar, where it was, and the float sits at its right.
      assert.are.equal(win, vim.api.nvim_get_current_win())
      assert.are.same(cursor, vim.api.nvim_win_get_cursor(win))
      local config = vim.api.nvim_win_get_config(float)
      assert.are.equal('win', config.relative)
      assert.are.equal(win, config.win)
      assert.are.equal(vim.api.nvim_win_get_width(win), config.col)
      assert.are.equal(cursor[1] - 1, config.row)
      assert.are.equal(' fix ', config.title[1][1])
      assert.is_true(config.width >= detail.MIN_WIDTH and config.width <= detail.MAX_WIDTH)
      -- As high as what it shows, up to half the editor.
      local half = math.floor((vim.o.lines - vim.o.cmdheight) / 2)
      assert.are.equal(math.min(#lines, half), config.height)
      assert.are.equal('wipe', vim.bo[vim.api.nvim_win_get_buf(float)].bufhidden)

      -- The sidebar's row counts the denials.
      local row =
        vim.api.nvim_buf_get_lines(assert(sidebar.buffer()), cursor[1] - 1, cursor[1], false)[1]
      assert.is_truthy(row:find('⊘3', 1, true))
    end
  )

  it('names a conflicting rebase with its step, and the row shows ≠', function()
    support.load_client()
    local repo = support.repo()
    vim.fn.writefile({ 'base' }, repo.root .. '/a.txt')
    support.git(repo, { 'add', 'a.txt' })
    support.git(repo, { 'commit', '--quiet', '-m', 'base' })
    support.run({ support.binary, 'worktree', 'new', 'fix' }, repo.root)
    local fix = support.worktree_path(repo, 'fix')
    vim.fn.writefile({ 'fix' }, fix .. '/a.txt')
    vim.fn.writefile({ 'more' }, fix .. '/b.txt')
    support.run({ 'git', 'add', 'a.txt' }, fix)
    support.run({ 'git', 'commit', '--quiet', '-m', 'fix a' }, fix)
    support.run({ 'git', 'add', 'b.txt' }, fix)
    support.run({ 'git', 'commit', '--quiet', '-m', 'fix b' }, fix)
    vim.fn.writefile({ 'main' }, repo.root .. '/a.txt')
    support.git(repo, { 'commit', '--quiet', '--all', '-m', 'main a' })
    local rebase = vim.system({ 'git', 'rebase', 'main' }, { cwd = fix, text = true }):wait()
    assert.are_not.equal(0, rebase.code, 'the rebase stops on its conflict')
    support.cd(repo.root)
    vim.cmd('OrcaTerm sidebar')

    local float, lines = open_detail('fix')
    -- A stopped rebase leaves the worktree detached on the commits replayed so far: here none, on
    -- `main` itself, with the conflicted `a.txt` changed on disk.
    assert.are.same({
      ' · no agent',
      ' detached  up to date with main',
      ' 1 file changed',
      '',
      ' rebase in progress (1/2)',
    }, lines)
    assert.is_true(
      vim.tbl_contains(highlighted(float), 'OrcaTermOperation rebase in progress (1/2)')
    )
    local cursor = vim.api.nvim_win_get_cursor(sidebar_window())
    local row =
      vim.api.nvim_buf_get_lines(assert(sidebar.buffer()), cursor[1] - 1, cursor[1], false)[1]
    assert.is_truthy(row:find(' fix%s+≠ │$'))
  end)

  it('shows a failed setup whole: its error and the end of its log', function()
    support.load_client()
    local repo = support.repo()
    local reason = 'npm ERR! missing script: build, '
      .. 'and a message long enough to need more than one line of the float to show it whole'
    support.set_setup(repo, { 'echo "installing"', 'echo "' .. reason .. '"', 'exit 1' })
    local failed = support.core(repo, { 'worktree', 'new', 'fix-login' })
    assert.are.equal(1, failed.code, failed.stderr)
    support.cd(repo.root)
    vim.cmd('OrcaTerm sidebar')

    local float, lines = open_detail('fix-login')
    assert.are.equal(' △ not ready', lines[1])
    assert.are.equal(' fix-login  up to date with main', lines[2])
    assert.is_nil(table.concat(lines):find('…', 1, true))

    -- The error, wrapped but whole, in the not-ready colour, as the core says it.
    local document = vim.json.decode(
      support.run({ support.binary, 'worktree', 'detail', 'fix-login', '--json' }, repo.root)
    )
    local setup = assert(vim.iter(ipairs(lines)):find(function(_, line)
      return line == ' Setup (failed)'
    end))
    assert.are.equal('', lines[3])
    assert.are.equal('', lines[setup - 1])
    assert.are.equal(squeezed({ document.detail }), squeezed(vim.list_slice(lines, 4, setup - 2)))
    assert.is_truthy(document.detail:find('npm ERR!', 1, true))
    local marks = highlighted(float)
    assert.is_true(vim.tbl_contains(marks, 'OrcaTermStateNotReady ' .. vim.trim(lines[4])))

    -- Then the end of the setup's log, muted, its long line wrapped.
    assert.are.equal(' installing', lines[setup + 1])
    assert.are.equal(squeezed({ reason }), squeezed(vim.list_slice(lines, setup + 2)))
    assert.is_true(vim.tbl_contains(marks, 'OrcaTermMuted installing'))
    assert.is_true(vim.tbl_contains(marks, 'OrcaTermMuted Setup (failed)'))
  end)

  it("shows the branch's drift from the local default branch, and none without one", function()
    support.load_client()
    local repo = support.repo()
    support.run({ support.binary, 'worktree', 'new', 'fix' }, repo.root)
    local path = support.worktree_path(repo, 'fix')
    for _, message in ipairs({ 'one', 'two' }) do
      support.run({ 'git', 'commit', '--quiet', '--allow-empty', '-m', message }, path)
    end
    support.git(repo, { 'commit', '--quiet', '--allow-empty', '-m', 'on main' })
    support.cd(repo.root)
    vim.cmd('OrcaTerm sidebar')

    local _, lines = open_detail('fix')
    assert.are.equal(' fix  ↑2 ↓1 main', lines[2])
    vim.cmd('normal K')
    assert.is_nil(detail.window())

    -- With no local default branch there is no drift to show.
    support.git(repo, { 'branch', '--quiet', '-m', 'main', 'trunk' })
    _, lines = open_detail('fix')
    assert.are.equal(' fix', lines[2])
  end)

  it(
    'closes when the cursor moves, on K again and when leaving the sidebar, one at a time',
    function()
      denied_repo()
      local win = sidebar_window()

      -- K again closes it; K once more opens it again, and only ever one.
      open_detail('fix')
      assert.are.equal(1, #floats())
      vim.cmd('normal K')
      assert.is_nil(detail.window())
      assert.are.equal(0, #floats())
      open_detail('fix')
      assert.are.equal(1, #floats())

      -- CursorMoved for a cursor that did not move leaves it open; a move closes it.
      vim.api.nvim_exec_autocmds('CursorMoved', { buffer = sidebar.buffer() })
      assert.is_not_nil(detail.window())
      move_cursor(1)
      assert.is_nil(detail.window())

      -- Entering the float keeps it, and `q` there closes it.
      local float = open_detail('fix')
      vim.api.nvim_set_current_win(float)
      assert.are.equal(float, detail.window())
      vim.cmd('normal q')
      assert.is_nil(detail.window())

      -- Going to another window closes it.
      open_detail('fix')
      vim.api.nvim_set_current_win(vim.fn.win_getid(vim.fn.winnr('l')))
      assert.are_not.equal(win, vim.api.nvim_get_current_win())
      assert.is_nil(detail.window())

      -- So does closing the sidebar.
      open_detail('fix')
      vim.cmd('OrcaTerm sidebar')
      assert.is_nil(detail.window())
      assert.are.equal(0, #floats())
    end
  )

  it('drops an answer that comes after the cursor moved or the sidebar closed', function()
    denied_repo()

    -- The answer is handed over in the main loop, so nothing comes before the spec waits.
    cursor_on('fix')
    local answered = detail.answered()
    vim.cmd('normal K')
    move_cursor(1)
    support.wait_for(function()
      return detail.answered() > answered
    end, 'the answer of the core')
    assert.is_nil(detail.window())
    assert.are.equal(0, #floats())

    cursor_on('fix')
    answered = detail.answered()
    vim.cmd('normal K')
    vim.cmd('OrcaTerm sidebar')
    support.wait_for(function()
      return detail.answered() > answered
    end, 'the answer of the core')
    assert.is_nil(detail.window())
    assert.are.equal(0, #floats())
  end)
end)
