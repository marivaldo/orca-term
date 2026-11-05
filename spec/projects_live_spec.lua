-- Seam 2: every project, live (#156), against the real binary. With projects on the project list,
-- Neovim registers with every project's primary checkout, the status strip counts every project's
-- worktrees, `:OrcaTerm jump` and `:OrcaTerm worktree N` go to any of them, the pull requests
-- refreshed without being asked are asked for in each worktree's own project, and a read runs
-- `orca-term fleets` once, without `worktree ls`.
--
-- The live updates run in child Neovims, as spec/live_spec.lua explains: embedded, driven over
-- RPC, each listening at a short socket path of its own, so the core can tell them. The spec's own
-- Neovim listens on nothing (`support.load_client`): there the sidebar is read when asked. The
-- project list lives in the fixture's XDG_STATE_HOME (`support.isolate()`), which the children
-- inherit, never in the developer's.

local support = require('support')
local pr = require('orca-term.pr')
local sidebar = require('orca-term.sidebar')

---The child Neovims the spec started, with their socket paths, stopped after it.
---@type { channel: integer, socket: string }[]
local children = {}

---How many sockets this spec process has named, to keep each path unique.
local sockets = 0

---A child Neovim listening at a short socket path under /tmp, in the directory `cwd`, with this
---client loaded and set up with the binary `cargo build` made. Nothing is opened in it.
---@param cwd string
---@return integer channel
---@return string socket
local function start_child(cwd)
  sockets = sockets + 1
  local socket = ('/tmp/ot-projects-%d-%d.sock'):format(vim.fn.getpid(), sockets)
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
  }, { rpc = true, cwd = cwd })
  assert(channel > 0, 'the child Neovim did not start')
  table.insert(children, { channel = channel, socket = socket })
  vim.rpcrequest(channel, 'nvim_exec_lua', 'vim.o.columns = 160', {})
  vim.rpcrequest(channel, 'nvim_exec_lua', "require('orca-term').setup({ binary = ... })", {
    support.binary,
  })
  return channel, socket
end

---Whether the sidebar of the child Neovim shows `text` on some line.
---@param child integer
---@param text string
---@return boolean
local function child_shows(child, text)
  local shown = vim.rpcrequest(
    child,
    'nvim_exec_lua',
    "return vim.api.nvim_buf_get_lines(require('orca-term.sidebar').buffer(), 0, -1, false)",
    {}
  ) --[[@as string[] ]]
  for _, line in ipairs(shown) do
    if line:find(text, 1, true) then
      return true
    end
  end
  return false
end

---Waits until the sidebar of the child Neovim shows `text`, or, with `gone`, no longer shows it.
---@param child integer
---@param text string
---@param gone? boolean
local function wait_for_sidebar(child, text, gone)
  support.wait_for(function()
    return child_shows(child, text) ~= (gone == true)
  end, ('the child to %s `%s`'):format(gone and 'drop' or 'show', text))
end

---Waits until the registry of `repo` holds `socket`, or, with `gone`, no longer holds it.
---@param repo spec.Repo
---@param socket string
---@param gone? boolean
local function wait_registered(repo, socket, gone)
  support.wait_for(function()
    return vim.tbl_contains(support.registry(repo), socket) ~= (gone == true)
  end, ('%s to be %s the registry of %s'):format(socket, gone and 'out of' or 'in', repo.root))
end

---What the child Neovim draws from its global statusline.
---@param child integer
---@return string
local function child_statusline(child)
  return vim.rpcrequest(
    child,
    'nvim_exec_lua',
    'return vim.api.nvim_eval_statusline(vim.o.statusline, {}).str',
    {}
  ) --[[@as string]]
end

---Quits the child Neovim with `:qa`, as a person would, and checks it exited 0.
---@param child integer
local function quit(child)
  -- The child exits before it answers: the request fails as its channel closes.
  local answered = pcall(vim.rpcrequest, child, 'nvim_command', 'qa')
  assert(not answered, 'the child is still running after :qa')
  assert.are.same({ 0 }, vim.fn.jobwait({ child }, 10000))
end

---Makes a worktree in `repo` through the core.
---@param repo spec.Repo
---@param name string
local function new_worktree(repo, name)
  support.run({ support.binary, 'worktree', 'new', name }, repo.root)
end

---Two repositories in one fixture, neither on the project list yet: `repo` and `api`.
---@return spec.Repo repo
---@return spec.Repo api
local function two_repos()
  support.load_client()
  local repo = support.repo()
  return repo, support.sibling_repo(repo, 'api')
