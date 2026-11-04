-- Pushing a worktree's branch (#146). Seam 2: the sidebar's menu offers `Push` when the worktree
-- is on a branch and the repository has `origin`, and `:OrcaTerm push` does the same for the
-- worktree under the cursor or the current tab's; each runs the real binary against a bare
-- `origin`. A push git rejects because the branch diverged asks before pushing with a lease.
-- `vim.ui` and `vim.notify` are stubbed; the core never is.

local support = require('support')
local actions = require('orca-term.actions')
local pr = require('orca-term.pr')
local sidebar = require('orca-term.sidebar')

---The menu of a worktree on a branch, in a repository with `origin/main`.
local MENU_WITH_PUSH = {
  'Re-run setup',
  'Copy path',
  'Swap agent',
  'New session',
  'Create pull request',
  'Create draft pull request',
  'Rebase onto origin/main',
  'Push',
  'Remove worktree',
}

---The menu of a worktree on no branch, in a repository with `origin/main`.
local MENU_DETACHED = {
  'Re-run setup',
  'Copy path',
  'Swap agent',
  'New session',
  'Create pull request',
  'Create draft pull request',
  'Rebase onto origin/main',
  'Remove worktree',
}

---What the sidebar asks when git rejected the push of `demo` because it diverged.
local DIVERGED_PROMPT = 'Push rejected: demo diverged from origin/demo (after a rebase?)'

---The sidebar's line number of the worktree `name`.
---@param name string
---@return integer? row 1-based
local function row_of(name)
  local lines = vim.api.nvim_buf_get_lines(assert(sidebar.buffer()), 0, -1, false)
  for index, line in ipairs(lines) do
    if line:find(' ' .. name .. ' ', 1, true) or vim.endswith(line, ' ' .. name) then
      return index
    end
  end
  return nil
end

---Puts the cursor of the sidebar of the current tab on the row of `name`.
---@param name string
local function cursor_on(name)
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_buf(win) == sidebar.buffer() then
      vim.api.nvim_set_current_win(win)
      vim.api.nvim_win_set_cursor(win, { assert(row_of(name), name), 0 })
      return
    end
  end
  error('no sidebar in the current tab')
end

---Presses `key` in the sidebar of the current tab, on the row of `name`.
---@param key string
---@param name string
local function press_on(key, name)
  cursor_on(name)
  vim.cmd('normal ' .. vim.keycode(key))
end

---Waits until the notification after the first `seen` arrives, and returns it.
---@param ui spec.Ui
---@param seen integer
---@return { message: string, level: integer }
local function note_after(ui, seen)
  support.wait_for(function()
    return #ui.notes > seen
  end, 'a notification')
  return ui.notes[seen + 1]
end

---Runs git in `dir` and returns its stdout, trimmed.
---@param dir string
---@param args string[]
---@return string
local function git_in(dir, args)
  return vim.trim(support.run(vim.list_extend({ 'git', '-C', dir }, args)))
end

---@class spec.PushRepo: spec.AgentRepo
---@field origin string the bare repository `origin`

---The commit `origin` holds for `branch`, or an empty string when it has none.
---@param repo spec.PushRepo
---@param branch string
---@return string
local function on_origin(repo, branch)
  return git_in(repo.origin, { 'for-each-ref', '--format=%(objectname)', 'refs/heads/' .. branch })
end

