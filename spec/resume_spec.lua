-- Seam 2: the cold start, in headless Neovim against the real binary and the fake claude of
-- `support.agent_repo()`. With the agent's holder gone, `<CR>` starts the agent again on the
-- session its worktree recorded (`--resume`), or on a new one when none is recorded, and attaches.
-- A session the core can no longer resume asks whether to start a fresh one. The spec acts as
-- claude does to record a session, playing a recording of claude's hooks through the handlers the
-- agent's settings register (`support.play_hooks`, as tests/hooks.rs does). `vim.ui` and
-- `vim.notify` are stubbed; the core never is.

local support = require('support')
local sidebar = require('orca-term.sidebar')
local terminal = require('orca-term.terminal')

---The recording of claude's hooks that records a session, and that session's id.
local HELLO = 'p-hello-2.1.300.log'
local SESSION = '40b1fb34-8b28-4e24-9bcf-31292c24b65f'

---Presses `<CR>` in the sidebar of the current tab, on the line of the worktree `name`.
---@param name string
local function open_in_sidebar(name)
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_buf(win) == sidebar.buffer() then
      vim.api.nvim_set_current_win(win)
      local lines = vim.api.nvim_buf_get_lines(0, 0, -1, false)
      for index, line in ipairs(lines) do
        if line:find(' ' .. name, 1, true) then
          vim.api.nvim_win_set_cursor(win, { index, 0 })
          vim.cmd('normal ' .. vim.keycode('<CR>'))
          return
        end
      end
      error('no sidebar line for ' .. name)
    end
  end
  error('no sidebar in the current tab')
end

---Waits until the agent terminal of the worktree at `path` is attached, and returns it.
---@param path string
---@return integer buf
local function wait_for_terminal(path)
  support.wait_for(function()
    local buf = terminal.find(path)
    return buf ~= nil and terminal.running_job(buf) ~= nil
  end, 'the agent terminal of ' .. path)
  return assert(terminal.find(path))
end

---Every agent terminal, running or not.
---@return integer[]
local function agent_terminals()
  return vim.tbl_filter(function(buf)
    return vim.b[buf][terminal.MARK] ~= nil
  end, vim.api.nvim_list_bufs())
end

---Waits until the fake claude of `repo` has started, and returns its arguments.
---@param repo spec.AgentRepo
---@return string[]
local function wait_for_argv(repo)
  support.wait_for(function()
    return support.claude_argv(repo) ~= nil
  end, 'the fake claude to start')
  return assert(support.claude_argv(repo))
end

---The transcript the recording `HELLO` names, in `repo`.
---@param repo spec.AgentRepo
---@return string
local function transcript_of(repo)
  return repo.tmp .. '/transcripts/' .. SESSION .. '.jsonl'
end

---A repository whose agents are the fake claude, with the worktree `alpha` whose agent the core
---started and then stopped, after recording the session of `HELLO` when `record` is true. The
---fake claude's arguments are forgotten, Neovim is in the primary checkout, the sidebar open.
---@param record boolean
---@return spec.AgentRepo repo
---@return string alpha the worktree's path
local function stopped_alpha(record)
  support.load_client()
  local repo = support.agent_repo()
  support.run({ support.binary, 'worktree', 'new', 'alpha' }, repo.root)
  support.run({ support.binary, 'agent', 'start', 'alpha' }, repo.root)
  local alpha = support.worktree_path(repo, 'alpha')
  if record then
    support.play_hooks(repo, alpha, HELLO)
  end
  support.run({ support.binary, 'agent', 'stop', 'alpha' }, repo.root)
  vim.fn.delete(repo.log .. '.argv')
  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  return repo, alpha
end

describe('the cold start', function()
  local columns = vim.o.columns

  before_each(function()
    vim.o.columns = 160
  end)

  after_each(function()
    support.reset_editor()
    support.cleanup()
    vim.o.columns = columns
  end)

  it('<CR> resumes the recorded session and attaches', function()
    local repo, alpha = stopped_alpha(true)
    vim.fn.mkdir(repo.tmp .. '/transcripts', 'p')
    vim.fn.writefile({}, transcript_of(repo))
    local ui = support.stub_ui({})

    open_in_sidebar('alpha')

    wait_for_terminal(alpha)
    local argv = wait_for_argv(repo)
    assert.are.same({ '--resume', SESSION }, vim.list_slice(argv, #argv - 1))
    assert.is_true(support.listed(repo, 'alpha').holder)
    assert.are.same({}, ui.prompts)
    assert.are.same({}, ui.notes)
  end)

  it('<CR> with no recorded session starts a new one and attaches', function()
    local repo, alpha = stopped_alpha(false)
    local ui = support.stub_ui({})

    open_in_sidebar('alpha')

    wait_for_terminal(alpha)
    local argv = wait_for_argv(repo)
    assert(not vim.tbl_contains(argv, '--resume'), table.concat(argv, ' '))
    assert.are.same({}, ui.prompts)
    assert.are.same({}, ui.notes)
  end)

  describe('with the transcript gone', function()
    it('asks with the reason; Start a fresh session attaches a new session', function()
      local repo, alpha = stopped_alpha(true)
      local ui = support.stub_ui({ select = { 'Start a fresh session' } })

      open_in_sidebar('alpha')

      wait_for_terminal(alpha)
      local reason = ('its transcript %s is gone'):format(transcript_of(repo))
      assert.are.same({ 'Cannot resume the session of alpha: ' .. reason }, ui.prompts)
      assert.are.same({ { 'Start a fresh session', 'Cancel' } }, ui.shown)
      local argv = wait_for_argv(repo)
      assert(not vim.tbl_contains(argv, '--resume'), table.concat(argv, ' '))
      assert.is_true(support.listed(repo, 'alpha').holder)
      assert.are.same({}, ui.notes)
    end)

    it('Cancel attaches nothing and starts no agent', function()
      local repo = stopped_alpha(true)
      local ui = support.stub_ui({ select = { 'Cancel' } })

      open_in_sidebar('alpha')

      support.wait_for(function()
        return #ui.prompts > 0
      end, 'the question')
      vim.wait(300)
      assert.are.same({ { 'Start a fresh session', 'Cancel' } }, ui.shown)
      assert.are.same({}, agent_terminals())
      assert.is_false(support.listed(repo, 'alpha').holder)
      assert.is_nil(support.claude_argv(repo))
      assert.are.same({}, ui.notes)
    end)
  end)
end)
