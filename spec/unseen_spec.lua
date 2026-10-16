-- Seam 2: the unseen mark in the sidebar, in headless Neovim against the real binary. `u` toggles
-- the mark of the worktree under the cursor through the core (`worktree seen`, `worktree unseen`),
-- whose `worktree ls --json` is then read to check it; a turn that ends, played through the hooks
-- the fake claude's settings register (`support.play_hooks`, as spec/status_spec.lua does), sets
-- it without `u`. The mark lives on disk: a child Neovim (as in spec/live_spec.lua) that quits
-- leaves it for the next one.

local support = require('support')
local render = require('orca-term.render')
local sidebar = require('orca-term.sidebar')

---The sidebar buffer's lines.
---@return string[]
local function lines()
  return vim.api.nvim_buf_get_lines(assert(sidebar.buffer()), 0, -1, false)
end

---The index of the first line holding `text`, or `nil`.
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

---Whether the sidebar shows the worktree `name` with the unseen mark before its glyph.
---@param name string
---@return boolean
local function shows_unseen(name)
  for _, line in ipairs(lines()) do
    -- Inside the card a row starts after ` │`.
    local bare = line:gsub('^ │', '')
    if vim.startswith(bare, render.UNSEEN) and bare:find(' ' .. name, 1, true) then
      return true
    end
  end
  return false
end

---Moves the cursor into the sidebar window of the current tab, onto line `line`.
---@param line integer
local function cursor_on(line)
  local buf = assert(sidebar.buffer())
  local win = assert(vim.fn.win_findbuf(buf)[1])
  vim.api.nvim_set_current_win(win)
  vim.api.nvim_win_set_cursor(win, { line, 0 })
end

---Presses `u` on the name line of the worktree `name`.
---@param name string
local function press_u_on(name)
  cursor_on(assert(line_with(' ' .. name), 'no line for ' .. name))
  vim.cmd('normal u')
end

---A repository with the worktrees `names`, made by the core, and the sidebar open in its primary
---checkout.
---@param names string[]
---@return spec.Repo
local function sidebar_with(names)
  support.load_client()
  local repo = support.repo()
  for _, name in ipairs(names) do
    support.run({ support.binary, 'worktree', 'new', name }, repo.root)
  end
  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  return repo
end

---Child Neovims the spec started, with their socket paths, stopped after it.
---@type { channel: integer, socket: string }[]
local children = {}

