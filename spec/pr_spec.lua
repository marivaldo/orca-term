-- A worktree's pull request and its checks (#87). `render.lua` draws `PR` after a row's `✎N`, in
-- the colour of the pull request's state and, while it is open, of its checks, and its line in the
-- detail float; `pr.lua` decides when the client asks the core to refresh them (`pr refresh`).
-- Seam 2: the sidebar against the real binary, whose `pr refresh` runs the fake `gh` every fixture
-- puts first on PATH (`support.gh_answer`), never the real GitHub CLI.

local support = require('support')
local detail = require('orca-term.detail')
local pr = require('orca-term.pr')
local render = require('orca-term.render')
local sidebar = require('orca-term.sidebar')

---A worktree on a branch of its own name, with the pull request `found` (`nil` for none known).
---@param name string
---@param found? orca_term.Pr
---@param fields? table
---@return orca_term.Worktree
local function worktree(name, found, fields)
  local item = {
    name = name,
    path = '/src/wt/' .. name,
    branch = name,
    state = 'no_agent',
    notes = 0,
    denials = 0,
    pr = found,
  }
  return vim.tbl_extend('force', item, fields or {})
end

---A fleet of `worktrees`.
---@param worktrees orca_term.Worktree[]
---@return orca_term.Fleet
local function fleet(worktrees)
  return {
    contract = 1,
    version = '0.0.0',
    primary = { path = '/src/orca-term', branch = 'main' },
    worktrees = worktrees,
  }
end

---Pull request 12 in `state` with `checks`, found at `fetched_at`.
---@param state string
---@param checks string
---@param fetched_at? integer
---@return orca_term.Pr
local function pull(state, checks, fetched_at)
  return {
    fetched_at = fetched_at or 0,
    number = 12,
    title = 'Fix the login',
    state = state,
    url = 'https://github.com/o/r/pull/12',
    checks = checks,
  }
end

---The group of the span covering `text` on line `line` of `rendered`, or `nil`.
---@param rendered orca_term.Rendered
---@param line integer
---@param text string
---@return string?
local function group_of(rendered, line, text)
  for _, span in ipairs(rendered.spans) do
    if span.line == line and rendered.lines[line]:sub(span.first + 1, span.last) == text then
      return span.group
    end
  end
  return nil
end

describe('render, a pull request', function()
  it('draws PR after the notes, in the colour of its state and, while open, its checks', function()
    local cases = {
      { pull('open', 'passing'), 'OrcaTermPrPassing' },
      { pull('open', 'failing'), 'OrcaTermPrFailing' },
      { pull('open', 'pending'), 'OrcaTermPrPending' },
      { pull('open', 'none'), 'OrcaTermPrOpen' },
      { pull('merged', 'passing'), 'OrcaTermPrMerged' },
      { pull('closed', 'failing'), 'OrcaTermPrClosed' },
      { pull('draft', 'pending'), 'OrcaTermPrDraft' },
    }
    for _, case in ipairs(cases) do
      local rendered =
        render.fleet(fleet({ worktree('aaa'), worktree('fix', case[1]) }), '/src/wt/aaa')
      local line = assert(rendered.firsts['/src/wt/fix'])
      assert(vim.endswith(rendered.lines[line], ' PR'), rendered.lines[line])
      assert.are.equal(case[2], group_of(rendered, line, 'PR'), vim.inspect(case[1]))
    end

    local busy = worktree('fix', pull('open', 'passing'), { denials = 1, notes = 2 })
    local rendered = render.fleet(fleet({ worktree('aaa'), busy }), '/src/wt/aaa')
    local line = rendered.lines[rendered.firsts['/src/wt/fix']]
    assert(vim.endswith(line, '⊘1 ✎2 PR'), line)
    assert(vim.fn.strdisplaywidth(line) <= render.WIDTH - 1, line)
  end)

  it(
    'draws nothing for no pull request, one gh could not tell, or one never asked about',
    function()
      for _, found in ipairs({
        { fetched_at = 1, none = true },
        { fetched_at = 1, unavailable = 'GitHub CLI is unavailable' },
        vim.NIL,
      }) do
        local rendered = render.fleet(
          fleet({ worktree('aaa'), worktree('fix', found ~= vim.NIL and found or nil) }),
          '/src/wt/aaa'
        )
        assert.are.equal(' · fix', rendered.lines[rendered.firsts['/src/wt/fix']])
      end
    end
  )

  it('shows its state, checks and age in the detail float, then its address', function()
    local now = 1700000000000
    local document = {
      contract = 1,
      version = '0.0.0',
      worktree = worktree('fix', pull('open', 'passing', now - 2 * 60 * 1000)),
      denials = {},
    }
    local rendered = render.detail(document, 50, now)
    assert.are.same({
      ' · no agent',
      ' fix',
      '',
      ' PR #12 open, checks passing (2 min ago)',
      ' https://github.com/o/r/pull/12',
    }, rendered.lines)
    assert.are.equal(
      'OrcaTermPrPassing',
      group_of(rendered, 4, 'PR #12 open, checks passing (2 min ago)')
    )
    assert.are.equal('OrcaTermMuted', group_of(rendered, 5, 'https://github.com/o/r/pull/12'))

    document.worktree.pr = pull('merged', 'none', now)
    assert.are.equal(
      ' PR #12 merged, no checks (just now)',
      render.detail(document, 50, now).lines[4]
    )
    document.worktree.pr = { fetched_at = now, none = true }
    assert.are.same({ ' · no agent', ' fix' }, render.detail(document, 50, now).lines)
  end)

  it('says how long ago in minutes, hours or days', function()
    local now = 100 * 24 * 3600 * 1000
    assert.are.equal('just now', render.ago(now - 59 * 1000, now))
    assert.are.equal('1 min ago', render.ago(now - 60 * 1000, now))
    assert.are.equal('59 min ago', render.ago(now - 59 * 60 * 1000, now))
    assert.are.equal('3 h ago', render.ago(now - 3 * 3600 * 1000, now))
    assert.are.equal('2 d ago', render.ago(now - 49 * 3600 * 1000, now))
  end)
end)

