-- Seam 2: live sidebar updates, in child Neovims against the real binary. Each child is embedded,
-- driven over RPC, with only this client loaded, listening at a short socket path of its own
-- (`--listen`; a unix socket address holds about 100 bytes, and macOS's $TMPDIR is long). Opening
-- the sidebar registers the child with the core; the spec then writes to a worktree from outside
-- the children, through the core, and waits for their sidebars to show it, calling into them only
-- to read their sidebar's lines. The spec's own Neovim listens on nothing (`support.load_client`),
-- so the core only ever tells the children.

local support = require('support')

---The `User` pattern the core fires.
local EVENT = 'OrcaTermWorktreeChanged'

---The child Neovims the spec started, with their socket paths, stopped after it.
---@type { channel: integer, socket: string }[]
local children = {}

---How many sockets this spec process has named, to keep each path unique.
local sockets = 0

---A new socket path under /tmp, short enough for a unix socket address, with nothing at it.
---@return string
local function new_socket()
  sockets = sockets + 1
  local path = ('/tmp/ot-%d-%d.sock'):format(vim.fn.getpid(), sockets)
  vim.fn.delete(path)
  return path
end

---A child Neovim listening at a socket of its own, in the primary checkout of `repo`, with this
---client loaded and set up with the binary `cargo build` made, and its sidebar open. It is started
---by its full path: an agent repository's PATH holds no nvim.
---@param repo spec.Repo
---@return integer channel
---@return string socket
local function start_child(repo)
  local socket = new_socket()
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
  return channel, socket
end

---Lua run in a child Neovim: the lines of the sidebar shown in tab `...` (the current tab when
---`nil`), or `{}` when that tab shows no sidebar.
local SIDEBAR_LINES = [[
  local tab = ... or vim.api.nvim_get_current_tabpage()
  local buf = require('orca-term.sidebar').buffer()
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(tab)) do
    if vim.api.nvim_win_get_buf(win) == buf then
      return vim.api.nvim_buf_get_lines(buf, 0, -1, false)
    end
  end
  return {}
]]

---Whether the sidebar of the child Neovim, in its tab `tab` (the current one when `nil`), shows
---`text` on some line.
---@param child integer
---@param text string
---@param tab? integer
---@return boolean
local function child_shows(child, text, tab)
  local lines = vim.rpcrequest(child, 'nvim_exec_lua', SIDEBAR_LINES, { tab }) --[[@as string[] ]]
  for _, line in ipairs(lines) do
    if line:find(text, 1, true) then
      return true
    end
  end
  return false
end

---Waits until the sidebar of every child Neovim in `channels` shows `text`, or, with `gone`, no
---longer shows it.
---@param channels integer[]
---@param text string
---@param gone? boolean
local function wait_for_sidebars(channels, text, gone)
  for index, child in ipairs(channels) do
    support.wait_for(function()
      return child_shows(child, text) ~= (gone == true)
    end, ('child %d to %s `%s`'):format(index, gone and 'drop' or 'show', text))
  end
end

---Waits until the registry of `repo` holds `socket`.
---@param repo spec.Repo
---@param socket string
local function wait_registered(repo, socket)
  support.wait_for(function()
    return vim.tbl_contains(support.registry(repo), socket)
  end, socket .. ' in the registry')
end

---Lua run in a child Neovim: counts every read of the fleet, waited for or not, in `_G.reads`.
local COUNT_READS = [[
  local core = require('orca-term.core')
  _G.reads = 0
  for _, name in ipairs({ 'worktree_ls', 'worktree_ls_async' }) do
    local real = core[name]
    core[name] = function(...)
      _G.reads = _G.reads + 1
      return real(...)
    end
  end
]]

---Quits the child Neovim with `:qa`, as a person would, and checks it exited 0.
---@param child integer
local function quit(child)
  -- The child exits before it answers: the request fails as its channel closes.
  local answered = pcall(vim.rpcrequest, child, 'nvim_command', 'qa')
  assert(not answered, 'the child is still running after :qa')
  assert.are.same({ 0 }, vim.fn.jobwait({ child }, 10000))
end

