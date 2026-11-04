-- The pull request composer (#133). Seam 2: the sidebar's menu and `:OrcaTerm pr-create` open a
-- buffer prefilled by the real binary's `pr compose`, and `:w` runs its `pr create`, which pushes to
-- a bare repository standing for `origin` and runs the fake `gh` every fixture puts first on PATH
-- (`support.lua`), never the real GitHub CLI. `vim.ui` and `vim.notify` are stubbed; the core never
-- is.

local support = require('support')
local compose = require('orca-term.compose')
local sidebar = require('orca-term.sidebar')

---The sidebar buffer's lines.
---@return string[]
local function sidebar_lines()
  return vim.api.nvim_buf_get_lines(assert(sidebar.buffer()), 0, -1, false)
end

---The index of the first sidebar line holding `text`, or `nil`.
---@param text string
---@return integer?
local function line_with(text)
  for index, line in ipairs(sidebar_lines()) do
    if line:find(text, 1, true) then
      return index
    end
  end
  return nil
end

---Goes to the sidebar of the current tab, its cursor on the line holding `text`.
---@param text string
local function cursor_on(text)
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_buf(win) == sidebar.buffer() then
      vim.api.nvim_set_current_win(win)
      vim.api.nvim_win_set_cursor(win, { assert(line_with(text), text), 0 })
      return
    end
  end
  error('no sidebar in the current tab')
end

---Presses `key` in the sidebar of the current tab, on the line holding `text`.
---@param key string
---@param text string
local function press_on(key, text)
  cursor_on(text)
  vim.cmd('normal ' .. vim.keycode(key))
end

---Waits until a notification arrives, and returns it.
---@param ui spec.Ui
---@param seen integer how many notifications there were before
---@return { message: string, level: integer }
local function note_after(ui, seen)
  support.wait_for(function()
    return #ui.notes > seen
  end, 'a notification')
  return ui.notes[seen + 1]
end

---The runs of the fake `gh` of `repo` that created a pull request, one string each.
---@param repo spec.Repo
---@return string[]
local function creates(repo)
  return vim.tbl_filter(function(call)
    return vim.startswith(call, 'pr create')
  end, support.gh_calls(repo))
end

---Whether the bare repository `origin` holds the branch `branch`.
---@param origin string
---@param branch string
---@return boolean
local function has_branch(origin, branch)
  local ref = 'refs/heads/' .. branch
  return vim.system({ 'git', '-C', origin, 'rev-parse', '--verify', '--quiet', ref }):wait().code
    == 0
end

---Commits the file `file` in the worktree at `path`, with `subject` and `body` as its message.
---@param path string
---@param file string
---@param subject string
---@param body string
local function commit(path, file, subject, body)
  vim.fn.writefile({ subject }, path .. '/' .. file)
  support.run({ 'git', '-C', path, 'add', file })
  support.run({ 'git', '-C', path, 'commit', '--quiet', '-m', subject, '-m', body })
end

---A repository whose `origin` is a bare repository beside it holding `main`, its worktrees kept
---under the fixture's own directory (`base:`), with the worktree `demo` one commit ahead of `main`,
---Neovim in its primary checkout and the sidebar open.
---@return spec.Repo repo
---@return string origin the bare repository
---@return string path the worktree `demo`
local function pr_repo()
  support.load_client()
  local repo = support.repo()
  local origin = repo.tmp .. '/origin.git'
  support.run({ 'git', 'init', '--quiet', '--bare', origin })
  support.git(repo, { 'remote', 'add', 'origin', origin })
  support.git(repo, { 'push', '--quiet', 'origin', 'main' })
  vim.fn.writefile({ 'base: ' .. repo.tmp .. '/worktrees' }, repo.root .. '/.git/orca-term.yaml')
  support.run({ support.binary, 'worktree', 'new', 'demo' }, repo.root)
  local path = support.worktree_path(repo, 'demo')
  commit(path, 'login.txt', 'Fix the login', 'The session was dropped on refresh.')
  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  return repo, origin, path
end

---The windows of the current tab.
---@return integer
local function window_count()
  return #vim.api.nvim_tabpage_list_wins(0)
end

---The lines of the buffer `buf`.
---@param buf integer
---@return string[]
local function lines_of(buf)
  return vim.api.nvim_buf_get_lines(buf, 0, -1, false)
end

describe('the pull request composer', function()
  local columns = vim.o.columns

  before_each(function()
    vim.o.columns = 160
  end)

  after_each(function()
    support.reset_editor()
    for _, buf in ipairs(vim.api.nvim_list_bufs()) do
      if vim.startswith(vim.api.nvim_buf_get_name(buf), compose.PREFIX) then
        vim.api.nvim_buf_delete(buf, { force = true })
      end
    end
    support.cleanup()
    vim.o.columns = columns
  end)

  it('is offered in the menu, before Remove worktree', function()
    pr_repo()
    local ui = support.stub_ui({})

    press_on('.', '· demo')

    assert.are.same({
      {
        'Re-run setup',
        'Copy path',
        'Swap agent',
        'New session',
        'Create pull request',
        'Create draft pull request',
        -- This repository has `origin/main`, so the rebase is offered too (#141).
        'Rebase onto origin/main',
        'Remove worktree',
      },
    }, ui.shown)
  end)

  it('opens in a split prefilled by pr compose, the title, a blank line, the body', function()
    pr_repo()
    support.stub_ui({ select = { 'Create pull request' } })
    local windows = window_count()

    press_on('.', '· demo')

    local buf = assert(compose.buffer('demo'), 'no composer for demo')
    assert.are.equal('orca-term://pr/demo', vim.api.nvim_buf_get_name(buf))
    assert.are.equal('markdown', vim.bo[buf].filetype)
    assert.are.equal('acwrite', vim.bo[buf].buftype)
    assert.are.same({ 'Fix the login', '', 'The session was dropped on refresh.' }, lines_of(buf))
    assert.is_false(vim.bo[buf].modified)
    assert.are.equal(buf, vim.api.nvim_get_current_buf())
    assert.are.equal(windows + 1, window_count())
    assert.is_false(compose.is_draft(buf))
  end)

  it(':w pushes the branch, creates the pull request, closes the buffer and shows PR', function()
    local repo, origin = pr_repo()
    local ui = support.stub_ui({ select = { 'Create pull request' } })
    press_on('.', '· demo')
    local buf = assert(compose.buffer('demo'))
    vim.api.nvim_buf_set_lines(buf, -1, -1, false, { '', '- [x] tested' })
    local seen = #ui.notes

    vim.cmd('write')

    assert.are.same({
      message = 'orca-term: creating the pull request of demo',
      level = vim.log.levels.INFO,
    }, note_after(ui, seen))
    assert.are.same({
      message = 'orca-term: created PR #12: https://github.com/o/r/pull/12',
      level = vim.log.levels.INFO,
    }, note_after(ui, seen + 1))
    assert.is_false(vim.api.nvim_buf_is_valid(buf))
    assert.is_nil(compose.buffer('demo'))

    local calls = creates(repo)
    assert.are.equal(1, #calls)
    local body_file = calls[1]:match('%-%-body%-file (%S+)$')
    assert.are.equal(
      'pr create --base main --head demo --title Fix the login --body-file ' .. body_file,
      calls[1]
    )
    assert.are.same(
      { 'The session was dropped on refresh.', '', '- [x] tested' },
      vim.fn.readfile(repo.gh .. '/create.body')
    )
    assert.are.equal(0, vim.fn.filereadable(body_file), 'the body file was left behind')
    assert.is_true(has_branch(origin, 'demo'))

    -- The core kept the new pull request, and the sidebar shows it on the row's second line.
    assert.are.equal(12, support.listed(repo, 'demo').pr.number)
    support.wait_for(function()
      local row = line_with('· demo')
      -- The selected row is drawn in a card: its border is taken off first.
      local second = row and (sidebar_lines()[row + 1] or ''):gsub('│', '') or ''
      return vim.endswith(vim.trim(second), ' PR')
    end, 'PR on the row of demo')
  end)

  it('creates a draft, and warns of the files with uncommitted changes left out', function()
    local repo, _, path = pr_repo()
    vim.fn.writefile({ 'wip' }, path .. '/wip.txt')
    local ui = support.stub_ui({ select = { 'Create draft pull request' } })
    press_on('.', '· demo')
    local buf = assert(compose.buffer('demo'))
    assert.is_true(compose.is_draft(buf))
    vim.api.nvim_buf_set_lines(buf, 0, 1, false, { 'Fix the login for good' })
    local seen = #ui.notes

    vim.cmd('write')

    assert.are.equal(
      'orca-term: created PR #12 (draft): https://github.com/o/r/pull/12',
      note_after(ui, seen + 1).message
    )
    assert.are.same({
      message = 'orca-term: 1 file with uncommitted changes was left out of the pull request',
      level = vim.log.levels.WARN,
    }, note_after(ui, seen + 2))
    local calls = creates(repo)
    assert.are.equal(1, #calls)
    assert(
      vim.startswith(calls[1], 'pr create --base main --head demo --title Fix the login for good '),
      calls[1]
    )
    assert(vim.endswith(calls[1], ' --draft'), calls[1])
  end)

  it('keeps the buffer and its text when the core refuses or gh fails', function()
    local repo, origin = pr_repo()
    support.gh_answer(repo, 'demo', {
      number = 7,
      title = 'Fix the login',
      state = 'OPEN',
      isDraft = false,
      url = 'https://github.com/o/r/pull/7',
      statusCheckRollup = {},
    })
    local ui = support.stub_ui({})
    -- `:OrcaTerm pr-create` in the sidebar opens it for the worktree under the cursor.
    cursor_on('· demo')
    vim.cmd('OrcaTerm pr-create')
    local buf = assert(compose.buffer('demo'))
    vim.api.nvim_buf_set_lines(buf, 0, 1, false, { 'My own title' })
    local text = lines_of(buf)
    local seen = #ui.notes

    vim.cmd('write')

    local refused = note_after(ui, seen + 1)
    assert.are.equal(vim.log.levels.ERROR, refused.level)
    assert(refused.message:find('#7', 1, true), refused.message)
    assert(vim.startswith(refused.message, 'orca-term: '), refused.message)
    assert.is_true(vim.api.nvim_buf_is_valid(buf))
    assert.are.same(text, lines_of(buf))
    assert.is_true(vim.bo[buf].modified)
    assert.are.same({}, creates(repo))
    assert.is_false(has_branch(origin, 'demo'))

    -- No pull request any more, but gh fails to create one: the branch was pushed, the text stays.
    vim.fn.delete(repo.gh .. '/demo.json')
    vim.fn.writefile({ 'HTTP 422: Validation Failed' }, repo.gh .. '/create.err')
    seen = #ui.notes
    vim.cmd('write')

    local failed = note_after(ui, seen + 1)
    assert.are.equal(vim.log.levels.ERROR, failed.level)
    assert(failed.message:find('HTTP 422: Validation Failed', 1, true), failed.message)
    assert(failed.message:find('branch demo was pushed', 1, true), failed.message)
    assert.are.equal(1, #creates(repo))
    assert.is_true(vim.api.nvim_buf_is_valid(buf))
    assert.are.same(text, lines_of(buf))
  end)

  it('refuses an empty title before anything runs', function()
    local repo = pr_repo()
    local ui = support.stub_ui({ select = { 'Create pull request' } })
    press_on('.', '· demo')
    local buf = assert(compose.buffer('demo'))
    vim.api.nvim_buf_set_lines(buf, 0, 1, false, { '   ' })
    local seen = #ui.notes

    vim.cmd('write')

    assert.are.same({
      message = 'orca-term: ' .. compose.EMPTY_TITLE,
      level = vim.log.levels.ERROR,
    }, ui.notes[seen + 1])
    vim.wait(300)
    assert.are.equal(seen + 1, #ui.notes)
    assert.are.same({}, creates(repo))
    assert.is_true(vim.api.nvim_buf_is_valid(buf))
    assert.are.same({ '   ', '', 'The session was dropped on refresh.' }, lines_of(buf))
  end)

  it('creates nothing when closed with :q! or wiped', function()
    local repo, origin = pr_repo()
    support.stub_ui({ select = { 'Create pull request' } })
    press_on('.', '· demo')
    local buf = assert(compose.buffer('demo'))
    vim.api.nvim_buf_set_lines(buf, 0, 1, false, { 'Changed my mind' })

    vim.cmd('quit!')

    assert.is_false(vim.api.nvim_buf_is_valid(buf))
    assert.is_nil(compose.buffer('demo'))

    support.stub_ui({ select = { 'Create draft pull request' } })
    press_on('.', '· demo')
    buf = assert(compose.buffer('demo'))
    vim.cmd('bwipeout!')

    assert.is_false(vim.api.nvim_buf_is_valid(buf))
    vim.wait(300)
    assert.are.same({}, creates(repo))
    assert.is_false(has_branch(origin, 'demo'))
  end)

  it(":OrcaTerm pr-create opens it for the current tab's worktree, and once only", function()
    local _, _, path = pr_repo()
    support.stub_ui({})
    press_on('<CR>', '· demo')
    assert.are.equal(path, vim.fn.getcwd(-1, vim.fn.tabpagenr()))

    vim.cmd('OrcaTerm pr-create')

    local buf = assert(compose.buffer('demo'))
    assert.are.equal(buf, vim.api.nvim_get_current_buf())
    vim.api.nvim_buf_set_lines(buf, 0, 1, false, { 'Kept' })

    -- Asked again, from another window: the same buffer, its text kept, now a draft.
    local windows = window_count()
    support.stub_ui({ select = { 'Create draft pull request' } })
    press_on('.', '· demo')

    assert.are.equal(buf, vim.api.nvim_get_current_buf())
    assert.are.equal('Kept', lines_of(buf)[1])
    assert.are.equal(windows, window_count())
    assert.is_true(compose.is_draft(buf))
  end)
end)