describe('pr, when the client asks', function()
  after_each(pr._reset)

  it('is due once a minute has passed since the core found it', function()
    local now = 10 * 60 * 1000
    assert.is_true(pr.due(worktree('never'), now))
    assert.is_true(pr.due(worktree('old', pull('open', 'none', now - pr.MIN_AGE_MS)), now))
    assert.is_false(pr.due(worktree('new', pull('open', 'none', now - 1000)), now))
  end)

  it('tells a pending pull request and a turn that ended', function()
    assert.is_true(pr.pending(worktree('a', pull('open', 'pending'))))
    assert.is_false(pr.pending(worktree('b', pull('open', 'passing'))))
    assert.is_false(pr.pending(worktree('c')))
    assert.is_true(pr.turn_ended('working', 'finished'))
    assert.is_true(pr.turn_ended('waiting', 'interrupted'))
    assert.is_true(pr.turn_ended('idle', 'finished'))
    assert.is_true(pr.turn_ended('working', 'idle'))
    assert.is_false(pr.turn_ended('working', 'waiting'))
    assert.is_false(pr.turn_ended('idle', 'working'))
    assert.is_false(pr.turn_ended('finished', 'finished'))
    assert.is_false(pr.turn_ended(nil, 'finished'))
  end)
end)

---What `gh pr view --json ...` prints for pull request 12 in `state` with the checks `rollup`.
---@param state string
---@param rollup table[]
---@return table
local function view(state, rollup)
  return {
    number = 12,
    title = 'Fix the login',
    state = state,
    isDraft = false,
    url = 'https://github.com/o/r/pull/12',
    statusCheckRollup = rollup,
  }
end

local RUNNING = { { __typename = 'CheckRun', status = 'IN_PROGRESS', conclusion = '' } }
local PASSED = { { __typename = 'CheckRun', status = 'COMPLETED', conclusion = 'SUCCESS' } }

---The sidebar's line of the worktree `name`, its number and the group `PR` is drawn in there
---(`nil` when there is no `PR`).
---@param name string
---@return string? line
---@return integer? row 1-based
---@return string? group
local function row_of(name)
  local buf = assert(sidebar.buffer())
  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  for index, line in ipairs(lines) do
    if line:find(' ' .. name .. ' ', 1, true) or vim.endswith(line, ' ' .. name) then
      local marks = vim.api.nvim_buf_get_extmarks(
        buf,
        sidebar.NAMESPACE,
        { index - 1, 0 },
        { index - 1, -1 },
        { details = true }
      )
      for _, mark in ipairs(marks) do
        local details = mark[4] or {}
        if line:sub(mark[3] + 1, details.end_col) == 'PR' then
          return line, index, details.hl_group
        end
      end
      return line, index, nil
    end
  end
  return nil
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

---Waits until the sidebar draws `PR` on the row of `name` in `group`.
---@param name string
---@param group string
local function wait_for_pr(name, group)
  support.wait_for(function()
    local _, _, drawn = row_of(name)
    return drawn == group
  end, 'PR in ' .. group .. ' on ' .. name)
end