---A repository with `main` on a bare `origin`, the worktree `demo` with one commit of its own
---(kept under the fixture's own directory), Neovim in the primary checkout and the sidebar open.
---With `remote` false, there is no `origin` at all.
---@param remote? boolean
---@return spec.PushRepo repo
---@return string path the worktree `demo`
local function push_repo(remote)
  support.load_client()
  local repo = support.agent_repo() --[[@as spec.PushRepo]]
  if remote ~= false then
    repo.origin = repo.tmp .. '/origin.git'
    support.run({ 'git', 'init', '--quiet', '--bare', repo.origin })
    support.git(repo, { 'remote', 'add', 'origin', repo.origin })
    support.git(repo, { 'push', '--quiet', 'origin', 'main' })
  end
  vim.fn.writefile({ 'base: ' .. repo.tmp .. '/worktrees' }, repo.root .. '/.git/orca-term.yaml')
  support.run({ support.binary, 'worktree', 'new', 'demo' }, repo.root)
  local path = support.worktree_path(repo, 'demo')
  git_in(path, { 'commit', '--quiet', '--allow-empty', '-m', 'Add a' })
  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  support.wait_for(function()
    return row_of('demo') ~= nil
  end, 'the sidebar')
  return repo, path
end

describe('pushing a worktree', function()
  after_each(function()
    pr._reset()
    support.reset_editor()
    support.cleanup()
  end)

  it('is offered only on a branch, in a repository with origin', function()
    push_repo(false)
    local ui = support.stub_ui({})
    press_on('.', 'demo')
    assert.are.same(actions.MENU, ui.shown[1])
    support.cleanup()
    support.reset_editor()

    local _, path = push_repo()
    ui = support.stub_ui({})
    press_on('.', 'demo')
    assert.are.same(MENU_WITH_PUSH, ui.shown[1])

    git_in(path, { 'switch', '--quiet', '--detach' })
    support.wait_for(function()
      ui = support.stub_ui({})
      sidebar.refresh()
      press_on('.', 'demo')
      return vim.deep_equal(ui.shown[1], MENU_DETACHED)
    end, 'the menu of a detached worktree')
  end)

  it('pushes through the core, says where, and warns of files left out', function()
    local repo, path = push_repo()
    local ui = support.stub_ui({ select = { 'Push' } })

    press_on('.', 'demo')

    assert.are.same({
      message = 'orca-term: pushing demo',
      level = vim.log.levels.INFO,
    }, note_after(ui, 0))
    assert.are.same({
      message = 'orca-term: demo: pushed to origin/demo',
      level = vim.log.levels.INFO,
    }, note_after(ui, 1))
    assert.are.equal(git_in(path, { 'rev-parse', 'HEAD' }), on_origin(repo, 'demo'))

    vim.fn.writefile({ 'not committed' }, path .. '/scratch.txt')
    ui = support.stub_ui({ select = { 'Push' } })
    press_on('.', 'demo')
    assert.are.equal(
      'orca-term: demo: origin/demo is already up to date',
      note_after(ui, 1).message
    )
    assert.are.same({
      message = 'orca-term: 1 file with uncommitted changes was left out of the push',
      level = vim.log.levels.WARN,
    }, note_after(ui, 2))
  end)

  it('asks before a push with lease when the branch diverged, and Cancel pushes nothing', function()
    local repo, path = push_repo()
    support.run({ support.binary, 'worktree', 'push', 'demo' }, repo.root)
    local pushed = on_origin(repo, 'demo')
    git_in(path, { 'commit', '--quiet', '--amend', '--allow-empty', '-m', 'Add a, reworded' })
    local ui = support.stub_ui({ select = { 'Push', 'Cancel' } })

    press_on('.', 'demo')

    support.wait_for(function()
      return #ui.prompts == 2
    end, 'the diverged prompt')
    assert.are.equal(DIVERGED_PROMPT, ui.prompts[2])
    assert.are.same(actions.FORCE_PUSH_ANSWERS, ui.shown[2])
    assert.are.same({ 'Force push (with lease)', 'Cancel' }, ui.shown[2])
    vim.wait(300)
    assert.are.equal(1, #ui.notes, 'only `pushing demo`')
    assert.are.equal(pushed, on_origin(repo, 'demo'))

    ui = support.stub_ui({ select = { 'Push', 'Force push (with lease)' } })
    press_on('.', 'demo')
    assert.are.same({
      message = 'orca-term: pushing demo with lease',
      level = vim.log.levels.INFO,
    }, note_after(ui, 1))
    assert.are.same({
      message = 'orca-term: demo: pushed to origin/demo',
      level = vim.log.levels.INFO,
    }, note_after(ui, 2))
    assert.are.equal(git_in(path, { 'rev-parse', 'HEAD' }), on_origin(repo, 'demo'))
  end)

  it("tells the core's reason when the lease is refused", function()
    local repo, path = push_repo()
    support.run({ support.binary, 'worktree', 'push', 'demo' }, repo.root)
    git_in(path, { 'commit', '--quiet', '--amend', '--allow-empty', '-m', 'Add a, reworded' })
    -- Someone else pushes onto demo meanwhile, which this repository has not fetched.
    local other = repo.tmp .. '/other'
    support.run({ 'git', 'clone', '--quiet', '--branch', 'demo', repo.origin, other })
    git_in(other, { 'commit', '--quiet', '--allow-empty', '-m', 'Their change' })
    git_in(other, { 'push', '--quiet', 'origin', 'demo' })
    local theirs = git_in(other, { 'rev-parse', 'HEAD' })
    -- Their commit is here, but `origin/demo` still holds what this repository pushed: the plain
    -- push is a non-fast-forward, and the lease, on that stale commit, is refused. Fetching by
    -- path, not by the remote's name, leaves `origin/demo` alone.
    support.git(repo, { 'fetch', '--quiet', repo.origin, 'demo:refs/theirs' })
    local ui = support.stub_ui({ select = { 'Push', 'Force push (with lease)' } })

    press_on('.', 'demo')

    local refused = note_after(ui, 2)
    assert.are.equal(vim.log.levels.ERROR, refused.level)
    assert(
      vim.startswith(
        refused.message,
        'orca-term: could not push branch demo of worktree demo to origin: '
      ),
      refused.message
    )
    assert(refused.message:find('(stale info)', 1, true), refused.message)
    assert.are.equal(theirs, on_origin(repo, 'demo'))
  end)

  it(":OrcaTerm push pushes the current tab's worktree, and says why it cannot", function()
    local repo, path = push_repo()
    local ui = support.stub_ui({})
    press_on('<CR>', 'demo')
    assert.are.equal(path, vim.fn.getcwd(-1, vim.fn.tabpagenr()))
    ui = support.stub_ui({})

    vim.cmd('OrcaTerm push')

    assert.are.equal('orca-term: demo: pushed to origin/demo', note_after(ui, 1).message)
    assert.are.equal(git_in(path, { 'rev-parse', 'HEAD' }), on_origin(repo, 'demo'))

    support.git(repo, { 'remote', 'remove', 'origin' })
    ui = support.stub_ui({})
    vim.cmd('OrcaTerm push')
    assert.are.same({
      message = 'orca-term: worktree demo cannot be pushed: this repository has no `origin` '
        .. 'remote (nothing was pushed)',
      level = vim.log.levels.ERROR,
    }, note_after(ui, 1))
  end)
end)
