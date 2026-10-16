-- Seam 2: the live fleet (`live.lua`) and its subscribers, in a child Neovim against the real
-- binary. The child is embedded, driven over RPC, with only this client loaded and its sidebar
-- closed, listening at a short socket path of its own as `spec/live_spec.lua` explains. The spec
-- subscribes functions that record every fleet they get, fires the core's event by hand, counts the
-- reads of the fleet by wrapping the core's two reads, and reads back what each subscriber got.

local support = require('support')

---The `User` pattern the core fires.
local EVENT = 'OrcaTermWorktreeChanged'

---The child Neovims the spec started, with their socket paths, stopped after it.
---@type { channel: integer, socket: string }[]
local children = {}

---A child Neovim listening at a short socket path under /tmp, in the primary checkout of `repo`,
---with this client loaded and set up with the binary `cargo build` made, its sidebar closed, and
---its reads of the fleet counted in `_G.reads`.
---@param repo spec.Repo
---@return integer channel
local function start_child(repo)
  local socket = ('/tmp/ot-store-%d-%d.sock'):format(vim.fn.getpid(), #children + 1)
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
  vim.rpcrequest(channel, 'nvim_exec_lua', "require('orca-term').setup({ binary = ... })", {
    support.binary,
  })
  vim.rpcrequest(
    channel,
    'nvim_exec_lua',
    [[
      local core = require('orca-term.core')
      _G.reads = 0
      for _, name in ipairs({ 'worktree_ls', 'worktree_ls_async' }) do
        local real = core[name]
        core[name] = function(...)
          _G.reads = _G.reads + 1
          return real(...)
        end
      end
    ]],
    {}
  )
  return channel
end

---Lua run in a child Neovim: subscribes `...` (a name) to the live fleet with a function that
---records the worktrees' names of every fleet it gets in `_G.got[name]`, one list per call.
local SUBSCRIBE = [[
  local name = ...
  _G.got = _G.got or {}
  _G.got[name] = {}
  require('orca-term.live').subscribe(name, function(fleet)
    local names = {}
    for _, worktree in ipairs(fleet and fleet.worktrees or {}) do
      table.insert(names, worktree.name)
    end
    table.insert(_G.got[name], names)
  end)
]]

---Subscribes `name` to the live fleet in the child Neovim.
---@param child integer
---@param name string
local function subscribe(child, name)
  vim.rpcrequest(child, 'nvim_exec_lua', SUBSCRIBE, { name })
end

---Unsubscribes `name` from the live fleet in the child Neovim.
---@param child integer
---@param name string
local function unsubscribe(child, name)
  vim.rpcrequest(child, 'nvim_exec_lua', "require('orca-term.live').unsubscribe(...)", { name })
end

---Every fleet the subscriber `name` got in the child Neovim, each as its worktrees' names.
---@param child integer
---@param name string
---@return string[][]
local function got(child, name)
  return vim.rpcrequest(child, 'nvim_exec_lua', 'return _G.got[...]', { name }) --[[@as string[][] ]]
end

---How many times the child Neovim read the fleet, waited for or not.
---@param child integer
---@return integer
local function reads(child)
  return vim.rpcrequest(child, 'nvim_exec_lua', 'return _G.reads', {}) --[[@as integer]]
end

---Fires the core's event in the child Neovim, as the core does.
---@param child integer
local function fire(child)
  vim.rpcnotify(child, 'nvim_exec_autocmds', 'User', { pattern = EVENT })
end

describe('the live fleet', function()
  after_each(function()
    for _, child in ipairs(children) do
      vim.fn.jobstop(child.channel)
      vim.fn.jobwait({ child.channel }, 5000)
      vim.fn.delete(child.socket)
    end
    children = {}
    support.cleanup()
  end)

  it('hands the fleet of one read after an event to every subscriber', function()
    support.load_client()
    local repo = support.repo()
    local child = start_child(repo)
    subscribe(child, 'first')
    subscribe(child, 'second')
    -- A worktree the core is not told about, so only the event makes the subscribers see it.
    support.add_worktree(repo, repo.tmp .. '/manual', 'manual')

    fire(child)
    support.wait_for(function()
      return #got(child, 'first') > 0 and #got(child, 'second') > 0
    end, 'both subscribers to get the fleet')
    -- Long enough for any read the event still owes to start and end.
    vim.wait(500)

    assert.are.equal(1, reads(child))
    assert.are.same({ { 'manual' } }, got(child, 'first'))
    assert.are.same({ { 'manual' } }, got(child, 'second'))
  end)

  it('hands nothing to a name that unsubscribed', function()
    support.load_client()
    local repo = support.repo()
    local child = start_child(repo)
    subscribe(child, 'stays')
    subscribe(child, 'leaves')
    unsubscribe(child, 'leaves')

    fire(child)
    support.wait_for(function()
      return #got(child, 'stays') > 0
    end, 'the subscriber left to get the fleet')
    vim.wait(500)

    assert.are.equal(1, #got(child, 'stays'))
    assert.are.same({}, got(child, 'leaves'))
  end)

  it('reads nothing on an event or a focus while nobody is subscribed', function()
    support.load_client()
    local repo = support.repo()
    local child = start_child(repo)
    subscribe(child, 'gone')
    unsubscribe(child, 'gone')

    fire(child)
    vim.rpcnotify(child, 'nvim_exec_autocmds', 'FocusGained', {})
    vim.wait(500)

    assert.are.equal(0, reads(child))
    assert.are.same({}, got(child, 'gone'))
    assert.are.equal('', vim.rpcrequest(child, 'nvim_eval', 'v:errmsg'))
  end)
end)
