-- Seam 2: approving a committed `sandbox.writable` list from the sidebar (#58), in headless Neovim
-- against the real binary and the fake claude of `support.agent_repo()`. When `<CR>` starts an
-- agent and the core refuses as `untrusted` (`what` `sandbox`), the paths and the file they come
-- from are shown, and the agent starts only on `Allow` (`agent start --approve-sandbox`);
-- `Cancel` starts nothing. `vim.ui` and `vim.notify` are stubbed; the core never is.

local support = require('support')
local sidebar = require('orca-term.sidebar')
local terminal = require('orca-term.terminal')

---The committed list of these specs, as written.
local PATHS = { '~/cache-committed', '../shared' }

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

---Waits until the agent terminal of the worktree at `path` is attached.
---@param path string
local function wait_for_terminal(path)
  support.wait_for(function()
    local buf = terminal.find(path)
    return buf ~= nil and terminal.running_job(buf) ~= nil
  end, 'the agent terminal of ' .. path)
end

---Writes a config file at `path` whose `sandbox.writable` is `paths`.
---@param path string
---@param paths string[]
local function write_writable(path, paths)
  local yaml = { 'sandbox:', '  writable:' }
  for _, item in ipairs(paths) do
    table.insert(yaml, '    - ' .. item)
  end
  vim.fn.writefile(yaml, path)
end

---A repository whose agents are the fake claude and whose committed `orca-term.yaml` holds
---`PATHS`, with the worktree `alpha`, whose agent the core started once (under an empty override
---list, which needs no approval) and stopped. Neovim is in the primary checkout, the sidebar open.
---@return spec.AgentRepo repo
---@return string alpha the worktree's path
local function stopped_alpha()
  support.load_client()
  local repo = support.agent_repo()
  write_writable(repo.root .. '/orca-term.yaml', PATHS)
  support.git(repo, { 'add', 'orca-term.yaml' })
  support.git(repo, { 'commit', '--quiet', '-m', 'sandbox config' })
  local override = repo.root .. '/.git/orca-term.yaml'
  vim.fn.writefile({ 'sandbox:', '  writable: []' }, override)
  support.run({ support.binary, 'worktree', 'new', 'alpha' }, repo.root)
  support.run({ support.binary, 'agent', 'start', 'alpha' }, repo.root)
  support.run({ support.binary, 'agent', 'stop', 'alpha' }, repo.root)
  vim.fn.delete(override)
  vim.fn.delete(repo.log .. '.argv')
  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  return repo, support.worktree_path(repo, 'alpha')
end

---The question shown for the committed list of the worktree at `alpha`.
---@param alpha string
---@return string
local function question(alpha)
  return table.concat({
    ('Widen the sandbox of alpha with these writable paths from %s/orca-term.yaml?'):format(alpha),
    'They are committed to the repository and not approved yet:',
    '',
    '    ~/cache-committed',
    '    ../shared',
  }, '\n')
end

describe('a committed writable list', function()
  local columns = vim.o.columns

  before_each(function()
    vim.o.columns = 160
  end)

  after_each(function()
    support.reset_editor()
    support.cleanup()
    vim.o.columns = columns
  end)

  it('is shown when <CR> starts the agent, which starts once allowed', function()
    local repo, alpha = stopped_alpha()
    local ui = support.stub_ui({ select = { 'Allow' } })

    open_in_sidebar('alpha')

    wait_for_terminal(alpha)
    assert.are.same({ question(alpha) }, ui.prompts)
    assert.are.same({ { 'Allow', 'Cancel' } }, ui.shown)
    assert.is_true(support.listed(repo, 'alpha').holder)
    local trust = table.concat(vim.fn.readfile(repo.root .. '/.git/orca-term-trust.json'), '')
    assert(trust:find('"kind":"sandbox"', 1, true), trust)
    assert.are.same({}, ui.notes)
  end)

  it('cancelled, starts nothing', function()
    local repo = stopped_alpha()
    local ui = support.stub_ui({ select = { 'Cancel' } })

    open_in_sidebar('alpha')

    support.wait_for(function()
      return #ui.notes > 0
    end, 'a notification')
    assert.are.same({
      message = 'orca-term: the writable paths of alpha were not approved; its agent was not started',
      level = vim.log.levels.WARN,
    }, ui.notes[1])
    assert.is_false(support.listed(repo, 'alpha').holder)
    assert.is_nil(support.claude_argv(repo))
    assert.is_nil(vim.uv.fs_stat(repo.root .. '/.git/orca-term-trust.json'))
  end)
end)
