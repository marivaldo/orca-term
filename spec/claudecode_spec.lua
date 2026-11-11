-- Seam 2: `orca-term doctor` asks the Neovims registered with the core whether they have
-- coder/claudecode.nvim loaded (#179), against the real binary and real child Neovims started with
-- `--clean`, so no person's config runs. The plugin is never installed: a child "has it loaded"
-- when `package.loaded['claudecode']` is set, which is all the doctor reads.

local support = require('support')

---The child Neovims the spec started, with their socket paths, stopped after it.
---@type { channel: integer, socket: string }[]
local children = {}

---How many sockets this spec process has named, to keep each path unique.
local sockets = 0

---A child Neovim, clean and embedded, in the primary checkout of `repo`, listening at a short
---socket path of its own (a unix socket address holds about 100 bytes) and registered with the
---core as the Lua client registers one.
---@param repo spec.Repo
---@return integer channel
local function start_registered_child(repo)
  sockets = sockets + 1
  local socket = ('/tmp/ot-%d-cc%d.sock'):format(vim.fn.getpid(), sockets)
  vim.fn.delete(socket)
  local channel = vim.fn.jobstart(
    { vim.v.progpath, '--embed', '--headless', '--clean', '--listen', socket },
    { rpc = true, cwd = repo.root }
  )
  assert(channel > 0, 'the child Neovim did not start')
  table.insert(children, { channel = channel, socket = socket })
  -- Answered once the child is up, so its socket listens.
  vim.rpcrequest(channel, 'nvim_get_api_info')
  support.run({ support.binary, 'nvim', 'register', socket }, repo.root)
  return channel
end

---Lua run in a child Neovim: on its next turn of the event loop, runs `doctor --json` with the
---binary `...` in the directory `select(2, ...)`, waits for it as `:checkhealth orca-term` does,
---and keeps what it printed in `_G.doctor_report`.
local RUN_DOCTOR_LATER = [[
  local binary, cwd = ...
  vim.schedule(function()
    local result = vim.system({ binary, 'doctor', '--json' }, { cwd = cwd, text = true }):wait()
    _G.doctor_report = result.stdout
  end)
]]

---The doctor's claudecode check in `report`, the document `doctor --json` printed.
---@param report string
---@return table
local function claudecode_check(report)
  for _, check in ipairs(vim.json.decode(report).checks) do
    if check.name == 'claudecode' then
      return check
    end
  end
  error('no claudecode check in ' .. report)
end

---The doctor's claudecode check, run from the primary checkout of `repo`. It may exit 9 when
---the machine has no claude, as CI does; its report is printed all the same.
---@param repo spec.Repo
---@return table
local function doctor_check(repo)
  return claudecode_check(support.core(repo, { 'doctor', '--json' }).stdout)
end

describe('the claudecode.nvim line of the doctor', function()
  after_each(function()
    for _, child in ipairs(children) do
      vim.fn.jobstop(child.channel)
      vim.fn.jobwait({ child.channel }, 5000)
      vim.fn.delete(child.socket)
    end
    children = {}
    support.cleanup()
  end)

  it('asks a registered Neovim whether the plugin is loaded, warning when it is not', function()
    local repo = support.repo()
    local child = start_registered_child(repo)

    local missing = doctor_check(repo)
    assert.are.equal('warn', missing.status)
    assert.are.equal(
      'claudecode.nvim not loaded in 1 of 1 registered Neovims; '
        .. 'claude links only to a Neovim running it (see the README)',
      missing.message
    )

    vim.rpcrequest(child, 'nvim_exec_lua', "package.loaded['claudecode'] = {}", {})
    local loaded = doctor_check(repo)
    assert.are.equal('ok', loaded.status)
    assert.are.equal('claudecode.nvim loaded in 1 of 1 registered Neovims', loaded.message)
  end)

  -- Why `:checkhealth orca-term` checks its own Neovim itself (health_spec.lua): a Neovim
  -- waiting for the doctor does not answer it, so the doctor leaves that one unchecked.
  it('leaves unchecked the Neovim that waits for it, as :checkhealth does', function()
    local repo = support.repo()
    local child = start_registered_child(repo)
    vim.rpcrequest(child, 'nvim_exec_lua', "package.loaded['claudecode'] = {}", {})

    -- Run from the child's own event loop, as a command the person types is, and not inside the
    -- spec's request.
    vim.rpcrequest(child, 'nvim_exec_lua', RUN_DOCTOR_LATER, { support.binary, repo.root })
    local report = nil
    support.wait_for(function()
      report = vim.rpcrequest(child, 'nvim_exec_lua', 'return _G.doctor_report', {})
      return report ~= vim.NIL and report ~= nil
    end, "the child's doctor report")

    local check = claudecode_check(report --[[@as string]])
    assert.are.equal('info', check.status)
    assert.are.equal(
      'claudecode.nvim not checked: none of the 1 registered Neovims answered',
      check.message
    )
  end)
end)
