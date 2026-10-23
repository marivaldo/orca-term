-- Seam 2: the agent screen, in headless Neovim against the real binary and the fake claude of
-- `support.agent_repo()`. `<CR>` on a worktree with an agent goes to its tab and shows the agent
-- there, in a terminal attached to its holder; wiping that terminal, or quitting another Neovim
-- holding one, only detaches, and `<CR>` attaches again with the scrollback. A worktree that is not
-- ready, or with no sandbox, is refused with the core's reason. `a` asks which agent to start.
-- `vim.ui` and `vim.notify` are stubbed; the core never is. What needs Neovim's main loop running
-- as a person's does (terminal mode, typed keys, quitting) is driven in a child Neovim over RPC.

local support = require('support')
local sidebar = require('orca-term.sidebar')
local terminal = require('orca-term.terminal')

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

---The tab-local directory of the current tab.
---@return string
local function tab_directory()
  return vim.fn.getcwd(-1, vim.fn.tabpagenr())
end

---Whether the terminal `buf` shows `text` anywhere on its screen or in its scrollback.
---@param buf integer
---@param text string
---@return boolean
local function shows(buf, text)
  for _, line in ipairs(vim.api.nvim_buf_get_lines(buf, 0, -1, false)) do
    if line:find(text, 1, true) then
      return true
    end
  end
  return false
end

---Waits until the terminal `buf` shows `text`.
---@param buf integer
---@param text string
local function wait_for_screen(buf, text)
  support.wait_for(function()
    return vim.api.nvim_buf_is_valid(buf) and shows(buf, text)
  end, '`' .. text .. '` on the terminal')
end

---Waits until the agent terminal of the worktree at `path` is attached, and returns it.
---@param path string
---@return integer buf
---@return integer job
local function wait_for_terminal(path)
  support.wait_for(function()
    local buf = terminal.find(path)
    return buf ~= nil and terminal.running_job(buf) ~= nil
  end, 'the agent terminal of ' .. path)
  local buf = assert(terminal.find(path))
  return buf, assert(terminal.running_job(buf))
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

---Every agent terminal, running or not.
---@return integer[]
local function agent_terminals()
  return vim.tbl_filter(function(buf)
    return vim.b[buf][terminal.MARK] ~= nil
  end, vim.api.nvim_list_bufs())
end

---Records, until the spec ends, the worktree name and agent of every `core.worktree_new` call and
---the path of every `core.agent_start` call, still running each one: the core is never stubbed.
---@return { worktree_new: string[][], agent_start: string[] }
local function record_core_calls()
  local core = require('orca-term.core')
  local worktree_new, agent_start = core.worktree_new, core.agent_start
  local calls = { worktree_new = {}, agent_start = {} }
  ---@diagnostic disable-next-line: duplicate-set-field
  core.worktree_new = function(name, agent, ...)
    table.insert(calls.worktree_new, { name, agent })
    return worktree_new(name, agent, ...)
  end
  ---@diagnostic disable-next-line: duplicate-set-field
  core.agent_start = function(path, ...)
    table.insert(calls.agent_start, path)
    return agent_start(path, ...)
  end
  finally(function()
    core.worktree_new, core.agent_start = worktree_new, agent_start
  end)
  return calls
end

---A repository whose agents are the fake claude, with worktrees named `names` made by the core;
---those in `with_agent` get their agent started by the core. Neovim is in its primary checkout,
---the sidebar open.
---@param names string[]
---@param with_agent? string[]
---@return spec.AgentRepo
local function repo_with(names, with_agent)
  support.load_client()
  local repo = support.agent_repo()
  for _, name in ipairs(names) do
    support.run({ support.binary, 'worktree', 'new', name }, repo.root)
  end
  for _, name in ipairs(with_agent or {}) do
    support.run({ support.binary, 'agent', 'start', name }, repo.root)
  end
  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  return repo
end

