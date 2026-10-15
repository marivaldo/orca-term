-- orca-term's Neovim client: the link to the core, the `orca-term` binary.
--
-- This is the only module that starts a process (`cargo xtask client-boundary` checks it). It
-- finds the binary, runs one of its commands with `--json`, and checks that the binary speaks this
-- client's contract. `worktree ls` is waited for, or read without waiting when the core says a
-- worktree changed; the commands that may run a setup (`worktree new`, `setup`, `rm`) and `agent
-- start` are not waited for, and hand their result to a callback in Neovim's main loop. `nvim
-- register` tells the core where this Neovim listens, so it says when a worktree changes; `nvim
-- unregister` runs as Neovim exits and is waited for, briefly. `agent attach` is the one command
-- run without `--json`: it runs in a terminal buffer the caller made current, for the person to
-- use, until it detaches or the agent ends. The client never works the fleet out by itself: it
-- shows what the core prints.
--
-- Every function returns (or hands its callback) its result, or `nil` and an `orca_term.Error`.
-- None of them throws.

---A failure, written for a person, naming the fix.
---@class orca_term.Error
---What failed: the core's own `error.kind` (`busy`, `dirty`, `other`, ...), or one of this
---client's: `no_binary` (no core binary found), `contract` (the core speaks another contract),
---`usage` (the core refused its arguments, such as an invalid worktree name) or `failed` (the core
---could not start, did not finish in time, or printed no JSON).
---@field kind string
---@field message string

---The primary checkout, as `worktree ls --json` prints it.
---@class orca_term.Primary
---@field path string
---@field branch? string `nil` when the primary checkout is on no branch

---One worktree, as `worktree ls --json` prints it.
---@class orca_term.Worktree
---@field name string
---@field path string
---@field branch? string
---`no_agent`, `idle`, `working`, `waiting`, `finished`, `failed`, `interrupted`, `setting_up`,
---`not_ready` or `broken`
---@field state string
---@field detail? string why the worktree is not ready or broken
---What its agent is doing (`working`, `waiting`: a tool and its input, a question, or the prompt)
---or how its latest turn ended (`finished`, `failed`, `interrupted`: the agent's last words, the
---error, or the prompt); `nil` when there is nothing to say.
---@field activity? string
---@field agent? string the worktree's agent, `claude-code`; `nil` when it has none
---@field holder boolean whether the agent's holder is alive right now

---The fleet, as `worktree ls --json` prints it.
---@class orca_term.Fleet
---@field contract integer
---@field version string the core's version
---@field primary orca_term.Primary
---@field worktrees orca_term.Worktree[]

---What a setup did, as `worktree new` and `worktree setup` print it.
---@class orca_term.SetupResult
---@field status string `none` (no setup script), `succeeded` or `failed`
---@field log? string the setup log

---What `worktree new --json` prints: the worktree made, and what its setup did.
---@class orca_term.Created
---@field worktree orca_term.Worktree
---@field setup orca_term.SetupResult

---What `worktree setup --json` prints.
---@class orca_term.SetUp
---@field worktree orca_term.Worktree
---@field setup orca_term.SetupResult

---What `worktree rm --json` did with the worktree's branch.
---@class orca_term.RemovedBranch
---@field name? string `nil` when the worktree was detached
---@field status string `deleted`, `kept` or `none`
---@field reason? string why it was deleted or kept

---What `worktree rm --json` prints.
---@class orca_term.Removed
---@field name string
---@field path string
---@field branch orca_term.RemovedBranch

---The holder of an agent, as `agent start --json` prints it.
---@class orca_term.Holder
---@field status string `started`, or `already_running` when the agent was running already
---@field pid? integer the holder's process id

---What `agent start --json` prints: the worktree, its agent and its holder.
---@class orca_term.Started
---@field worktree orca_term.Worktree
---@field agent string the agent started, `claude-code`
---@field holder orca_term.Holder

local M = {}

---The JSON contract this client speaks. A core printing another one is refused.
M.CONTRACT = 1

---How long a core command that is waited for may take before it is stopped.
local TIMEOUT_SECONDS = 10

---How long `nvim unregister` may take: it runs as Neovim exits, which it must not hold up.
local UNREGISTER_TIMEOUT_SECONDS = 1

---The repository this client is loaded from: three directories up from this file,
---`<root>/lua/orca-term/core.lua`. `cargo build` puts the core binary under `<root>/target/`.
local ROOT = vim.fn.fnamemodify(debug.getinfo(1, 'S').source:sub(2), ':p:h:h:h')

---@param kind string
---@param message string
---@return orca_term.Error
local function failure(kind, message)
  return { kind = kind, message = 'orca-term: ' .. message }
end

---The core binary: `binary` from setup() when set; else the one `cargo build --release` or
---`cargo build` made in this repository; else `orca-term` on PATH.
---@return string? path
---@return orca_term.Error? err
function M.find_binary()
  local configured = require('orca-term').config.binary
  if configured then
    local path = vim.fs.normalize(configured)
    if vim.fn.executable(path) == 1 then
      return path
    end
    local message = 'the core binary set in setup() is not an executable file: '
      .. path
      .. '; fix `binary` in setup()'
    return nil, failure('no_binary', message)
  end

  local built = { ROOT .. '/target/release/orca-term', ROOT .. '/target/debug/orca-term' }
  for _, path in ipairs(built) do
    if vim.fn.executable(path) == 1 then
      return path
    end
  end

  local on_path = vim.fn.exepath('orca-term')
  if on_path ~= '' then
    return on_path
  end

  local message = 'no core binary found; run `cargo build` in '
    .. ROOT
    .. ' or set `binary` in setup()'
  return nil, failure('no_binary', message)
end

---The command line of `orca-term <args> --json`, and the same as one string for messages.
---@param binary string
---@param args string[]
---@return string[] command
---@return string shown
local function command_line(binary, args)
  local command = vim.list_extend({ binary }, args)
  table.insert(command, '--json')
  return command, table.concat(command, ' ')
end

---The JSON document a finished core command printed, checked against the contract, or the error
---it stands for: the core's own `error`, a usage error, output that is not JSON, or another
---contract.
---@param result vim.SystemCompleted
---@param binary string
---@param shown string the command line, for messages
---@return table? document
---@return orca_term.Error? err
local function decode(result, binary, shown)
  -- `luanil` turns JSON `null` into Lua `nil`, so a missing branch reads as `nil`.
  local decoded, document = pcall(vim.json.decode, result.stdout or '', {
    luanil = { object = true, array = true },
  })
  if not decoded or type(document) ~= 'table' then
    local stderr = vim.trim(result.stderr or '')
    -- Exit 2 is a usage error: the arguments were refused before the command ran, such as an
    -- invalid worktree name. Its first line says why, after `error: `.
    if result.code == 2 then
      local why = vim.split(stderr, '\n')[1]:gsub('^error: ', '')
      return nil, failure('usage', why)
    end
    local message = ('`%s` printed no JSON (exit %d): %s'):format(shown, result.code, stderr)
    return nil, failure('failed', message)
  end

  if document.contract ~= M.CONTRACT then
    local message = (
      'the core at %s speaks contract %s, this plugin speaks %d; '
      .. 'rebuild it with `cargo build` in %s or set `binary` in setup()'
    ):format(binary, tostring(document.contract), M.CONTRACT, ROOT)
    return nil, failure('contract', message)
  end

  if document.error then
    return nil, failure(document.error.kind, document.error.message)
  end
  return document
end

---The document a finished core command printed, or the error it stands for, as `decode` reads
---it; a command stopped for taking longer than `timeout` seconds is an error of its own.
---@param result vim.SystemCompleted
---@param binary string
---@param shown string the command line, for messages
---@param timeout? integer the time limit it ran with, in seconds; `nil` when it had none
---@return table? document
---@return orca_term.Error? err
local function finished(result, binary, shown, timeout)
  -- vim.system reports a command stopped for taking too long with exit code 124.
  if timeout and result.code == 124 then
    local message = ('`%s` did not finish within %d seconds'):format(shown, timeout)
    return nil, failure('failed', message)
  end
  return decode(result, binary, shown)
end

---Runs `orca-term <args> --json` in `cwd`, waits for it (`timeout` seconds at most, 10 when
---`nil`), and returns the JSON document it printed.
---@param args string[]
---@param cwd string
---@param timeout? integer
---@return table? document
---@return orca_term.Error? err
local function run(args, cwd, timeout)
  timeout = timeout or TIMEOUT_SECONDS
  local binary, find_err = M.find_binary()
  if not binary then
    return nil, find_err
  end
  local command, shown = command_line(binary, args)

  -- vim.system throws when the process cannot start, for example when `cwd` does not exist.
  local started, result = pcall(function()
    return vim.system(command, { cwd = cwd, text = true }):wait(timeout * 1000)
  end)
  if not started then
    return nil, failure('failed', ('could not run `%s` in %s: %s'):format(shown, cwd, result))
  end
  return finished(result, binary, shown, timeout)
end

---Runs `orca-term <args> --json` in `cwd` without waiting for it, and hands the document it
---printed, or the error, to `on_done` once it finishes, in Neovim's main loop. With no `timeout`
---(in seconds) there is no time limit: a setup may take minutes.
---@param args string[]
---@param cwd string
---@param on_done fun(document: table?, err: orca_term.Error?)
---@param timeout? integer
local function run_async(args, cwd, on_done, timeout)
  local binary, find_err = M.find_binary()
  if not binary then
    vim.schedule(function()
      on_done(nil, find_err)
    end)
    return
  end
  local command, shown = command_line(binary, args)
  local options = { cwd = cwd, text = true, timeout = timeout and timeout * 1000 }

  -- The exit callback runs outside the main loop, where most of Neovim's API is off limits.
  local started, problem = pcall(vim.system, command, options, function(result)
    vim.schedule(function()
      on_done(finished(result, binary, shown, timeout))
    end)
  end)
  if not started then
    local err = failure('failed', ('could not run `%s` in %s: %s'):format(shown, cwd, problem))
    vim.schedule(function()
      on_done(nil, err)
    end)
  end
end

---The fleet of the repository `cwd` is in: `orca-term worktree ls --json`.
---@param cwd string
---@return orca_term.Fleet? fleet
---@return orca_term.Error? err
function M.worktree_ls(cwd)
  return run({ 'worktree', 'ls' }, cwd)
end

---The fleet of the repository `cwd` is in, without waiting: `orca-term worktree ls --json`,
---handed to `on_done` in Neovim's main loop. It is stopped after 10 seconds, as `worktree_ls` is.
---@param cwd string
---@param on_done fun(fleet: orca_term.Fleet?, err: orca_term.Error?)
function M.worktree_ls_async(cwd, on_done)
  run_async({ 'worktree', 'ls' }, cwd, on_done, TIMEOUT_SECONDS)
end

---Creates the worktree `name` in the repository `cwd` is in, without waiting:
---`orca-term worktree new <name> --json`. The core runs the setup before it finishes, so
---`on_done` may come minutes later. A failed setup is a document whose `setup.status` is
---`failed`, not an error: the worktree exists.
---@param name string
---@param cwd string
---@param on_done fun(created: orca_term.Created?, err: orca_term.Error?)
function M.worktree_new(name, cwd, on_done)
  run_async({ 'worktree', 'new', name }, cwd, on_done)
end

---Runs the setup of the worktree at `path` again, without waiting:
---`orca-term worktree setup <path> --json`. A worktree in use fails as `busy`.
---@param path string
---@param cwd string
---@param on_done fun(set_up: orca_term.SetUp?, err: orca_term.Error?)
function M.worktree_setup(path, cwd, on_done)
  run_async({ 'worktree', 'setup', path }, cwd, on_done)
end

---Removes the worktree at `path`, without waiting: `orca-term worktree rm <path> --json`, with
---`--force` when `force` is true. Without it, a worktree with changes fails as `dirty`; a
---worktree in use fails as `busy` either way.
---@param path string
---@param force boolean
---@param cwd string
---@param on_done fun(removed: orca_term.Removed?, err: orca_term.Error?)
function M.worktree_rm(path, force, cwd, on_done)
  local args = { 'worktree', 'rm', path }
  if force then
    table.insert(args, '--force')
  end
  run_async(args, cwd, on_done)
end

---Starts the agent of the worktree at `path` in its holder, without waiting:
---`orca-term agent start <path> --rows R --cols C --json`, `size` being the agent's first terminal
---size. An agent already running is not an error: `holder.status` is then `already_running`. A
---worktree that is not ready fails as `other`, a missing sandbox as `no_sandbox`; the message says
---why and names the fix.
---@param path string
---@param size { rows: integer, cols: integer }
---@param cwd string
---@param on_done fun(started: orca_term.Started?, err: orca_term.Error?)
function M.agent_start(path, size, cwd, on_done)
  local rows, cols = tostring(size.rows), tostring(size.cols)
  run_async({ 'agent', 'start', path, '--rows', rows, '--cols', cols }, cwd, on_done)
end

---What `nvim register --json` and `nvim unregister --json` print: only the contract and the core's
---version.
---@class orca_term.Registered
---@field contract integer
---@field version string

---Registers the Neovim listening at `address` (its `v:servername`, the absolute path of a unix
---socket) with the repository `cwd` is in, without waiting:
---`orca-term nvim register <address> --json`. From then on the core fires
---`User OrcaTermWorktreeChanged` there after every change to a worktree. Registering again
---changes nothing; a relative address is refused as `usage`.
---@param address string
---@param cwd string
---@param on_done fun(registered: orca_term.Registered?, err: orca_term.Error?)
function M.nvim_register(address, cwd, on_done)
  run_async({ 'nvim', 'register', address }, cwd, on_done)
end

---Unregisters the Neovim listening at `address` from the repository `cwd` is in, and waits for it,
---1 second at most, since it runs as Neovim exits: `orca-term nvim unregister <address> --json`.
---An address that was not registered is not an error.
---@param address string
---@param cwd string
---@return orca_term.Registered? unregistered
---@return orca_term.Error? err
function M.nvim_unregister(address, cwd)
  return run({ 'nvim', 'unregister', address }, cwd, UNREGISTER_TIMEOUT_SECONDS)
end

---Attaches the current buffer, as a terminal, to the agent of the worktree at `path`:
---`orca-term agent attach <path> --no-detach-key`, run in `cwd`. The caller makes an empty buffer
---current first. No key detaches (Ctrl-\ is Neovim's terminal prefix): the attach ends when its
---terminal closes, which detaches and leaves the agent running, or when the agent ends. Returns
---the terminal's job id.
---@param path string
---@param cwd string
---@return integer? job
---@return orca_term.Error? err
function M.agent_attach(path, cwd)
  local binary, find_err = M.find_binary()
  if not binary then
    return nil, find_err
  end
  local command = { binary, 'agent', 'attach', path, '--no-detach-key' }
  local shown = table.concat(command, ' ')
  -- jobstart throws when `cwd` is not a directory, and returns 0 or -1 when it cannot start.
  local started, job = pcall(vim.fn.jobstart, command, { term = true, cwd = cwd })
  if not started or job <= 0 then
    local why = started and ('jobstart returned %d'):format(job) or tostring(job)
    return nil, failure('failed', ('could not run `%s` in %s: %s'):format(shown, cwd, why))
  end
  return job
end

return M