describe('the live sidebar', function()
  after_each(function()
    for _, child in ipairs(children) do
      vim.fn.jobstop(child.channel)
      vim.fn.jobwait({ child.channel }, 5000)
      vim.fn.delete(child.socket)
    end
    children = {}
    support.cleanup()
  end)

  it(
    'registers when the sidebar opens, stays registered when it closes, and :qa unregisters',
    function()
      support.load_client()
      local repo = support.repo()
      local child, socket = start_child(repo)

      wait_registered(repo, socket)
      vim.rpcrequest(child, 'nvim_command', 'OrcaTerm sidebar')
      assert.are.same({ socket }, support.registry(repo))

      quit(child)
      assert.are.same({}, support.registry(repo))
    end
  )

  it('shows a write made outside them in two Neovims of the repository', function()
    support.load_client()
    local repo = support.repo()
    local first, first_socket = start_child(repo)
    local second, second_socket = start_child(repo)
    wait_registered(repo, first_socket)
    wait_registered(repo, second_socket)

    support.run({ support.binary, 'worktree', 'new', 'demo' }, repo.root)
    wait_for_sidebars({ first, second }, '· demo')

    support.run({ support.binary, 'worktree', 'rm', 'demo' }, repo.root)
    wait_for_sidebars({ first, second }, '· demo', true)
  end)

  it("shows the agent's hooks in the sidebar of every tab", function()
    support.load_client()
    local repo = support.agent_repo()
    support.run({ support.binary, 'worktree', 'new', 'fix' }, repo.root)
    support.run({ support.binary, 'agent', 'start', 'fix' }, repo.root)
    local fix = support.worktree_path(repo, 'fix')
    local child, socket = start_child(repo)
    vim.rpcrequest(child, 'nvim_command', 'tabnew')
    local second_tab = vim.rpcrequest(child, 'nvim_get_current_tabpage')
    vim.rpcrequest(child, 'nvim_command', 'tabfirst')
    assert.is_true(child_shows(child, '○ fix', second_tab))
    wait_registered(repo, socket)

    support.play_hooks(repo, fix, 'p-hello-2.1.300.log')

    -- The second tab is not the current one: its window shows the same buffer, painted anew.
    support.wait_for(function()
      return child_shows(child, '● fix', second_tab)
    end, 'the finished turn in the second tab')
    assert.is_true(child_shows(child, '● fix'))
  end)

  it('prunes a stale address on the next write, and the live Neovim still updates', function()
    support.load_client()
    local repo = support.repo()
    -- A path nothing ever listened at, registered through the core command.
    local nothing = new_socket()
    support.run({ support.binary, 'nvim', 'register', nothing, '--json' }, repo.root)
    local live, live_socket = start_child(repo)
    local killed, killed_socket = start_child(repo)
    wait_registered(repo, live_socket)
    wait_registered(repo, killed_socket)
    -- Killed outright, it never unregisters, and its socket file stays behind.
    vim.uv.kill(vim.fn.jobpid(killed), 'sigkill')
    vim.fn.jobwait({ killed }, 5000)
    local everyone = { nothing, live_socket, killed_socket }
    table.sort(everyone)
    assert.are.same(everyone, support.registry(repo))

    support.run({ support.binary, 'worktree', 'new', 'demo' }, repo.root)

    assert.are.same({ live_socket }, support.registry(repo))
    wait_for_sidebars({ live }, '· demo')
  end)

  it(
    'reads the fleet a bounded number of times for a burst of events, ending up current',
    function()
      support.load_client()
      local repo = support.repo()
      local child, socket = start_child(repo)
      wait_registered(repo, socket)
      vim.rpcrequest(child, 'nvim_exec_lua', COUNT_READS, {})
      -- A worktree the core is not told about, so only the burst below makes the child see it.
      local manual = support.add_worktree(repo, repo.tmp .. '/manual', 'manual')

      -- 20 notifications in a row, as the core sends them; two without `data`, which nothing needs.
      for index = 1, 20 do
        local options = { pattern = EVENT, data = { path = manual } }
        if index % 10 == 0 then
          options.data = nil
        end
        vim.rpcnotify(child, 'nvim_exec_autocmds', 'User', options)
      end

      wait_for_sidebars({ child }, 'manual')
      -- Long enough for any read the burst still owes to start and end.
      vim.wait(500)
      local reads = vim.rpcrequest(child, 'nvim_exec_lua', 'return _G.reads', {})
      assert(reads >= 1 and reads <= 2, ('the burst made %d reads'):format(reads))
      assert.are.equal('', vim.rpcrequest(child, 'nvim_eval', 'v:errmsg'))
    end
  )

  it('reads nothing on an event while the sidebar is closed', function()
    support.load_client()
    local repo = support.repo()
    local child, socket = start_child(repo)
    wait_registered(repo, socket)
    vim.rpcrequest(child, 'nvim_command', 'OrcaTerm sidebar')
    vim.rpcrequest(child, 'nvim_exec_lua', COUNT_READS, {})

    -- A write the core tells the child about, and an event fired by hand.
    support.run({ support.binary, 'worktree', 'new', 'demo' }, repo.root)
    vim.rpcnotify(child, 'nvim_exec_autocmds', 'User', { pattern = EVENT })
    vim.wait(500)

    assert.are.equal(0, vim.rpcrequest(child, 'nvim_exec_lua', 'return _G.reads', {}))
    assert.are.same({ socket }, support.registry(repo))
  end)
end)
