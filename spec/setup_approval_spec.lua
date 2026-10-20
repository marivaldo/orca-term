-- Seam 2: approving a committed setup script from the sidebar (#58), in headless Neovim against
-- the real binary. When the core refuses a setup script from a committed `orca-term.yaml` as
-- `untrusted`, `a` (a new worktree) and `.` then `Re-run setup` show the script and where it comes
-- from, and run it only on `Run it` (`worktree setup --approve-setup`); `Cancel` leaves the
-- worktree not ready. `vim.ui` and `vim.notify` are stubbed; the core never is.

local support = require('support')
local sidebar = require('orca-term.sidebar')

---The sidebar buffer's lines.
---@return string[]
local function lines()
  return vim.api.nvim_buf_get_lines(assert(sidebar.buffer()), 0, -1, false)
end

---The index of the first sidebar line holding `text`, or `nil`.
---@param text string
---@return integer?
local function line_with(text)
  for index, line in ipairs(lines()) do
    if line:find(text, 1, true) then
      return index
    end
  end
  return nil
end

---Presses `key` in the sidebar of the current tab, on the line holding `text`.
---@param key string
---@param text string
local function press_on(key, text)
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_buf(win) == sidebar.buffer() then
      vim.api.nvim_set_current_win(win)
      vim.api.nvim_win_set_cursor(win, { assert(line_with(text), text), 0 })
      vim.cmd('normal ' .. vim.keycode(key))
      return
    end
  end
  error('no sidebar in the current tab')
end

---Waits until a notification arrives, and returns it.
---@param ui spec.Ui
---@return { message: string, level: integer }
local function next_note(ui)
  local seen = #ui.notes
  support.wait_for(function()
    return #ui.notes > seen
  end, 'a notification')
  return ui.notes[seen + 1]
end

---The committed setup script of these specs: it appends a line to `ran.txt`, beside the
---worktrees, so a spec can tell whether it ran.
local SCRIPT = { 'echo ran >> ../ran.txt' }

---What the committed script wrote: one `ran` per run, empty when it never ran.
---@param repo spec.Repo
---@return string[]
local function runs(repo)
  local file = repo.home .. '/orca-term/worktrees/repo/ran.txt'
  if vim.uv.fs_stat(file) == nil then
    return {}
  end
  return vim.fn.readfile(file)
end

---A repository whose committed `orca-term.yaml` holds `SCRIPT`, with the worktrees `names` made
---by the core (whose setup it refused), Neovim in its primary checkout and the sidebar open.
---@param names string[]
---@return spec.Repo
local function repo_with(names)
  support.load_client()
  local repo = support.repo()
  support.commit_setup(repo, SCRIPT)
  for _, name in ipairs(names) do
    local result = support.core(repo, { 'worktree', 'new', name, '--json' })
    assert.are.equal(8, result.code, result.stdout)
  end
  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  return repo
end

---The question shown for the committed script of the worktree `name`.
---@param repo spec.Repo
---@param name string
---@return string
local function question(repo, name)
  return table.concat({
    ('Run the setup script of %s from %s/orca-term.yaml?'):format(
      name,
      repo.home .. '/orca-term/worktrees/repo/' .. name
    ),
    'It is committed to the repository and not approved yet:',
    '',
    '    echo ran >> ../ran.txt',
  }, '\n')
end

describe('a committed setup script', function()
  local columns = vim.o.columns

  before_each(function()
    vim.o.columns = 160
  end)

  after_each(function()
    support.reset_editor()
    support.cleanup()
    vim.o.columns = columns
  end)

  it('is shown when a new worktree is created, and runs once approved', function()
    local repo = repo_with({})
    local ui = support.stub_ui({ input = 'gamma', select = { 'No agent', 'Run it' } })

    press_on('a', 'no worktrees yet')

    assert.are.same({
      message = 'orca-term: created worktree gamma',
      level = vim.log.levels.INFO,
    }, next_note(ui))
    assert.are.same(
      { 'New worktree name: ', 'Agent for gamma', question(repo, 'gamma') },
      ui.prompts
    )
    assert.are.same({ 'Run it', 'Cancel' }, ui.shown[2])
    assert.are.same({ 'ran' }, runs(repo))
    assert.are.equal('no_agent', support.listed(repo, 'gamma').state)
    assert.are.equal(support.worktree_path(repo, 'gamma'), vim.fn.getcwd(-1, vim.fn.tabpagenr()))
  end)

  it('cancelled, runs nothing and leaves the new worktree not ready', function()
    local repo = repo_with({})
    local ui = support.stub_ui({ input = 'gamma', select = { 'No agent', 'Cancel' } })

    press_on('a', 'no worktrees yet')

    assert.are.same({
      message = 'orca-term: the setup script of gamma was not approved; the worktree is not ready',
      level = vim.log.levels.WARN,
    }, next_note(ui))
    assert.are.same({}, runs(repo))
    local gamma = support.listed(repo, 'gamma')
    assert.are.equal('not_ready', gamma.state)
    assert.are.equal('setup not approved; run orca-term worktree setup gamma', gamma.detail)
    support.wait_for(function()
      return line_with('△ gamma') ~= nil
    end, 'the sidebar to show gamma not ready')
  end)

  it('is shown when the setup runs again, and runs once approved', function()
    local repo = repo_with({ 'alpha' })
    local ui = support.stub_ui({ select = { 'Re-run setup', 'Run it' } })

    press_on('.', '△ alpha')

    assert.are.same({
      message = 'orca-term: worktree alpha is ready',
      level = vim.log.levels.INFO,
    }, next_note(ui))
    assert.are.same({ 'Worktree alpha', question(repo, 'alpha') }, ui.prompts)
    assert.are.same({ 'ran' }, runs(repo))
    support.wait_for(function()
      return line_with('· alpha') ~= nil
    end, 'the sidebar to show alpha ready')

    -- Approved once, the same script runs again without asking.
    ui = support.stub_ui({ select = { 'Re-run setup' } })
    press_on('.', '· alpha')
    assert.are.equal('orca-term: worktree alpha is ready', next_note(ui).message)
    assert.are.same({ 'Worktree alpha' }, ui.prompts)
    assert.are.same({ 'ran', 'ran' }, runs(repo))
  end)

  it('cancelled when the setup runs again, leaves the worktree not ready', function()
    local repo = repo_with({ 'alpha' })
    local ui = support.stub_ui({ select = { 'Re-run setup', 'Cancel' } })

    press_on('.', '△ alpha')

    assert.are.equal(vim.log.levels.WARN, next_note(ui).level)
    assert.are.same({}, runs(repo))
    assert.are.equal('not_ready', support.listed(repo, 'alpha').state)
  end)
end)