describe('the pull requests in the sidebar', function()
  after_each(function()
    pr._reset()
    pr.MIN_AGE_MS = 60 * 1000
    pr.POLL_MS = 60 * 1000
    support.reset_editor()
    support.cleanup()
  end)

  it('refreshes on opening, at most once a minute, and shows PR in its checks colour', function()
    support.load_client()
    local repo = support.repo()
    support.run({ support.binary, 'worktree', 'new', 'demo' }, repo.root)
    support.gh_answer(repo, 'demo', view('OPEN', RUNNING))
    support.cd(repo.root)

    vim.cmd('OrcaTerm sidebar')
    wait_for_pr('demo', 'OrcaTermPrPending')
    assert.are.equal(1, #support.gh_calls(repo))
    assert.are.equal(
      'pr view demo --json number,title,state,isDraft,url,statusCheckRollup',
      support.gh_calls(repo)[1]
    )

    -- Closed and opened again within the minute: nothing is asked.
    vim.cmd('OrcaTerm sidebar')
    vim.cmd('OrcaTerm sidebar')
    vim.wait(300)
    assert.are.equal(1, #support.gh_calls(repo))
  end)

  it('R refreshes the worktree under the cursor, and K shows its line and address', function()
    support.load_client()
    local repo = support.repo()
    support.run({ support.binary, 'worktree', 'new', 'demo' }, repo.root)
    support.gh_answer(repo, 'demo', view('OPEN', RUNNING))
    support.cd(repo.root)
    vim.cmd('OrcaTerm sidebar')
    wait_for_pr('demo', 'OrcaTermPrPending')

    support.gh_answer(repo, 'demo', view('MERGED', PASSED))
    local win = sidebar_window()
    vim.api.nvim_set_current_win(win)
    local _, row = row_of('demo')
    vim.api.nvim_win_set_cursor(win, { assert(row), 0 })
    vim.cmd('normal R')
    wait_for_pr('demo', 'OrcaTermPrMerged')
    assert.are.equal(2, #support.gh_calls(repo))

    vim.cmd('normal K')
    support.wait_for(function()
      return detail.window() ~= nil
    end, 'the detail float')
    local float = assert(detail.window())
    local lines = vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(float), 0, -1, false)
    assert.is_true(vim.tbl_contains(lines, ' PR #12 merged, checks passing (just now)'))
    assert.is_true(vim.tbl_contains(lines, ' https://github.com/o/r/pull/12'))
  end)

  it(':OrcaTerm pr-refresh refreshes every worktree and says how many', function()
    support.load_client()
    local repo = support.repo()
    for _, name in ipairs({ 'demo', 'docs' }) do
      support.run({ support.binary, 'worktree', 'new', name }, repo.root)
    end
    support.cd(repo.root)
    local ui = support.stub_ui({})

    vim.cmd('OrcaTerm pr-refresh')

    support.wait_for(function()
      return #ui.notes > 0
    end, 'the notification')
    assert.are.same({
      message = 'orca-term: refreshed the pull request of 2 worktrees',
      level = vim.log.levels.INFO,
    }, ui.notes[1])
    assert.are.equal(2, #support.gh_calls(repo))
    assert.is_true(support.listed(repo, 'docs').pr.none)
  end)

  it('refreshes pending checks on a timer while the sidebar is open, and stops after', function()
    support.load_client()
    pr.POLL_MS = 200
    local repo = support.repo()
    support.run({ support.binary, 'worktree', 'new', 'demo' }, repo.root)
    support.gh_answer(repo, 'demo', view('OPEN', RUNNING))
    support.cd(repo.root)
    vim.cmd('OrcaTerm sidebar')
    wait_for_pr('demo', 'OrcaTermPrPending')
    assert.is_true(pr.polling())

    support.gh_answer(repo, 'demo', view('OPEN', PASSED))
    wait_for_pr('demo', 'OrcaTermPrPassing')
    support.wait_for(function()
      return not pr.polling()
    end, 'the timer to stop once nothing is pending')

    -- Pending again, then the sidebar closes: the timer stops with it.
    support.gh_answer(repo, 'demo', view('OPEN', RUNNING))
    local win = sidebar_window()
    vim.api.nvim_set_current_win(win)
    local _, row = row_of('demo')
    vim.api.nvim_win_set_cursor(win, { assert(row), 0 })
    vim.cmd('normal R')
    wait_for_pr('demo', 'OrcaTermPrPending')
    assert.is_true(pr.polling())
    vim.cmd('OrcaTerm sidebar')
    assert.is_false(pr.polling())
  end)

  it("refreshes a worktree's pull request when one of its turns ends", function()
    support.load_client()
    pr.MIN_AGE_MS = 60 * 60 * 1000
    local repo = support.agent_repo()
    support.run({ support.binary, 'worktree', 'new', 'fix' }, repo.root)
    support.run({ support.binary, 'agent', 'start', 'fix' }, repo.root)
    support.cd(repo.root)
    vim.cmd('OrcaTerm sidebar')
    support.wait_for(function()
      return #support.gh_calls(repo) == 1
    end, 'the refresh on opening')

    support.play_hooks(repo, support.worktree_path(repo, 'fix'), 'p-hello-2.1.300.log')
    -- This Neovim is not registered with the core: the sidebar reads the fleet when asked.
    sidebar.refresh()

    support.wait_for(function()
      return #support.gh_calls(repo) == 2
    end, 'the refresh after the turn ended')
  end)
end)
