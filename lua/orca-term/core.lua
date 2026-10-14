-- orca-term's Neovim client: the link to the core, the `orca-term` binary.
--
-- This is the only module that starts a process (`cargo xtask client-boundary` checks it). It
-- finds the binary, runs one of its commands with `--json`, waits for it, and checks that the
-- binary speaks this client's contract. The client never works the fleet out by itself: it shows
-- what the core prints.
--
-- Every function returns its result, or `nil` and an `orca_term.Error`. None of them throws.

---A failure, written for a person, naming the fix.
---@class orca_term.Error
---What failed: the core's own `error.kind` (`busy`, `other`), or one of this client's:
---`no_binary` (no core binary found), `contract` (the core speaks another contract) or `failed`
---(the core could not start, did not finish in time, or printed no JSON).
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
---@field state string `no_agent`, `idle`, `setting_up`, `not_ready` or `broken`
---@field detail? string why the worktree is not ready or broken
---@field agent? string the worktree's agent, `claude-code`; `nil` when it has none
---@field holder boolean whether the agent's holder is alive right now

---The fleet, as `worktree ls --json` prints it.
---@class orca_term.Fleet
---@field contract integer
---@field version string the core's version
---@field primary orca_term.Primary
---@field worktrees orca_term.Worktree[]

local M = {}

---The JSON contract this client speaks. A core printing another one is refused.
M.CONTRACT = 1

---How long a core command may take before it is stopped.
local TIMEOUT_SECONDS = 10

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

---Runs `orca-term <args> --json` in `cwd`, waits for it, and returns the JSON document it printed.
---@param args string[]
---@param cwd string
---@return table? document
---@return orca_term.Error? err
local function run(args, cwd)
  local binary, find_err = M.find_binary()
  if not binary then
    return nil, find_err
  end
  local command = vim.list_extend({ binary }, args)
  table.insert(command, '--json')
  local shown = table.concat(command, ' ')

  -- vim.system throws when the process cannot start, for example when `cwd` does not exist.
  local started, result = pcall(function()
    return vim.system(command, { cwd = cwd, text = true }):wait(TIMEOUT_SECONDS * 1000)
  end)
  if not started then
    return nil, failure('failed', ('could not run `%s` in %s: %s'):format(shown, cwd, result))
  end
  -- vim.system reports a command stopped for taking too long with exit code 124.
  if result.code == 124 then
    local message = ('`%s` did not finish within %d seconds'):format(shown, TIMEOUT_SECONDS)
    return nil, failure('failed', message)
  end

  -- `luanil` turns JSON `null` into Lua `nil`, so a missing branch reads as `nil`.
  local decoded, document = pcall(vim.json.decode, result.stdout or '', {
    luanil = { object = true, array = true },
  })
  if not decoded or type(document) ~= 'table' then
    local stderr = vim.trim(result.stderr or '')
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

---The fleet of the repository `cwd` is in: `orca-term worktree ls --json`.
---@param cwd string
---@return orca_term.Fleet? fleet
---@return orca_term.Error? err
function M.worktree_ls(cwd)
  return run({ 'worktree', 'ls' }, cwd)
end

return M