---The main window of the current tab: the one window that is not the sidebar.
---@return integer
local function main_window()
  local others = vim.tbl_filter(function(win)
    return vim.api.nvim_win_get_buf(win) ~= sidebar.buffer()
  end, vim.api.nvim_tabpage_list_wins(0))
  assert.are.equal(1, #others, 'one window besides the sidebar')
  return others[1]
end

---Lua run in a child Neovim: the current buffer's 'buftype' and the current tab's directory.
local CURRENT_BUFFER_INFO = [[
  return { buftype = vim.bo.buftype, cwd = vim.fn.getcwd(-1, vim.fn.tabpagenr()) }
]]

---The child Neovims the spec started, stopped after it.
---@type integer[]
local children = {}

---A child Neovim, embedded and driven over RPC, with only this client loaded, in the primary
---checkout of `repo` with its sidebar open. Unlike the spec's own Neovim, its main loop runs as a
---person's does, so keys typed into it behave as typed and `startinsert` takes effect. It inherits
---the fake claude's PATH, so it is started by its full path.
---@param repo spec.Repo
---@return integer channel
local function start_child(repo)
  local child = vim.fn.jobstart({
    vim.v.progpath,
    '--embed',
    '--headless',
    '--clean',
    '--cmd',
    'set runtimepath^=' .. vim.fn.fnameescape(support.root),
  }, { rpc = true, cwd = repo.root })
  assert(child > 0, 'the child Neovim did not start')
  table.insert(children, child)
  vim.rpcrequest(child, 'nvim_exec_lua', 'vim.o.columns = 160', {})
  local setup = "require('orca-term').setup({ binary = ... }); vim.cmd('OrcaTerm sidebar')"
  vim.rpcrequest(child, 'nvim_exec_lua', setup, { support.binary })
  return child
end

---Lua run in a child Neovim: puts the cursor on the sidebar line holding `...`.
local CURSOR_ON_SIDEBAR_LINE = [[
  local text = ...
  local sidebar = require('orca-term.sidebar')
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_buf(win) == sidebar.buffer() then
      vim.api.nvim_set_current_win(win)
      for index, line in ipairs(vim.api.nvim_buf_get_lines(0, 0, -1, false)) do
        if line:find(text, 1, true) then
          vim.api.nvim_win_set_cursor(win, { index, 0 })
          return true
        end
      end
    end
  end
  return false
]]

---Types `key` in the sidebar of the child Neovim, on the line holding `text`.
---@param child integer
---@param key string
---@param text string
local function press_in_child(child, key, text)
  local found = vim.rpcrequest(child, 'nvim_exec_lua', CURSOR_ON_SIDEBAR_LINE, { text })
  assert(found, 'no sidebar line holding ' .. text .. ' in the child')
  vim.rpcrequest(child, 'nvim_input', key)
end

---Lua run in a child Neovim: whether the current buffer holds the text `...` on some line.
local CURRENT_BUFFER_HAS = [[
  local text = ...
  for _, line in ipairs(vim.api.nvim_buf_get_lines(0, 0, -1, false)) do
    if line:find(text, 1, true) then
      return true
    end
  end
  return false
]]

---Whether the current buffer of the child Neovim shows `text`.
---@param child integer
---@param text string
---@return boolean
local function child_shows(child, text)
  return vim.rpcrequest(child, 'nvim_exec_lua', CURRENT_BUFFER_HAS, { text }) == true
end

---The mode of the child Neovim, such as `n` or `t`.
---@param child integer
---@return string
local function child_mode(child)
  return vim.rpcrequest(child, 'nvim_get_mode').mode --[[@as string]]
end

describe('the agent screen', function()
  local columns = vim.o.columns

  before_each(function()
    vim.o.columns = 160
  end)

  after_each(function()
    for _, child in ipairs(children) do
      vim.fn.jobstop(child)
    end
    children = {}
    support.reset_editor()
    support.cleanup()
    vim.o.columns = columns
  end)

  it('<CR> on a worktree with an agent opens its attached terminal, filling the tab', function()
    local repo = repo_with({ 'alpha' }, { 'alpha' })
    local alpha = support.worktree_path(repo, 'alpha')

    press_on('<CR>', '○ alpha')
    local buf, job = wait_for_terminal(alpha)

    assert.are.equal(alpha, tab_directory())
    assert.are.equal(main_window(), vim.api.nvim_get_current_win())
    assert.are.equal(buf, vim.api.nvim_get_current_buf())
    -- Terminal mode is checked in a child Neovim, below: `startinsert` takes effect once Neovim is
    -- back in its main loop, which it never is while a spec runs.
    local argv = vim.api.nvim_get_chan_info(job).argv
    assert.are.same(
      { 'agent', 'attach', alpha, '--no-detach-key' },
      vim.list_slice(argv, #argv - 3)
    )
    wait_for_screen(buf, 'fake claude ready')
    vim.api.nvim_chan_send(job, 'hello\r')
    wait_for_screen(buf, 'you said: hello')
  end)

  it('<CR> again reuses the live terminal, attaching only once', function()
    local repo = repo_with({ 'alpha' }, { 'alpha' })
    local alpha = support.worktree_path(repo, 'alpha')
    press_on('<CR>', '○ alpha')
    local buf, job = wait_for_terminal(alpha)
    -- Shown elsewhere meanwhile: the terminal stays attached, hidden.
    vim.cmd('enew')

    press_on('<CR>', '○ alpha')
    support.wait_for(function()
      return vim.api.nvim_get_current_buf() == buf
    end, 'the terminal to show again')

    assert.are.equal(job, terminal.running_job(buf))
    assert.are.same({ buf }, agent_terminals())
    assert.are.equal(2, #vim.api.nvim_list_tabpages())
  end)

  it('<CR> on a worktree with no agent only goes to its tab', function()
    local repo = repo_with({ 'beta' })
    local ui = support.stub_ui({})

    press_on('<CR>', '· beta')
    vim.wait(300)

    assert.are.equal(support.worktree_path(repo, 'beta'), tab_directory())
    assert.are.same({}, agent_terminals())
    assert.are.same({}, ui.notes)
    assert.is_false(support.listed(repo, 'beta').holder)
  end)

  it('wiping the terminal detaches without stopping the agent; <CR> reattaches', function()
    local repo = repo_with({ 'alpha' }, { 'alpha' })
    local alpha = support.worktree_path(repo, 'alpha')
    press_on('<CR>', '○ alpha')
    local buf, job = wait_for_terminal(alpha)
    wait_for_screen(buf, 'fake claude ready')
    vim.api.nvim_chan_send(job, 'before detaching\r')
    wait_for_screen(buf, 'you said: before detaching')

    vim.cmd('bwipeout! ' .. buf)
    support.wait_for(function()
      return vim.fn.jobwait({ job }, 0)[1] ~= -1
    end, 'the attach to end')
    -- The tab held only the terminal besides the sidebar, so the sidebar closes it.
    support.wait_for(function()
      return #vim.api.nvim_list_tabpages() == 1
    end, "the worktree's tab to close")

    assert.is_true(support.listed(repo, 'alpha').holder)
    press_on('<CR>', '○ alpha')
    local again = wait_for_terminal(alpha)
    assert.are_not.equal(buf, again)
    wait_for_screen(again, 'you said: before detaching')
  end)

  it('another Neovim gets the terminal in terminal mode, typing into the agent', function()
    local repo = repo_with({ 'alpha' }, { 'alpha' })
    local child = start_child(repo)

    press_in_child(child, '<CR>', '○ alpha')
    support.wait_for(function()
      return child_mode(child) == 't'
    end, 'the child to be in terminal mode')
    support.wait_for(function()
      return child_shows(child, 'fake claude ready')
    end, 'the child to show the agent')
    vim.rpcrequest(child, 'nvim_input', 'typed in the child<CR>')

    support.wait_for(function()
      return child_shows(child, 'you said: typed in the child')
    end, 'the agent to echo what the child typed')
    local current = vim.rpcrequest(child, 'nvim_exec_lua', CURRENT_BUFFER_INFO, {})
    assert.are.same({ buftype = 'terminal', cwd = support.worktree_path(repo, 'alpha') }, current)
  end)

  it(':wqa in another Neovim holding the terminal quits, and the agent survives it', function()
    local repo = repo_with({ 'alpha' }, { 'alpha' })
    local alpha = support.worktree_path(repo, 'alpha')
    local child = start_child(repo)
    press_in_child(child, '<CR>', '○ alpha')
    support.wait_for(function()
      return child_shows(child, 'fake claude ready')
    end, 'the child to show the agent')
    vim.rpcrequest(child, 'nvim_input', 'from the child<CR>')
    support.wait_for(function()
      return child_shows(child, 'you said: from the child')
    end, 'the agent to echo what the child typed')

    -- Neovim refuses `:wqa` (E948) while a terminal's job runs, unless the client detaches its
    -- terminals first. `:qa` is never refused. The child must exit on its own, and succeed.
    local quit, err = pcall(vim.rpcrequest, child, 'nvim_command', 'wqa')
    assert(not quit, 'the child is still running after :wqa')
    assert(tostring(err):find('closed', 1, true), tostring(err))
    assert.are.same({ 0 }, vim.fn.jobwait({ child }, 10000))

    assert.is_true(support.listed(repo, 'alpha').holder)
    press_on('<CR>', '○ alpha')
    local buf = wait_for_terminal(alpha)
    wait_for_screen(buf, 'you said: from the child')
  end)

  it("<CR> on a worktree that is not ready refuses with the core's reason", function()
    local repo = repo_with({ 'alpha' }, { 'alpha' })
    support.run({ support.binary, 'agent', 'stop', 'alpha' }, repo.root)
    support.set_setup(repo, { 'echo boom', 'exit 3' })
    support.core(repo, { 'worktree', 'setup', 'alpha' })
    local listed = support.listed(repo, 'alpha')
    assert.are.same({ 'not_ready', 'claude-code' }, { listed.state, listed.agent })
    vim.cmd('doautocmd FocusGained')
    local ui = support.stub_ui({})

    press_on('<CR>', '△ alpha')

    local note = next_note(ui)
    assert.are.equal(vim.log.levels.ERROR, note.level)
    assert(note.message:find('cannot start an agent in worktree alpha', 1, true), note.message)
    assert(note.message:find('it is not ready', 1, true), note.message)
    assert.are.equal(support.worktree_path(repo, 'alpha'), tab_directory())
    assert.are.same({}, agent_terminals())
    assert.is_false(support.listed(repo, 'alpha').holder)
  end)

  it("<CR> with no sandbox tools refuses with the core's reason", function()
    local repo = repo_with({ 'alpha' }, { 'alpha' })
    support.run({ support.binary, 'agent', 'stop', 'alpha' }, repo.root)
    support.remove_sandbox(repo)
    vim.cmd('doautocmd FocusGained')
    local ui = support.stub_ui({})

    press_on('<CR>', '○ alpha')

    local note = next_note(ui)
    assert.are.equal(vim.log.levels.ERROR, note.level)
    assert(note.message:find("claude's sandbox is not available", 1, true), note.message)
    assert.are.same({}, agent_terminals())
    assert.is_false(support.listed(repo, 'alpha').holder)
  end)

  describe('a', function()
    it('asks for the agent after the name; claude-code lands in its attached terminal', function()
      local repo = repo_with({})
      local ui = support.stub_ui({ input = 'gamma', select = { 'claude-code' } })
      local calls = record_core_calls()

      press_on('a', 'no worktrees yet')

      assert.are.same({ 'New worktree name: ', 'Agent for gamma' }, ui.prompts)
      assert.are.same({ { 'claude-code', 'opencode', 'No agent' } }, ui.shown)
      assert.are.equal('orca-term: created worktree gamma', next_note(ui).message)
      local gamma = support.worktree_path(repo, 'gamma')
      local buf = wait_for_terminal(gamma)
      assert.are.equal(gamma, tab_directory())
      assert.are.equal(buf, vim.api.nvim_get_current_buf())
      wait_for_screen(buf, 'fake claude ready')
      assert.is_true(support.listed(repo, 'gamma').holder)
      assert.are.equal('claude-code', support.listed(repo, 'gamma').agent)
      assert.are.equal(1, #ui.notes)
      -- One call: `worktree new --agent claude-code` started the agent; no `agent start` followed.
      assert.are.same({ { 'gamma', 'claude-code' } }, calls.worktree_new)
      assert.are.same({}, calls.agent_start)
    end)

    it('a start the core refuses lands in the tab, records the agent and tells why', function()
      local repo = repo_with({})
      support.remove_sandbox(repo)
      local ui = support.stub_ui({ input = 'gamma', select = { 'claude-code' } })

      press_on('a', 'no worktrees yet')

      -- Both notes come in the same turn of the event loop.
      support.wait_for(function()
        return #ui.notes >= 2
      end, 'two notifications')
      assert.are.equal('orca-term: created worktree gamma', ui.notes[1].message)
      local note = ui.notes[2]
      assert.are.equal(vim.log.levels.ERROR, note.level)
      assert(note.message:find("claude's sandbox is not available", 1, true), note.message)
      assert.are.equal(support.worktree_path(repo, 'gamma'), tab_directory())
      assert.are.same({}, agent_terminals())
      local listed = support.listed(repo, 'gamma')
      assert.are.same({ 'claude-code', false }, { listed.agent, listed.holder })
    end)

    it('No agent lands in the tab with no terminal', function()
      local repo = repo_with({})
      local ui = support.stub_ui({ input = 'gamma', select = { 'No agent' } })

      press_on('a', 'no worktrees yet')

      assert.are.equal('orca-term: created worktree gamma', next_note(ui).message)
      assert.are.equal(support.worktree_path(repo, 'gamma'), tab_directory())
      vim.wait(300)
      assert.are.same({}, agent_terminals())
      local listed = support.listed(repo, 'gamma')
      assert.are.same({ 'no_agent', false }, { listed.state, listed.holder })
    end)

    it('cancelling the agent question creates nothing', function()
      local repo = repo_with({})
      local ui = support.stub_ui({ input = 'gamma' })

      press_on('a', 'no worktrees yet')
      vim.wait(300)

      assert.are.same({ 'New worktree name: ', 'Agent for gamma' }, ui.prompts)
      assert.is_false((pcall(support.listed, repo, 'gamma')))
      assert.are.equal(1, #vim.api.nvim_list_tabpages())
      assert.are.same({}, ui.notes)
    end)

    it('a failed setup with claude-code warns and starts no agent', function()
      local repo = repo_with({})
      support.set_setup(repo, { 'echo boom', 'exit 3' })
      local ui = support.stub_ui({ input = 'broken', select = { 'claude-code' } })

      press_on('a', 'no worktrees yet')

      local note = next_note(ui)
      assert.are.equal(vim.log.levels.WARN, note.level)
      assert.are.equal(
        'orca-term: created worktree broken, but its setup failed: setup failed (exit 3): boom; '
          .. 'its agent was not started',
        note.message
      )
      assert.are.equal(support.worktree_path(repo, 'broken'), tab_directory())
      vim.wait(300)
      assert.are.same({}, agent_terminals())
      local listed = support.listed(repo, 'broken')
      assert.are.same({ 'not_ready', false }, { listed.state, listed.holder })
    end)
  end)
end)