end

---Two projects on the list, in this order: `repo`, with the worktree `one`, then `api`, with the
---worktrees `two` and `three`. Neovim sits in the first project's primary checkout.
---@return spec.Repo repo the first project
---@return spec.Repo api the second project
local function two_projects()
  local repo, api = two_repos()
  new_worktree(repo, 'one')
  new_worktree(api, 'two')
  new_worktree(api, 'three')
  support.add_project(repo)
  support.add_project(api)
  support.cd(repo.root)
  return repo, api
end

---The sidebar buffer's lines, in this Neovim.
---@return string[]
local function lines()
  return vim.api.nvim_buf_get_lines(assert(sidebar.buffer()), 0, -1, false)
end

---The highlight group `PR` is drawn in on the row of the worktree `name` in this Neovim's sidebar,
---at the end of the row's second line; `nil` when the row shows no `PR`.
---@param name string
---@return string?
local function pr_group(name)
  local buf = assert(sidebar.buffer())
  local shown = lines()
  for index, line in ipairs(shown) do
    if vim.endswith(line, ' ' .. name) or line:find(' ' .. name .. ' ', 1, true) then
      local second = shown[index + 1] or ''
      local marks = vim.api.nvim_buf_get_extmarks(
        buf,
        sidebar.NAMESPACE,
        { index, 0 },
        { index, -1 },
        { details = true }
      )
      for _, mark in ipairs(marks) do
        local details = mark[4] or {}
        if second:sub(mark[3] + 1, details.end_col) == 'PR' then
          return details.hl_group
        end
      end
      return nil
    end
  end
  return nil
end

---What `gh pr view --json ...` prints for pull request 12, open, with the checks `rollup`.
---@param rollup table[]
---@return table
local function view(rollup)
  return {
    number = 12,
    title = 'Two',
    state = 'OPEN',
    isDraft = false,
    url = 'https://github.com/o/r/pull/12',
    statusCheckRollup = rollup,
  }
end

local RUNNING = { { __typename = 'CheckRun', status = 'IN_PROGRESS', conclusion = '' } }
local PASSED = { { __typename = 'CheckRun', status = 'COMPLETED', conclusion = 'SUCCESS' } }

---The tab-local directory of the current tab.
---@return string
local function tab_directory()
  return vim.fn.getcwd(-1, vim.fn.tabpagenr())
end

---Counts every run of the core's reads in this Neovim, until the spec ends: `fleets` and
---`worktree_ls`, waited for or not.
---@return { fleets: integer, worktree_ls: integer } counted
local function count_reads()
  local core = require('orca-term.core')
  local counted = { fleets = 0, worktree_ls = 0 }
  local real = {}
  for name, key in pairs({
    fleets = 'fleets',
    fleets_async = 'fleets',
    worktree_ls = 'worktree_ls',
    worktree_ls_async = 'worktree_ls',
  }) do
    real[name] = core[name]
    core[name] = function(...)
      counted[key] = counted[key] + 1
      return real[name](...)
    end
  end
  finally(function()
    for name, read in pairs(real) do
      core[name] = read
    end
  end)
  return counted
end

