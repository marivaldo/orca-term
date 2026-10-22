-- Seam 2: swapping a worktree's agent from the sidebar (#35), in headless Neovim against the real
-- binary, the fake claude of `support.agent_repo()` and a fake opencode, with fake sandbox tools
-- that run what they wrap. `.` then `Swap agent` offers the other agent, swaps through the core
-- (`agent swap`) and shows the new agent in the worktree's tab, in a terminal attached again; a
-- swap the core refuses (an agent busy in a turn) is told with its reason and changes nothing.
-- `a` with `opencode` creates a worktree whose opencode the core starts (#93).
-- `vim.ui` and `vim.notify` are stubbed; the core never is.

local support = require('support')
local sidebar = require('orca-term.sidebar')
local terminal = require('orca-term.terminal')

---The fake `opencode`: answers `--version`, says it is ready, echoes every line typed, and ends on
---`SIGHUP`. Bash builtins only: its PATH holds nothing else.
local FAKE_OPENCODE = {
  '#!/bin/bash',
  'if [ "$1" = "--version" ]; then echo "1.18.0"; exit 0; fi',
  "trap 'exit 129' HUP",
  'echo "fake opencode ready"',
  'while IFS= read -r -t 120 line; do echo "you said: $line"; done',
  'exit 0',
}

---The fake `sandbox-exec`, which runs what follows `-f <profile>`.
local FAKE_SANDBOX_EXEC = { '#!/bin/bash', 'shift 2', 'exec "$@"' }

---The fake `bwrap`, which runs what follows `--`.
local FAKE_BWRAP = {
  '#!/bin/bash',
  'while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do shift; done',
  'shift',
  'exec "$@"',
}

---Writes the executable `name` on the PATH of `repo`, with `lines`.
---@param repo spec.AgentRepo
---@param name string
---@param lines string[]
local function write_tool(repo, name, lines)
  local path = repo.bin .. '/' .. name
  vim.fn.delete(path)
  vim.fn.writefile(lines, path)
  assert(vim.uv.fs_chmod(path, tonumber('755', 8)))
end

---Presses `key` in the sidebar of the current tab, on the line of the worktree `name`.
---@param key string
---@param name string
local function press_on(key, name)
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_buf(win) == sidebar.buffer() then
      vim.api.nvim_set_current_win(win)
      for index, line in ipairs(vim.api.nvim_buf_get_lines(0, 0, -1, false)) do
        if line:find(' ' .. name, 1, true) then
          vim.api.nvim_win_set_cursor(win, { index, 0 })
          vim.cmd('normal ' .. vim.keycode(key))
          return
        end
      end
      error('no sidebar line for ' .. name)
    end
  end
  error('no sidebar in the current tab')
end

---Waits until the attached agent terminal of the worktree at `path` shows `text`, and returns it.
---@param path string
---@param text string
---@return integer buf
local function wait_for_terminal_showing(path, text)
  support.wait_for(function()
    local buf = terminal.find(path)
    if not buf or not terminal.running_job(buf) then
      return false
    end
    for _, line in ipairs(vim.api.nvim_buf_get_lines(buf, 0, -1, false)) do
      if line:find(text, 1, true) then
        return true
      end
    end
    return false
  end, '`' .. text .. '` on the agent terminal of ' .. path)
  return assert(terminal.find(path))
end

---A repository whose agents are the fake claude and the fake opencode, with the worktree `demo`,
---whose claude the core started. Neovim is in the primary checkout, the sidebar open.
---@return spec.AgentRepo repo
---@return string demo the worktree's path
local function claude_on_demo()
  support.load_client()
  local repo = support.agent_repo()
  write_tool(repo, 'opencode', FAKE_OPENCODE)
  write_tool(repo, 'sandbox-exec', FAKE_SANDBOX_EXEC)
  write_tool(repo, 'bwrap', FAKE_BWRAP)
  support.run({ support.binary, 'worktree', 'new', 'demo' }, repo.root)
  support.run({ support.binary, 'agent', 'start', 'demo' }, repo.root)
  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  return repo, support.worktree_path(repo, 'demo')
end

describe('Swap agent', function()
  local columns = vim.o.columns

  before_each(function()
    vim.o.columns = 160
  end)

  after_each(function()
    support.reset_editor()
    support.cleanup()
    vim.o.columns = columns
  end)

  it('offers the other agent, swaps through the core and attaches the terminal to it', function()
    local repo, demo = claude_on_demo()
    support.stub_ui({})
    press_on('<CR>', 'demo')
    local claudes = wait_for_terminal_showing(demo, 'fake claude ready')

    local ui = support.stub_ui({ select = { 'Swap agent', 'opencode' } })
    press_on('.', 'demo')

    local opencodes = wait_for_terminal_showing(demo, 'fake opencode ready')
    assert.are.same({
      { 'Re-run setup', 'Copy path', 'Swap agent', 'New session', 'Remove worktree' },
      { 'opencode' },
    }, ui.shown)
    assert.are.equal('Swap the agent of demo for', ui.prompts[2])
    assert.are_not.equal(claudes, opencodes)
    assert(not vim.api.nvim_buf_is_valid(claudes), "claude's terminal is gone")
    assert.are.equal(opencodes, vim.api.nvim_get_current_buf(), 'shown in the current window')
    assert.are.equal(demo, vim.fn.getcwd(-1, vim.fn.tabpagenr()), "in the worktree's tab")
    local listed = support.listed(repo, 'demo')
    assert.are.equal('opencode', listed.agent)
    assert.is_true(listed.holder)
    assert.are.same({}, ui.notes)
  end)

  it('tells why the core refuses an agent busy in a turn, and swaps nothing', function()
    local repo, demo = claude_on_demo()
    support.play_hooks(repo, demo, 'tui-queuetool-2.1.300.log', 1, 3)
    local ui = support.stub_ui({ select = { 'Swap agent', 'opencode' } })

    press_on('.', 'demo')

    support.wait_for(function()
      return #ui.notes > 0
    end, 'a notification')
    assert.are.equal(vim.log.levels.ERROR, ui.notes[1].level)
    assert(ui.notes[1].message:find('its agent is in a turn', 1, true), ui.notes[1].message)
    local listed = support.listed(repo, 'demo')
    assert.are.equal('claude-code', listed.agent)
    assert.are.equal('working', listed.state)
  end)
end)

describe('a with opencode', function()
  after_each(function()
    support.reset_editor()
    support.cleanup()
  end)

  it('creates the worktree with opencode started and lands in its attached terminal', function()
    local repo = claude_on_demo()
    local ui = support.stub_ui({ input = 'gamma', select = { 'opencode' } })

    press_on('a', 'demo')

    support.wait_for(function()
      return #ui.notes > 0
    end, 'a notification')
    local gamma = support.worktree_path(repo, 'gamma')
    local buf = wait_for_terminal_showing(gamma, 'fake opencode ready')
    assert.are.same({ { 'claude-code', 'opencode', 'No agent' } }, ui.shown)
    assert.are.equal(buf, vim.api.nvim_get_current_buf(), 'shown in the current window')
    assert.are.equal(gamma, vim.fn.getcwd(-1, vim.fn.tabpagenr()), "in the worktree's tab")
    assert.are.equal('orca-term: created worktree gamma', ui.notes[1].message)
    local listed = support.listed(repo, 'gamma')
    assert.are.same({ 'opencode', true }, { listed.agent, listed.holder })
  end)
end)
