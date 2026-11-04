-- Rebasing a worktree onto its base (#141). Seam 2: the sidebar's menu offers `Rebase onto
-- origin/main` while no operation is in progress, and `Resolve conflicts with the agent` and
-- `Abort rebase` during a rebase; each runs the real binary, against a bare `origin` that gets a new
-- commit on `main` from another clone, and the fake claude of `support.lua`. `vim.ui` and
-- `vim.notify` are stubbed; the core never is.

local support = require('support')
local actions = require('orca-term.actions')
local pr = require('orca-term.pr')
local sidebar = require('orca-term.sidebar')

---The menu of a worktree with no operation in progress, in a repository with `origin/main`.
local MENU_WITH_REBASE = {
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

---The menu of a worktree in the middle of a rebase.
local MENU_DURING_REBASE = {
  'Re-run setup',
  'Copy path',
  'Swap agent',
  'New session',
  'Create pull request',
  'Create draft pull request',
  'Resolve conflicts with the agent',
  'Abort rebase',
  'Remove worktree',
}

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

---Presses `key` in the sidebar of the current tab, on the row of `name`.
---@param key string
---@param name string
local function press_on(key, name)
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_buf(win) == sidebar.buffer() then
      vim.api.nvim_set_current_win(win)
      vim.api.nvim_win_set_cursor(win, { assert(row_of(name), name), 0 })
      vim.cmd('normal ' .. vim.keycode(key))
      return
    end
  end
  error('no sidebar in the current tab')
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

---Waits until `.` on `demo` offers `items`, the sidebar having read the fleet again.
---@param items string[]
local function wait_for_menu(items)
  local ui = support.stub_ui({})
  support.wait_for(function()
    press_on('.', 'demo')
    return vim.deep_equal(ui.shown[#ui.shown], items)
  end, 'the menu ' .. table.concat(items, ', '))
end

---@class spec.RebaseRepo: spec.AgentRepo
---@field upstream string another clone of `origin`, where its `main` gets new commits

---Runs git in `dir` and returns its stdout, trimmed.
---@param dir string
---@param args string[]
---@return string
local function git_in(dir, args)
  return vim.trim(support.run(vim.list_extend({ 'git', '-C', dir }, args)))
end

---Writes `contents` to `file` in `dir` and commits it.
---@param dir string
---@param file string
---@param contents string
local function commit_in(dir, file, contents)
  vim.fn.writefile({ contents }, dir .. '/' .. file)
  git_in(dir, { 'add', file })
  git_in(dir, { 'commit', '--quiet', '-m', 'change ' .. file })
end

---A repository whose agents are the fake claude, with `shared.txt` on `main`, a bare `origin`
---holding it and another clone of it (`upstream`), the worktree `demo` (kept under the fixture's
---own directory), Neovim in the primary checkout and the sidebar open. With `remote` false, there
---is no `origin` at all.
---@param remote? boolean
---@return spec.RebaseRepo repo
---@return string path the worktree `demo`
local function rebase_repo(remote)
  support.load_client()
  local repo = support.agent_repo() --[[@as spec.RebaseRepo]]
  vim.fn.writefile({ 'one' }, repo.root .. '/shared.txt')
  support.git(repo, { 'add', 'shared.txt' })
  support.git(repo, { 'commit', '--quiet', '-m', 'shared' })
  if remote ~= false then
    local origin = repo.tmp .. '/origin.git'
    support.run({ 'git', 'init', '--quiet', '--bare', origin })
    support.git(repo, { 'remote', 'add', 'origin', origin })
    support.git(repo, { 'push', '--quiet', 'origin', 'main' })
    repo.upstream = repo.tmp .. '/upstream'
    support.run({ 'git', 'clone', '--quiet', origin, repo.upstream })
  end
  vim.fn.writefile({ 'base: ' .. repo.tmp .. '/worktrees' }, repo.root .. '/.git/orca-term.yaml')
  support.run({ support.binary, 'worktree', 'new', 'demo' }, repo.root)
  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  support.wait_for(function()
    return row_of('demo') ~= nil
  end, 'the sidebar')
  return repo, support.worktree_path(repo, 'demo')
end

---Gives `origin`'s `main` a new commit changing `file` to `contents`, from the other clone.
---@param repo spec.RebaseRepo
---@param file string
---@param contents string
local function advance_origin(repo, file, contents)
  commit_in(repo.upstream, file, contents)
  git_in(repo.upstream, { 'push', '--quiet', 'origin', 'main' })
end

---Makes `demo` and `origin` change `shared.txt` each their own way, then rebases `demo` from the
---menu, which stops on the conflict and warns. Returns the warning.
---@param repo spec.RebaseRepo
---@param path string
---@return { message: string, level: integer }
local function rebase_into_conflict(repo, path)
  commit_in(path, 'shared.txt', 'mine')
  advance_origin(repo, 'shared.txt', 'theirs')
  local ui = support.stub_ui({ select = { 'Rebase onto origin/main' } })
  press_on('.', 'demo')
  assert.are.equal('orca-term: rebasing demo onto origin/main', note_after(ui, 0).message)
  local warned = note_after(ui, 1)
  support.wait_for(function()
    return support.listed(repo, 'demo').operation == 'rebase'
  end, 'the rebase in progress')
  return warned
end

describe('rebasing a worktree', function()
  after_each(function()
    pr._reset()
    support.reset_editor()
    support.cleanup()
  end)

  it('is offered, naming the base, only when the repository has one on origin', function()
    rebase_repo(false)
    local ui = support.stub_ui({})
    press_on('.', 'demo')
    assert.are.same(actions.MENU, ui.shown[1])
    support.cleanup()
    support.reset_editor()

    rebase_repo()
    ui = support.stub_ui({})
    press_on('.', 'demo')
    assert.are.same(MENU_WITH_REBASE, ui.shown[1])
  end)

  it('rebases onto origin/main through the core and says what it replayed', function()
    local repo, path = rebase_repo()
    commit_in(path, 'a.txt', 'a')
    advance_origin(repo, 'c.txt', 'c')
    local upstream = git_in(repo.upstream, { 'rev-parse', 'HEAD' })
    local ui = support.stub_ui({ select = { 'Rebase onto origin/main' } })

    press_on('.', 'demo')

    assert.are.same({
      message = 'orca-term: rebasing demo onto origin/main',
      level = vim.log.levels.INFO,
    }, note_after(ui, 0))
    assert.are.same({
      message = 'orca-term: demo: rebased onto origin/main (1 commit replayed)',
      level = vim.log.levels.INFO,
    }, note_after(ui, 1))
    assert.are.equal(upstream, git_in(path, { 'rev-parse', 'HEAD~1' }))

    ui = support.stub_ui({ select = { 'Rebase onto origin/main' } })
    press_on('.', 'demo')
    assert.are.equal(
      'orca-term: demo: already up to date with origin/main',
      note_after(ui, 1).message
    )
  end)

  it('leaves a conflict in progress, warns, and offers to resolve or abort it', function()
    local repo, path = rebase_repo()

    local warned = rebase_into_conflict(repo, path)

    assert.are.same({
      message = 'orca-term: the rebase of worktree demo onto origin/main stopped on conflicts in '
        .. 'shared.txt; it stays in progress: send them to the agent or abort the rebase, from '
        .. 'the worktree menu or with `orca-term agent resolve demo` or `orca-term worktree '
        .. 'rebase demo --abort`',
      level = vim.log.levels.WARN,
    }, warned)
    wait_for_menu(MENU_DURING_REBASE)
    -- The row's second line shows `≠`, the operation in progress.
    local row = assert(row_of('demo'))
    local second = vim.api.nvim_buf_get_lines(assert(sidebar.buffer()), row, row + 1, false)[1]
    assert(second:find('≠', 1, true), second)
  end)

  it('asks before aborting, and Cancel leaves the rebase in progress', function()
    local repo, path = rebase_repo()
    rebase_into_conflict(repo, path)
    wait_for_menu(MENU_DURING_REBASE)

    local ui = support.stub_ui({ select = { 'Abort rebase', 'Cancel' } })
    press_on('.', 'demo')
    assert.are.equal(
      'Abort the rebase of demo? Its branch goes back to where it was.',
      ui.prompts[2]
    )
    assert.are.same({ 'Abort rebase', 'Cancel' }, ui.shown[2])
    vim.wait(300)
    assert.are.equal('rebase', support.listed(repo, 'demo').operation)

    ui = support.stub_ui({ select = { 'Abort rebase', 'Abort rebase' } })
    press_on('.', 'demo')
    assert.are.same({
      message = 'orca-term: aborted the rebase of demo',
      level = vim.log.levels.INFO,
    }, note_after(ui, 0))
    assert.are.equal(vim.NIL, support.listed(repo, 'demo').operation)
    assert.are.same({ 'mine' }, vim.fn.readfile(path .. '/shared.txt'))
    wait_for_menu(MENU_WITH_REBASE)
  end)

  it('types the prompt into the running agent, and says why when none runs', function()
    local repo, path = rebase_repo()
    rebase_into_conflict(repo, path)
    wait_for_menu(MENU_DURING_REBASE)

    local ui = support.stub_ui({ select = { 'Resolve conflicts with the agent' } })
    press_on('.', 'demo')
    local refused = note_after(ui, 0)
    assert.are.equal(vim.log.levels.ERROR, refused.level)
    assert.are.equal(
      ('orca-term: worktree demo at %s has no running agent to resolve the conflicts; start it '):format(
        path
      ) .. 'with `orca-term agent start demo`, then try again (the rebase stays in progress)',
      refused.message
    )

    support.run({ support.binary, 'agent', 'start', 'demo' }, repo.root)
    support.wait_for(function()
      return support.claude_argv(repo) ~= nil
    end, 'the agent to start')
    ui = support.stub_ui({ select = { 'Resolve conflicts with the agent' } })
    press_on('.', 'demo')

    assert.are.same({
      message = 'orca-term: sent the conflicts of demo to its agent',
      level = vim.log.levels.INFO,
    }, note_after(ui, 0))
    support.wait_for(function()
      return vim.tbl_contains(support.claude_input(repo), '- shared.txt')
    end, 'the prompt reaching the agent')
    local typed = table.concat(support.claude_input(repo), '\n')
    assert(typed:find('Never run `git rebase --abort`, `git reset`, `git stash`', 1, true), typed)
    assert.are.equal('rebase', support.listed(repo, 'demo').operation)
  end)
end)