---A child Neovim listening at a short socket path of its own, in the primary checkout of `repo`,
---with this client loaded and its sidebar open. Started by its full path: an agent repository's
---PATH holds no nvim.
---@param repo spec.Repo
---@return integer channel
local function start_child(repo)
  local socket = ('/tmp/ot-u-%d-%d.sock'):format(vim.fn.getpid(), #children + 1)
  vim.fn.delete(socket)
  local channel = vim.fn.jobstart({
    vim.v.progpath,
    '--embed',
    '--headless',
    '--clean',
    '--listen',
    socket,
    '--cmd',
    'set runtimepath^=' .. vim.fn.fnameescape(support.root),
  }, { rpc = true, cwd = repo.root })
  assert(channel > 0, 'the child Neovim did not start')
  table.insert(children, { channel = channel, socket = socket })
  vim.rpcrequest(channel, 'nvim_exec_lua', 'vim.o.columns = 160', {})
  vim.rpcrequest(channel, 'nvim_exec_lua', "require('orca-term').setup({ binary = ... })", {
    support.binary,
  })
  vim.rpcrequest(channel, 'nvim_command', 'OrcaTerm sidebar')
  return channel
end

---Lua run in a child Neovim: the sidebar's lines.
local CHILD_LINES = [[
  local buf = require('orca-term.sidebar').buffer()
  return buf and vim.api.nvim_buf_get_lines(buf, 0, -1, false) or {}
]]

---Lua run in a child Neovim: presses `u` on the line of the sidebar holding `...`.
local CHILD_PRESS_U = [[
  local name = ...
  local buf = require('orca-term.sidebar').buffer()
  local win = vim.fn.win_findbuf(buf)[1]
  vim.api.nvim_set_current_win(win)
  for index, line in ipairs(vim.api.nvim_buf_get_lines(buf, 0, -1, false)) do
    if line:find(' ' .. name, 1, true) then
      vim.api.nvim_win_set_cursor(win, { index, 0 })
      vim.cmd('normal u')
      return true
    end
  end
  return false
]]

---Whether the sidebar of the child Neovim shows a line starting with the unseen mark and holding
---`name`.
---@param child integer
---@param name string
---@return boolean
local function child_shows_unseen(child, name)
  local shown = vim.rpcrequest(child, 'nvim_exec_lua', CHILD_LINES, {}) --[[@as string[] ]]
  for _, line in ipairs(shown) do
    local bare = line:gsub('^ │', '')
    if vim.startswith(bare, render.UNSEEN) and bare:find(' ' .. name, 1, true) then
      return true
    end
  end
  return false
end

---Quits the child Neovim with `:qa`, as a person would, and checks it exited 0.
---@param child integer
local function quit(child)
  -- The child exits before it answers: the request fails as its channel closes.
  local answered = pcall(vim.rpcrequest, child, 'nvim_command', 'qa')
  assert(not answered, 'the child is still running after :qa')
  assert.are.same({ 0 }, vim.fn.jobwait({ child }, 10000))
end

describe('the unseen mark', function()
  after_each(function()
    for _, child in ipairs(children) do
      vim.fn.jobstop(child.channel)
      vim.fn.jobwait({ child.channel }, 5000)
      vim.fn.delete(child.socket)
    end
    children = {}
    support.reset_editor()
    support.cleanup()
  end)

  it('is set by u through the core and cleared by u again', function()
    local repo = sidebar_with({ 'fix' })
    assert.is_false(support.listed(repo, 'fix').unseen)
    assert.is_false(shows_unseen('fix'))

    press_u_on('fix')
    support.wait_for(function()
      return shows_unseen('fix')
    end, 'the sidebar to show fix unseen')
    assert.is_true(support.listed(repo, 'fix').unseen)

    press_u_on('fix')
    support.wait_for(function()
      return not shows_unseen('fix')
    end, 'the sidebar to show fix seen')
    assert.is_false(support.listed(repo, 'fix').unseen)
  end)

  it('shows when a turn ends, without u', function()
    support.load_client()
    local repo = support.agent_repo()
    support.run({ support.binary, 'worktree', 'new', 'fix' }, repo.root)
    support.run({ support.binary, 'agent', 'start', 'fix' }, repo.root)
    support.cd(repo.root)
    vim.cmd('OrcaTerm sidebar')
    assert.is_false(shows_unseen('fix'))

    support.play_hooks(repo, support.worktree_path(repo, 'fix'), 'p-hello-2.1.300.log')
    sidebar.refresh()

    assert.is_true(support.listed(repo, 'fix').unseen)
    assert.is_true(shows_unseen('fix'))
    assert.truthy(line_with('NEEDS YOU 1'))

    -- `u` on it now clears it: the person has seen the turn.
    press_u_on('fix')
    support.wait_for(function()
      return not shows_unseen('fix')
    end, 'the sidebar to show fix seen')
    assert.is_false(support.listed(repo, 'fix').unseen)
  end)

  it('does nothing on a line that is no worktree', function()
    local repo = sidebar_with({ 'fix' })
    local ui = support.stub_ui({})
    local before = lines()

    for _, line in ipairs({ 1, 2, assert(line_with('QUIET 1')) }) do
      cursor_on(line)
      vim.cmd('normal u')
    end
    vim.wait(500)

    assert.is_false(support.listed(repo, 'fix').unseen)
    assert.are.same(before, lines())
    assert.are.same({}, ui.notes)
  end)

  it("tells the core's refusal, on a broken worktree", function()
    local repo = sidebar_with({ 'fix' })
    vim.fn.delete(support.worktree_path(repo, 'fix'), 'rf')
    sidebar.refresh()
    local ui = support.stub_ui({})

    press_u_on('fix')
    support.wait_for(function()
      return #ui.notes > 0
    end, 'the refusal')

    assert.are.equal(vim.log.levels.ERROR, ui.notes[1].level)
    assert.truthy(ui.notes[1].message:find('^orca%-term: worktree fix at .* is broken'))
    assert.is_false(shows_unseen('fix'))
  end)

  it('survives a restart of Neovim', function()
    support.load_client()
    local repo = support.repo()
    support.run({ support.binary, 'worktree', 'new', 'fix' }, repo.root)

    local first = start_child(repo)
    assert.is_false(child_shows_unseen(first, 'fix'))
    assert.is_true(vim.rpcrequest(first, 'nvim_exec_lua', CHILD_PRESS_U, { 'fix' }))
    support.wait_for(function()
      return child_shows_unseen(first, 'fix')
    end, 'the first child to show fix unseen')
    quit(first)

    local second = start_child(repo)
    assert.is_true(child_shows_unseen(second, 'fix'))
    assert.is_true(support.listed(repo, 'fix').unseen)
  end)
end)
