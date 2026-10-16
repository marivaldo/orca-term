-- Seam 2, golden: the sidebar as Neovim shows it, for a real repository holding a worktree in
-- every state the core reports today, read by the real binary. The agents are the fake claude of
-- `support.agent_repo()`, and their states come from recordings of claude's hooks played through
-- the handlers their settings register (`support.play_hooks`). A turn that ends leaves its worktree
-- unseen; the core's `worktree seen` and `worktree unseen` clear and set the mark of others, so
-- unseen and seen worktrees show side by side. The buffer's text and its highlights are compared
-- with spec/golden/sidebar.txt.
--
-- The golden file holds the buffer's lines, then one highlight per line as
-- `<line>:<first>-<last> <group>` (1-based line, 0-based byte columns, the last one excluded).
-- After a deliberate change to the sidebar's look, rewrite it and review the diff:
--
--     UPDATE_GOLDEN=1 just lua-test
--
-- The repository's directory is named `repo` and no path is shown, so the file is the same on
-- every machine.

local support = require('support')
local sidebar = require('orca-term.sidebar')

local GOLDEN = support.root .. '/spec/golden/sidebar.txt'

---Waits up to 30 seconds for `path` to exist; fails the spec if it never does.
---@param path string
local function wait_for(path)
  local appeared = vim.wait(30000, function()
    return vim.uv.fs_stat(path) ~= nil
  end, 50)
  assert(appeared, path .. ' never appeared')
end

---A setup script that says it started by creating `<dir>/started`, then waits for
---`<dir>/release` for at most 30 seconds, so a failing spec cannot hang.
---@param dir string
---@return string[]
local function blocking_script(dir)
  return {
    'touch ' .. dir .. '/started',
    'i=0',
    'while [ ! -e ' .. dir .. '/release ]; do',
    '  i=$((i + 1))',
    '  if [ "$i" -gt 600 ]; then exit 1; fi',
    '  sleep 0.05',
    'done',
  }
end

---The sidebar buffer's text and highlights, in the golden file's format.
---@return string[]
local function snapshot()
  local buf = assert(sidebar.buffer())
  local out = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  table.insert(out, '')
  table.insert(out, '# highlights: <line>:<first>-<last> <group>')
  local marks = vim.api.nvim_buf_get_extmarks(buf, sidebar.NAMESPACE, 0, -1, { details = true })
  for _, mark in ipairs(marks) do
    local row, col, details = mark[2], mark[3], mark[4] or {}
    table.insert(out, ('%d:%d-%d %s'):format(row + 1, col, details.end_col, details.hl_group))
  end
  return out
end

describe('the sidebar, golden', function()
  after_each(function()
    support.reset_editor()
    support.cleanup()
  end)

  it('renders every state the core reports today, unseen and seen', function()
    support.load_client()
    local repo = support.agent_repo()
    -- The setup scripts below need `touch` and `sleep`, which the fake claude's PATH leaves out;
    -- its own directory stays first, so the fake claude is the one found.
    support.set_env('PATH', repo.bin .. ':/usr/bin:/bin')

    -- no_agent: plain `worktree new`. `spike` then moves to a branch of another name.
    for _, name in ipairs({ 'install-deps', 'docs', 'old-spike', 'spike' }) do
      support.run({ support.binary, 'worktree', 'new', name }, repo.root)
    end

    -- The agent states: an agent started by the core, then the hooks of a recording, played up to
    -- the hook that leaves the worktree in that state. A turn that ended leaves it unseen.
    local agents = {
      { name = 'run-tests', fixture = 'tui-queuetool-2.1.300.log', last = 3 }, -- working
      { name = 'pick-colour', fixture = 'tui-ask-2.1.300.log', last = 4 }, -- waiting
      { name = 'greet', fixture = 'p-hello-2.1.300.log' }, -- finished, unseen
      { name = 'api-call', fixture = 'p-stopfailure-2.1.300.log' }, -- failed, unseen
      { name = 'slow-job', fixture = 'tui-kill-2.1.300.log' }, -- interrupted, unseen
      { name = 'reviewed', fixture = 'p-hello-2.1.300.log' }, -- finished, then seen below
      { name = 'standby' }, -- idle: no turn yet
      { name = 'nudged' }, -- idle, then marked unseen below
    }
    for _, agent in ipairs(agents) do
      support.run({ support.binary, 'worktree', 'new', agent.name }, repo.root)
      support.run({ support.binary, 'agent', 'start', agent.name }, repo.root)
      if agent.fixture then
        local path = support.worktree_path(repo, agent.name)
        support.play_hooks(repo, path, agent.fixture, 1, agent.last)
      end
    end
    support.run({ support.binary, 'worktree', 'seen', 'reviewed' }, repo.root)
    support.run({ support.binary, 'worktree', 'unseen', 'nudged' }, repo.root)
    support.run(
      { 'git', 'switch', '--quiet', '-c', 'other-branch' },
      support.worktree_path(repo, 'spike')
    )

    -- not_ready: a setup that fails.
    support.set_setup(repo, { 'echo "npm ERR! missing script: build"', 'exit 1' })
    local failed = support.core(repo, { 'worktree', 'new', 'fix-login' })
    assert.are.equal(1, failed.code, failed.stderr)

    -- broken: the worktree's directory is gone.
    vim.fn.delete(support.worktree_path(repo, 'old-spike'), 'rf')

    -- setting_up: a setup that waits for a file, run in the background.
    local signals = support.temp_dir()
    support.set_setup(repo, blocking_script(signals))
    local running = vim.system(
      { support.binary, 'worktree', 'setup', 'install-deps' },
      { cwd = repo.root, text = true }
    )
    finally(function()
      vim.fn.writefile({}, signals .. '/release')
      running:wait(30000)
    end)
    wait_for(signals .. '/started')

    support.cd(repo.root)
    vim.cmd('OrcaTerm sidebar')
    local actual = snapshot()

    if os.getenv('UPDATE_GOLDEN') == '1' then
      vim.fn.mkdir(vim.fs.dirname(GOLDEN), 'p')
      vim.fn.writefile(actual, GOLDEN)
    end
    assert(
      vim.uv.fs_stat(GOLDEN),
      'no ' .. GOLDEN .. '; write it with UPDATE_GOLDEN=1 just lua-test'
    )
    assert.are.same(vim.fn.readfile(GOLDEN), actual)
  end)
end)