describe('every project, live', function()
  local columns = vim.o.columns

  before_each(function()
    vim.o.columns = 160
  end)

  after_each(function()
    for _, child in ipairs(children) do
      vim.fn.jobstop(child.channel)
      vim.fn.jobwait({ child.channel }, 5000)
      vim.fn.delete(child.socket)
    end
    children = {}
    pr._reset()
    pr.POLL_MS = 60 * 1000
    require('orca-term.strip')._reset()
    support.reset_editor()
    support.cleanup()
    vim.o.columns = columns
  end)

  it(
    'registers with every project, shows a write made in either, and :qa unregisters from both',
    function()
      local repo, api = two_repos()
      support.add_project(repo)
      support.add_project(api)
      local child, socket = start_child(repo.root)
      vim.rpcrequest(child, 'nvim_command', 'OrcaTerm sidebar')
      wait_registered(repo, socket)
      wait_registered(api, socket)

      -- Nothing is asked of the child from here on but its sidebar's lines.
      new_worktree(api, 'two')
      wait_for_sidebar(child, '· two')
      new_worktree(repo, 'one')
      wait_for_sidebar(child, '· one')
      support.run({ support.binary, 'worktree', 'rm', 'two' }, api.root)
      wait_for_sidebar(child, '· two', true)

      quit(child)
      assert.are.same({}, support.registry(repo))
      assert.are.same({}, support.registry(api))
    end
  )

  it('registers with every project from a Neovim outside every repository', function()
    local repo, api = two_repos()
    support.add_project(repo)
    support.add_project(api)
    local child, socket = start_child(repo.tmp)
    vim.rpcrequest(child, 'nvim_command', 'OrcaTerm sidebar')
    wait_registered(repo, socket)
    wait_registered(api, socket)

    new_worktree(api, 'two')
    wait_for_sidebar(child, '· two')
  end)

  it(
    'registers with a project added after the sidebar opened, and unregisters once it is removed',
    function()
      local repo, api = two_repos()
      support.add_project(repo)
      local child, socket = start_child(repo.root)
      vim.rpcrequest(child, 'nvim_command', 'OrcaTerm sidebar')
      wait_registered(repo, socket)
      assert.are.same({}, support.registry(api))

      -- The project list belongs to no repository, so adding to it tells nobody: the next read,
      -- here after a change in the first project, finds the new project and registers with it.
      support.add_project(api)
      new_worktree(repo, 'one')
      wait_for_sidebar(child, '▾ api')
      wait_registered(api, socket)

      -- From then on a change in the new project alone shows.
      new_worktree(api, 'two')
      wait_for_sidebar(child, '· two')

      -- Taken off the list, it is unregistered from at the next read.
      support.run({ support.binary, 'project', 'rm', api.root }, repo.root)
      new_worktree(repo, 'later')
      wait_for_sidebar(child, '▾ api', true)
      wait_registered(api, socket, true)
      assert.are.same({ socket }, support.registry(repo))
    end
  )

  it("counts every project's worktrees in the status strip", function()
    two_projects()
    local strip = require('orca-term.strip')
    strip._reset()

    strip.statusline()

    support.wait_for(function()
      return strip.statusline() == '%#OrcaTermStateNoAgent#·3%*'
    end, 'the strip to count the worktrees of both projects')
    assert.are.same({ states = { { state = 'no_agent', count = 3 } }, unseen = 0 }, strip.counts())
  end)

  it('shows a write in the second project in the status strip, the sidebar never opened', function()
    local repo, api = two_repos()
    new_worktree(repo, 'one')
    support.add_project(repo)
    support.add_project(api)
    local child, socket = start_child(repo.root)
    vim.rpcrequest(
      child,
      'nvim_exec_lua',
      'vim.o.statusline = ...',
      { "%{%v:lua.require'orca-term.strip'.statusline()%}" }
    )

    -- Drawing the statusline starts the strip, which registers with both projects.
    child_statusline(child)
    wait_registered(repo, socket)
    wait_registered(api, socket)
    support.wait_for(function()
      return child_statusline(child) == '·1'
    end, "the child's statusline to show the first project's worktree")

    new_worktree(api, 'two')

    support.wait_for(function()
      return child_statusline(child) == '·2'
    end, "the child's statusline to count the second project's new worktree")
    assert.is_true(
      vim.rpcrequest(
        child,
        'nvim_exec_lua',
        "return package.loaded['orca-term.sidebar'] == nil",
        {}
      )
    )
  end)

  it(":OrcaTerm jump lists every project's worktrees in sidebar order and goes to one", function()
    local _, api = two_projects()
    local ui = support.stub_ui({ select = { 'two  no agent' } })

    -- The sidebar is closed: jumping reads the projects itself, from the first project.
    vim.cmd('OrcaTerm jump')

    assert.are.same({ { 'one  no agent', 'three  no agent', 'two  no agent' } }, ui.shown)
    assert.are.equal(support.worktree_path(api, 'two'), tab_directory())
    assert.are.equal(2, #vim.api.nvim_list_tabpages())
    assert.are.same({}, ui.notes)
  end)

  it(':OrcaTerm worktree N counts across the projects, and says when there is none', function()
    local repo, api = two_projects()
    local ui = support.stub_ui({})

    vim.cmd('OrcaTerm worktree 3')
    assert.are.equal(support.worktree_path(api, 'two'), tab_directory())

    vim.cmd('OrcaTerm worktree 1')
    assert.are.equal(support.worktree_path(repo, 'one'), tab_directory())

    vim.cmd('normal ' .. vim.keycode('<Plug>(OrcaTermWorktree2)'))
    assert.are.equal(support.worktree_path(api, 'three'), tab_directory())
    assert.are.equal(4, #vim.api.nvim_list_tabpages())

    vim.cmd('OrcaTerm worktree 4')
    assert.are.equal(4, #vim.api.nvim_list_tabpages())
    assert.are.same({
      {
        message = 'orca-term: there is no worktree 4; the projects have 3',
        level = vim.log.levels.ERROR,
      },
    }, ui.notes)
  end)

  it('the jumps say why the project list cannot be read', function()
    local repo = two_projects()
    local ui = support.stub_ui({})
    vim.fn.writefile({ 'not json' }, vim.env.XDG_STATE_HOME .. '/orca-term/projects.json')

    vim.cmd('OrcaTerm jump')
    vim.cmd('OrcaTerm worktree 1')

    assert.are.equal(2, #ui.notes)
    for _, note in ipairs(ui.notes) do
      assert.are.equal(vim.log.levels.ERROR, note.level)
      assert(note.message:find('projects.json', 1, true), note.message)
    end
    assert.are.same({}, ui.prompts)
    assert.are.equal(repo.root, tab_directory())
  end)

  it(
    "refreshes the second project's pull requests on opening and while their checks are pending",
    function()
      local repo, api = two_projects()
      pr.POLL_MS = 200
      -- Both repositories share the fixture's fake gh, which answers by branch.
      support.gh_answer(repo, 'two', view(RUNNING))

      -- No key is pressed: Neovim sits in the first project, where the core knows no `two`.
      vim.cmd('OrcaTerm sidebar')

      support.wait_for(function()
        return pr_group('two') == 'OrcaTermPrPending'
      end, 'the pending pull request of two, in the second project')
      assert.is_true(support.listed(api, 'two').pr.checks == 'pending')
      assert.is_true(support.listed(api, 'three').pr.none)
      assert.is_true(support.listed(repo, 'one').pr.none)
      assert.is_true(pr.polling())

      support.gh_answer(repo, 'two', view(PASSED))
      support.wait_for(function()
        return pr_group('two') == 'OrcaTermPrPassing'
      end, 'the timer to refresh the pull request of two')
      support.wait_for(function()
        return not pr.polling()
      end, 'the timer to stop once nothing is pending')
    end
  )

  it('refreshes a pull request of the second project when one of its turns ends', function()
    support.load_client()
    pr.MIN_AGE_MS = 60 * 60 * 1000
    finally(function()
      pr.MIN_AGE_MS = 60 * 1000
    end)
    local repo = support.agent_repo()
    local api = support.sibling_repo(repo, 'api')
    new_worktree(repo, 'one')
    new_worktree(api, 'two')
    support.run({ support.binary, 'agent', 'start', 'two' }, api.root)
    support.add_project(repo)
    support.add_project(api)
    support.cd(repo.root)
    vim.cmd('OrcaTerm sidebar')
    support.wait_for(function()
      return #support.gh_calls(repo) == 2
    end, 'the refresh of both projects on opening')

    support.play_hooks(api, support.worktree_path(api, 'two'), 'p-hello-2.1.300.log')
    -- This Neovim is not registered with the core: the sidebar reads the projects when asked.
    sidebar.refresh()

    support.wait_for(function()
      return #support.gh_calls(repo) == 3
    end, 'the refresh after the turn of two ended')
    assert.is_true(vim.startswith(support.gh_calls(repo)[3], 'pr view two '))
  end)

  it('reads fleets once, and never worktree ls, with projects on the list', function()
    two_projects()
    local counted = count_reads()

    vim.cmd('OrcaTerm sidebar')
    assert.are.same({ fleets = 1, worktree_ls = 0 }, counted)

    vim.cmd('doautocmd FocusGained')
    assert.are.same({ fleets = 2, worktree_ls = 0 }, counted)

    -- A read without waiting, as after the core's event.
    require('orca-term.live').read_soon()
    support.wait_for(function()
      return counted.fleets >= 3
    end, 'the read without waiting')
    vim.wait(300)
    assert.are.equal(0, counted.worktree_ls)
  end)

  it('reads fleets, then worktree ls, with an empty project list, as before', function()
    local repo = two_repos()
    new_worktree(repo, 'one')
    support.cd(repo.root)
    local counted = count_reads()

    vim.cmd('OrcaTerm sidebar')

    assert.are.same({ fleets = 1, worktree_ls = 1 }, counted)
    assert.are.equal(' ▾ repo  main', lines()[1])
  end)
end)
