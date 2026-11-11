-- Seam 2: what the core asks a worktree's Neovim (#177), in child Neovims against the real binary.
-- When the agent's turn ends, the core asks every registered Neovim to `:checktime`, so a buffer of
-- a file the agent wrote shows what it wrote. `orca-term nvim diff <worktree> <file>`, what the
-- application runs on enter on a changed file of its right column, asks every registered Neovim to
-- open that file's diff in the worktree's review; only a Neovim working in the worktree opens it.
--
-- Each child is embedded, driven over RPC, started with `--clean` in a directory of the spec's
-- choosing and listening at a short socket path of its own (`--listen`; a unix socket address holds
-- about 100 bytes, and macOS's $TMPDIR is long). It is registered with the core by hand, as the
-- application registers the Neovims it starts. diffview.nvim comes from .rocks/diffview.nvim, as
-- spec/review_spec.lua takes it.

local support = require('support')

---Where `just lua-rocks` puts diffview.nvim.
local DIFFVIEW = support.root .. '/.rocks/diffview.nvim'

---The recording of claude's hooks of one turn: the session starts (1), a prompt (2), the agent
---stops (3), the session ends (4).
local HELLO = 'p-hello-2.1.300.log'

---The child Neovims the spec started, with their socket paths, stopped after it.
---@type { channel: integer, socket: string }[]
local children = {}

---How many sockets this spec process has named, to keep each path unique.
local sockets = 0

---A new socket path under /tmp, short enough for a unix socket address, with nothing at it.
---@return string
local function new_socket()
  sockets = sockets + 1
  local path = ('/tmp/ot-l%d-%d.sock'):format(vim.fn.getpid(), sockets)
  vim.fn.delete(path)
  return path
end

---A child Neovim in `dir`, listening at a socket of its own and registered with the core there,
---with this client and diffview.nvim loaded. It is started by its full path: an agent repository's
---PATH holds no nvim.
---@param dir string
---@return integer channel
local function start_child(dir)
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
    '--cmd',
    'set runtimepath+=' .. vim.fn.fnameescape(DIFFVIEW),
  }, { rpc = true, cwd = dir })
  assert(channel > 0, 'the child Neovim did not start')
  table.insert(children, { channel = channel, socket = socket })
  vim.rpcrequest(channel, 'nvim_exec_lua', "require('orca-term').setup({ binary = ... })", {
    support.binary,
  })
  -- diffview logs under stdpath('cache'), which follows the fixture's HOME.
  vim.rpcrequest(
    channel,
    'nvim_exec_lua',
    "require('diffview').setup({ use_icons = false }); DiffviewGlobal.logger.outfile = ...",
    { vim.fn.tempname() }
  )
  support.run({ support.binary, 'nvim', 'register', socket }, dir)
  return channel
end

---The lines of the child's current buffer.
---@param child integer
---@return string[]
local function buffer_lines(child)
  return vim.rpcrequest(child, 'nvim_buf_get_lines', 0, 0, -1, false) --[[@as string[] ]]
end

---Lua run in a child Neovim: the worktree the current tab is orca-term's review of and the file
---diffview shows there, or `nil` for each.
local SHOWN = [[
  local lib = package.loaded['diffview.lib']
  local view = lib and lib.get_current_view()
  local file = view and view.panel.cur_file and view.panel.cur_file.path
  return { vim.t.orca_term_review or vim.NIL, file or vim.NIL }
]]

describe("the core's requests to a worktree's Neovim", function()
  after_each(function()
    for _, child in ipairs(children) do
      vim.fn.jobstop(child.channel)
      vim.fn.jobwait({ child.channel }, 5000)
      vim.fn.delete(child.socket)
    end
    children = {}
    support.cleanup()
  end)

  it('reload a file the agent wrote once its turn ends, with :checktime', function()
    support.load_client()
    local repo = support.agent_repo()
    support.run({ support.binary, 'worktree', 'new', 'fix' }, repo.root)
    local fix = support.worktree_path(repo, 'fix')
    support.run({ support.binary, 'agent', 'start', 'fix' }, repo.root)
    vim.fn.writefile({ 'before' }, fix .. '/notes.txt')
    local child = start_child(fix)
    vim.rpcrequest(child, 'nvim_command', 'edit notes.txt')

    vim.fn.writefile({ 'after', 'the agent wrote it' }, fix .. '/notes.txt')
    support.play_hooks(repo, fix, HELLO, 1, 2)
    assert.are.same({ 'before' }, buffer_lines(child), 'the turn has not ended yet')

    support.play_hooks(repo, fix, HELLO, 3, 3)
    assert.are.same({ 'after', 'the agent wrote it' }, buffer_lines(child))
  end)

  it("open a file's diff in the review where the Neovim works in the worktree only", function()
    support.load_client()
    local repo = support.agent_repo()
    support.run({ support.binary, 'worktree', 'new', 'fix' }, repo.root)
    local fix = support.worktree_path(repo, 'fix')
    vim.fn.writefile({ 'one' }, fix .. '/a.txt')
    vim.fn.writefile({ 'two' }, fix .. '/b.txt')
    support.run({ 'git', 'add', '.' }, fix)
    support.run({ 'git', 'commit', '--quiet', '-m', 'two files' }, fix)
    local in_fix = start_child(fix)
    local in_primary = start_child(repo.root)

    support.run({ support.binary, 'nvim', 'diff', 'fix', 'b.txt' }, repo.root)
    support.wait_for(function()
      local shown = vim.rpcrequest(in_fix, 'nvim_exec_lua', SHOWN, {}) --[[@as any[] ]]
      return shown[1] == fix and shown[2] == 'b.txt'
    end, "the review of fix on b.txt in fix's Neovim")
    assert.are.same(
      1,
      #vim.rpcrequest(in_primary, 'nvim_list_tabpages'),
      'the Neovim working in the primary checkout opened nothing'
    )

    -- Asked again, the review already open shows the other file.
    support.run({ support.binary, 'nvim', 'diff', 'fix', 'a.txt' }, repo.root)
    support.wait_for(function()
      return vim.rpcrequest(in_fix, 'nvim_exec_lua', SHOWN, {})[2] == 'a.txt'
    end, 'the review on a.txt')
    assert.are.same(2, #vim.rpcrequest(in_fix, 'nvim_list_tabpages'), 'one review tab')
  end)
end)
