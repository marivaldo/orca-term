-- orca-term's Neovim client: the link to the core, the `orca-term` binary.
--
-- This is the only module that starts a process (`cargo xtask client-boundary` checks it). It
-- finds the binary, runs one of its commands with `--json`, and checks that the binary speaks this
-- client's contract. `worktree ls` is waited for, or read without waiting when the core says a
-- worktree changed; the commands that may run a setup (`worktree new`, `setup`, `rm`), `agent
-- start`, the marks `worktree seen` and `unseen`, `worktree review`, `worktree detail`, the note
-- commands (`note add`, `ls`, `edit`, `rm`, `send`), `turn ls`, the session commands (`session ls`,
-- `resume`, `new`) and `pr refresh` are not waited for, and hand their
-- result to a callback in Neovim's main loop. `nvim register` tells the core where this Neovim listens, so it
-- says when a worktree changes; `nvim unregister` runs as Neovim exits and is
-- waited for, briefly. `doctor` is waited for too, for `:checkhealth orca-term` (`health.lua`).
-- `agent attach` is the one command run without `--json`: it runs in a terminal buffer the caller
-- made current, for the person to use, until it detaches or the agent ends. The client never works
-- the fleet out by itself: it shows what the core prints.
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
---With `untrusted` (what a committed `orca-term.yaml` asks for is not approved): `setup` (its
---setup script did not run) or `sandbox` (its extra writable paths: the agent did not start).
---@field what? string
---With `untrusted` and `what == 'setup'`: the script's text.
---@field script? string
---With `untrusted` and `what == 'sandbox'`: the writable paths, as the file writes them.
---@field writable? string[]
---With `untrusted`: the committed file they come from, absolute.
---@field source? string

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
---Whether its latest turn ended after the person last looked, or the person marked it unseen
---(`CONTEXT.md`, **Unseen**)
---@field unseen boolean
---@field notes integer how many notes wait to be sent to its agent
---How many tool calls its agent's permission rules refused in its latest turn (`CONTEXT.md`,
---**Denial**)
---@field denials integer
---The git operation it is in the middle of, `merge`, `rebase`, `cherry-pick` or `revert`; `nil`
---when there is none. Never a state.
---@field operation? string
---What `pr refresh` last found about its pull request; `nil` when it never ran for the worktree
---(only `worktree ls` and `worktree detail` print it)
---@field pr? orca_term.Pr
---When its current session was last active, in milliseconds since the Unix epoch; `nil` when it
---has no current session or nothing says (only `worktree ls` and `worktree detail` print it)
---@field last_activity? integer
---How many files it changed against its merge base with the default branch, committed or not,
---untracked files git does not ignore included; `nil` when it cannot be counted, or it is broken
---or setting up (only `worktree ls` and `worktree detail` print it)
---@field changed_files? integer

---What `pr refresh` last found about a worktree's pull request, as `worktree ls --json` prints it:
---the pull request (`number` set), `none` when its branch has none, or `unavailable`, why the
---GitHub CLI could not say. Only the fields of one of the three are set.
---@class orca_term.Pr
---@field fetched_at integer when it was found, in milliseconds since the Unix epoch
---@field number? integer
---@field title? string
---@field state? string `open`, `draft`, `merged` or `closed`
---@field url? string
---@field checks? string `passing`, `failing`, `pending` or `none`
---@field none? boolean `true` when the branch has no pull request
---@field unavailable? string why nothing is known, such as `GitHub CLI is unavailable`

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
---With `--agent`, what starting the agent did once the setup succeeded; `nil` otherwise
---@field started? orca_term.Started

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
---@field agent string the agent started, `claude-code` or `opencode`
---@field holder orca_term.Holder
---@field resumed? string the session resumed on a cold start; `nil` for a new session

---What `agent swap --json` prints: what starting the new agent did, the agent the worktree had
---before and whether its holder was stopped.
---@class orca_term.Swapped: orca_term.Started
---@field previous? string the agent swapped out, `nil` when the worktree had none
---@field stopped boolean

local M = {}

---The JSON contract this client speaks. A core printing another one is refused.
M.CONTRACT = 1

---How long a core command that is waited for may take before it is stopped.
local TIMEOUT_SECONDS = 10

---How long `nvim unregister` may take: it runs as Neovim exits, which it must not hold up.
local UNREGISTER_TIMEOUT_SECONDS = 1

---The repository this client is loaded from: three directories up from this file,
---`<root>/lua/orca-term/core.lua`. The build hook (`build.lua`) installs a release's core binary
---at `<root>/bin/orca-term`, and `cargo build` puts it under `<root>/target/`.
local ROOT = vim.fn.fnamemodify(debug.getinfo(1, 'S').source:sub(2), ':p:h:h:h')

---@param kind string
---@param message string
---@return orca_term.Error
local function failure(kind, message)
  return { kind = kind, message = 'orca-term: ' .. message }
end

---The core binary: `binary` from setup() when set; else the release binary the build hook
---installed at `bin/orca-term` in this repository; else the one `cargo build --release` or
---`cargo build` made there; else `orca-term` on PATH.
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

  local installed = {
    ROOT .. '/bin/orca-term',
    ROOT .. '/target/release/orca-term',
    ROOT .. '/target/debug/orca-term',
  }
  for _, path in ipairs(installed) do
    if vim.fn.executable(path) == 1 then
      return path
    end
  end

  local on_path = vim.fn.exepath('orca-term')
  if on_path ~= '' then
    return on_path
  end

  local message = 'no core binary found; run `nvim -l build.lua` in '
    .. ROOT
    .. ' to download or build it, or set `binary` in setup()'
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
---it stands for: the core's own `error` (unless `keep_error`, which returns the document with its
---`error` inside, for `doctor`), a usage error, output that is not JSON, or another contract.
---@param result vim.SystemCompleted
---@param binary string
---@param shown string the command line, for messages
---@param keep_error? boolean
---@return table? document
---@return orca_term.Error? err
local function decode(result, binary, shown, keep_error)
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

  if document.error and not keep_error then
    local err = failure(document.error.kind, document.error.message)
    err.what = document.error.what
    err.script = document.error.script
    err.writable = document.error.writable
    err.source = document.error.source
    return nil, err
  end
  return document
end

---The document a finished core command printed, or the error it stands for, as `decode` reads
---it; a command stopped for taking longer than `timeout` seconds is an error of its own.
---@param result vim.SystemCompleted
---@param binary string
---@param shown string the command line, for messages
---@param timeout? integer the time limit it ran with, in seconds; `nil` when it had none
---@param keep_error? boolean return a document carrying an `error` as it is (`decode`)
---@return table? document
---@return orca_term.Error? err
local function finished(result, binary, shown, timeout, keep_error)
  -- vim.system reports a command stopped for taking too long with exit code 124.
  if timeout and result.code == 124 then
    local message = ('`%s` did not finish within %d seconds'):format(shown, timeout)
    return nil, failure('failed', message)
  end
  return decode(result, binary, shown, keep_error)
end

---Runs `orca-term <args> --json` in `cwd`, waits for it (`timeout` seconds at most, 10 when
---`nil`), and returns the JSON document it printed.
---@param args string[]
---@param cwd string
---@param timeout? integer
---@param keep_error? boolean return a document carrying an `error` as it is (`decode`)
---@return table? document
---@return orca_term.Error? err
local function run(args, cwd, timeout, keep_error)
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
  return finished(result, binary, shown, timeout, keep_error)
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
---`failed`, not an error: the worktree exists. A setup script from a committed `orca-term.yaml`
---that the person has not approved does not run: the worktree exists, not ready, and the error is
---`untrusted`, carrying the `script` and its `source`. With an `agent` (`claude-code` or
---`opencode`) it passes `--agent <agent> --rows R --cols C`, `size` being the agent's first
---terminal size: once the setup succeeded the core records that agent and starts it, and the
---document carries `started`. A failed setup starts nothing. A start the core refuses is
---`agent start`'s error (`no_sandbox`, `prerequisite`, `untrusted` with `what` `sandbox`); the
---worktree then exists, ready, with the agent recorded.
---@param name string
---@param agent? string
---@param size { rows: integer, cols: integer }
---@param cwd string
---@param on_done fun(created: orca_term.Created?, err: orca_term.Error?)
function M.worktree_new(name, agent, size, cwd, on_done)
  local args = { 'worktree', 'new', name }
  if agent then
    vim.list_extend(args, { '--agent', agent, '--rows', tostring(size.rows) })
    vim.list_extend(args, { '--cols', tostring(size.cols) })
  end
  run_async(args, cwd, on_done)
end

---Runs the setup of the worktree at `path` (or named `path`) again, without waiting:
---`orca-term worktree setup <path> --json`, with `--approve-setup` when `approve` is true, which
---approves a setup script from a committed `orca-term.yaml` before running it. Without it, such a
---script the person has not approved fails as `untrusted`. A worktree in use fails as `busy`.
---@param path string
---@param approve boolean
---@param cwd string
---@param on_done fun(set_up: orca_term.SetUp?, err: orca_term.Error?)
function M.worktree_setup(path, approve, cwd, on_done)
  local args = { 'worktree', 'setup', path }
  if approve then
    table.insert(args, '--approve-setup')
  end
  run_async(args, cwd, on_done)
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
---size, with `--fresh` when `fresh` is true. With no live holder the core resumes the session the
---worktree recorded (`resumed`), unless `fresh`; one it can no longer resume fails as
---`session_gone`. An agent already running is not an error: `holder.status` is then
---`already_running`. A worktree that is not ready fails as `other`, a missing sandbox as
---`no_sandbox`; the message says why and names the fix. Extra writable paths from a committed
---`orca-term.yaml` not approved fail as `untrusted` (`what` is `sandbox`, with the `writable`
---paths and their `source`), unless `approve` passes `--approve-sandbox`, which approves them.
---With an `agent` it passes `--agent <agent>`, which starts that agent and records it.
---@param path string
---@param size { rows: integer, cols: integer }
---@param fresh boolean
---@param approve boolean
---@param cwd string
---@param on_done fun(started: orca_term.Started?, err: orca_term.Error?)
---@param agent? string
function M.agent_start(path, size, fresh, approve, cwd, on_done, agent)
  local rows, cols = tostring(size.rows), tostring(size.cols)
  local args = { 'agent', 'start', path, '--rows', rows, '--cols', cols }
  if agent then
    vim.list_extend(args, { '--agent', agent })
  end
  if fresh then
    table.insert(args, '--fresh')
  end
  if approve then
    table.insert(args, '--approve-sandbox')
  end
  run_async(args, cwd, on_done)
end

---Swaps the agent of the worktree at `path` for `agent` (`claude-code` or `opencode`), without
---waiting: `orca-term agent swap <path> <agent> --rows R --cols C --json`, `size` being the new
---agent's first terminal size. The core stops the agent running there and starts the other, which
---resumes only a session it recorded itself. An agent in a turn (working or waiting) fails as
---`busy`, as does a worktree in use; extra writable paths from a committed `orca-term.yaml` not
---approved fail as `untrusted`, unless `approve` passes `--approve-sandbox`; the other refusals
---are `agent start`'s.
---@param path string
---@param agent string
---@param size { rows: integer, cols: integer }
---@param approve boolean
---@param cwd string
---@param on_done fun(swapped: orca_term.Swapped?, err: orca_term.Error?)
function M.agent_swap(path, agent, size, approve, cwd, on_done)
  local rows, cols = tostring(size.rows), tostring(size.cols)
  local args = { 'agent', 'swap', path, agent, '--rows', rows, '--cols', cols }
  if approve then
    table.insert(args, '--approve-sandbox')
  end
  run_async(args, cwd, on_done)
end

---What `worktree seen --json` and `worktree unseen --json` print: the worktree and its mark now.
---@class orca_term.Marked
---@field contract integer
---@field version string
---@field name string
---@field path string
---@field unseen boolean

---Clears the unseen mark of the worktree at `path`, without waiting:
---`orca-term worktree seen <path> --json`. It takes no lock, so it is never `busy`; the primary
---checkout is refused. The core then tells the running Neovims the worktree changed.
---@param path string
---@param cwd string
---@param on_done fun(marked: orca_term.Marked?, err: orca_term.Error?)
function M.worktree_seen(path, cwd, on_done)
  run_async({ 'worktree', 'seen', path }, cwd, on_done, TIMEOUT_SECONDS)
end

---Sets the unseen mark of the worktree at `path`, without waiting:
---`orca-term worktree unseen <path> --json`, as `worktree_seen` clears it.
---@param path string
---@param cwd string
---@param on_done fun(marked: orca_term.Marked?, err: orca_term.Error?)
function M.worktree_unseen(path, cwd, on_done)
  run_async({ 'worktree', 'unseen', path }, cwd, on_done, TIMEOUT_SECONDS)
end

---What `worktree review --json` prints: the worktree (now seen), the review's base and the branch
---it was found against.
---@class orca_term.Review
---@field contract integer
---@field version string
---@field worktree orca_term.Worktree the worktree, its `unseen` now `false`
---@field base string the review's base: the commit the worktree forked from, a full commit id
---@field against string the default branch the base was found against, `main` or `origin/main`

---Starts the review of the worktree `target` (its name or its path), without waiting:
---`orca-term worktree review <target> --json`. The core finds the review's base (`git merge-base
---<default branch> HEAD` in the worktree), marks the worktree seen and tells the running Neovims it
---changed. The primary checkout, an unknown worktree, a worktree whose directory is gone and a
---history unrelated to the default branch are refused, and then nothing is marked.
---@param target string
---@param cwd string
---@param on_done fun(review: orca_term.Review?, err: orca_term.Error?)
function M.worktree_review(target, cwd, on_done)
  run_async({ 'worktree', 'review', target }, cwd, on_done, TIMEOUT_SECONDS)
end

---A tool call the agent's permission rules refused, as `worktree detail --json` prints it.
---@class orca_term.Denial
---@field tool string the tool, such as `Write` or `Bash`
---@field text string the call as the activity shows it, such as `Bash: FOO=bar ls`

---A worktree's setup, as `worktree detail --json` prints it.
---@class orca_term.SetupDetail
---@field status string `running`, `interrupted`, `succeeded`, `failed` or `not_approved`
---@field log string the setup log's path
---@field tail string[] the log's last lines, terminal control sequences taken out

---How far a worktree's branch is from the local default branch (`CONTEXT.md`, **Drift**).
---@class orca_term.Drift
---@field against string the local default branch, such as `main`
---@field ahead integer the commits on the worktree's side only
---@field behind integer the commits on the default branch's side only

---What `worktree detail --json` prints: the worktree and everything about it the sidebar has no
---room for.
---@class orca_term.Detail
---@field contract integer
---@field version string
---@field worktree orca_term.Worktree
---@field detail? string why it is in its state, whole (a failed setup, a broken worktree, a failed
---turn's error); `nil` when there is nothing to say
---@field denials orca_term.Denial[] the latest turn's denials, in order
---@field setup? orca_term.SetupDetail `nil` when the worktree has no setup
---@field drift? orca_term.Drift `nil` when there is no local default branch
---@field operation_step? orca_term.OperationStep the step of the rebase in progress; `nil` for any
---other operation, for none, or when git records no step

---The step a rebase in progress is at: `current` of `total` commits.
---@class orca_term.OperationStep
---@field current integer
---@field total integer

---The detail of the worktree `target` (its name or its path), without waiting:
---`orca-term worktree detail <target> --json`. Drift is computed on request against the local
---default branch, never fetched. The primary checkout and an unknown worktree are refused.
---@param target string
---@param cwd string
---@param on_done fun(detail: orca_term.Detail?, err: orca_term.Error?)
function M.worktree_detail(target, cwd, on_done)
  run_async({ 'worktree', 'detail', target }, cwd, on_done, TIMEOUT_SECONDS)
end

---One worktree `pr refresh --json` asked about.
---@class orca_term.RefreshedPr
---@field name string
---@field path string
---@field pr orca_term.Pr

---What `pr refresh --json` prints: each worktree it asked about.
---@class orca_term.PrRefreshed
---@field contract integer
---@field version string
---@field worktrees orca_term.RefreshedPr[]

---How long `pr refresh` may take: the core gives the GitHub CLI 10 seconds a worktree, every
---worktree at once.
local PR_REFRESH_TIMEOUT_SECONDS = 30

---Asks the core to refresh the pull request of each worktree of `targets` (paths or names), or of
---every worktree when it is empty, without waiting: `orca-term pr refresh [<target>...] --json`.
---The core runs the GitHub CLI; the client never does.
---@param targets string[]
---@param cwd string
---@param on_done fun(refreshed: orca_term.PrRefreshed?, err: orca_term.Error?)
function M.pr_refresh(targets, cwd, on_done)
  local args = vim.list_extend({ 'pr', 'refresh' }, targets)
  run_async(args, cwd, on_done, PR_REFRESH_TIMEOUT_SECONDS)
end

---Where a note is, as the note commands print it: on one line (`scope` `line`, with `line`), on a
---range of lines (`lines`, from `start` to `["end"]`, both included) or on the whole file (`file`).
---Lines are numbered from 1.
---@class orca_term.NoteAnchor
---@field scope string `line`, `lines` or `file`
---@field line? integer
---@field start? integer
---@field ["end"]? integer

---A pending note, as the note commands print it.
---@class orca_term.Note
---@field id integer never reused in its worktree
---@field path string the file, relative to the worktree's root, `/`-separated
---@field anchor orca_term.NoteAnchor where the note was made
---Where its lines are in the file on disk now: the whole file when its line is gone or was edited
---@field current orca_term.NoteAnchor
---@field body string what the note says to the agent
---@field rev integer bumped by every edit
---@field created integer when it was made, in milliseconds since the epoch

---What `note add --json` and `note edit --json` print: the worktree and the note.
---@class orca_term.NoteWritten
---@field contract integer
---@field version string
---@field name string the worktree's name
---@field path string the worktree's path
---@field note orca_term.Note

---What `note ls --json` prints: the worktree and its pending notes, in id order.
---@class orca_term.NoteList
---@field contract integer
---@field version string
---@field name string
---@field path string
---@field notes orca_term.Note[]

---What `note rm --json` prints: the worktree and the id of the note removed.
---@class orca_term.NoteRemoved
---@field contract integer
---@field version string
---@field name string
---@field path string
---@field id integer

---What `note send --json` prints: the prompt typed into the agent, the notes it carried, and those
---of them kept pending because they were edited while it was sent.
---@class orca_term.NoteSent
---@field contract integer
---@field version string
---@field name string
---@field path string
---@field prompt string
---@field sent integer[]
---@field kept integer[]

---Attaches a note saying `body` to the file at `path` (relative to the worktree's root) of the
---worktree `target` (its name or its path), without waiting:
---`orca-term note add <target> <path> --body=<body> --line N --json` for one line (`lines` of two
---equal numbers), `--lines S-E` for a range, or neither for the whole file (`lines` `nil`). The core refuses lines outside the file on disk and an
---empty body, then tells the running Neovims the worktree changed.
---@param target string
---@param path string
---@param lines? integer[] the first and the last line, from 1
---@param body string
---@param cwd string
---@param on_done fun(written: orca_term.NoteWritten?, err: orca_term.Error?)
function M.note_add(target, path, lines, body, cwd, on_done)
  local args = { 'note', 'add', target, path, '--body=' .. body }
  if lines and lines[1] == lines[2] then
    vim.list_extend(args, { '--line', tostring(lines[1]) })
  elseif lines then
    vim.list_extend(args, { '--lines', ('%d-%d'):format(lines[1], lines[2]) })
  end
  run_async(args, cwd, on_done, TIMEOUT_SECONDS)
end

---The pending notes of the worktree `target`, without waiting: `orca-term note ls <target> --json`,
---each with where its lines are in the file on disk now.
---@param target string
---@param cwd string
---@param on_done fun(listed: orca_term.NoteList?, err: orca_term.Error?)
function M.note_ls(target, cwd, on_done)
  run_async({ 'note', 'ls', target }, cwd, on_done, TIMEOUT_SECONDS)
end

---Replaces what the note `id` of the worktree `target` says, without waiting:
---`orca-term note edit <target> <id> --body=<body> --json`.
---@param target string
---@param id integer
---@param body string
---@param cwd string
---@param on_done fun(written: orca_term.NoteWritten?, err: orca_term.Error?)
function M.note_edit(target, id, body, cwd, on_done)
  run_async(
    { 'note', 'edit', target, tostring(id), '--body=' .. body },
    cwd,
    on_done,
    TIMEOUT_SECONDS
  )
end

---Removes the note `id` of the worktree `target` without sending it, without waiting:
---`orca-term note rm <target> <id> --json`.
---@param target string
---@param id integer
---@param cwd string
---@param on_done fun(removed: orca_term.NoteRemoved?, err: orca_term.Error?)
function M.note_rm(target, id, cwd, on_done)
  run_async({ 'note', 'rm', target, tostring(id) }, cwd, on_done, TIMEOUT_SECONDS)
end

---Types the pending notes `ids` of the worktree `target` (every pending note when `ids` is empty)
---into its running agent as one prompt, then consumes them, without waiting:
---`orca-term note send <target> [<id>...] --json`. With no running agent nothing is typed nor
---consumed, and it fails as `no_holder`.
---@param target string
---@param ids integer[]
---@param cwd string
---@param on_done fun(sent: orca_term.NoteSent?, err: orca_term.Error?)
function M.note_send(target, ids, cwd, on_done)
  local args = { 'note', 'send', target }
  for _, id in ipairs(ids) do
    table.insert(args, tostring(id))
  end
  run_async(args, cwd, on_done, TIMEOUT_SECONDS)
end

---One turn of a worktree's agent, as `turn ls --json` prints it (`CONTEXT.md`, **Turn**). A turn
---from before orca-term took snapshots has `nil` for what it lacks.
---@class orca_term.Turn
---@field number integer the turn's number in its worktree, from 1
---@field agent? string the agent that ran it, such as `claude`
---@field session? string
---@field prompt string the prompt's first line
---@field started_at? integer when it started, in milliseconds since the epoch
---@field ended_at? integer when it ended; `nil` while it is open
---@field outcome? string `finished`, `failed` or `interrupted` once it ended; `nil` while open
---@field phase? string `working` or `waiting` while it is open, else `nil`
---Whether the agent reported its end (`false` for an interrupted turn, whose diff may include
---edits made after the interruption); `nil` while open and for a turn from before the snapshots
---@field end_observed? boolean
---@field begin? string the snapshot it began on, a full commit id
---@field ["end"]? string the snapshot it ended on, a full commit id; `nil` while it is open
---@field denials integer how many tool calls its agent was denied during it

---What `turn ls --json` prints: the worktree and its turns, oldest first, the open one last.
---@class orca_term.TurnList
---@field contract integer
---@field version string
---@field name string
---@field path string
---@field turns orca_term.Turn[]

---The turns of the worktree `target` (its name or its path), without waiting:
---`orca-term turn ls <target> --json`, from its turn log. It only reads. A worktree whose
---directory is gone is refused.
---@param target string
---@param cwd string
---@param on_done fun(listed: orca_term.TurnList?, err: orca_term.Error?)
function M.turn_ls(target, cwd, on_done)
  run_async({ 'turn', 'ls', target }, cwd, on_done, TIMEOUT_SECONDS)
end

---One session of a worktree, as `session ls --json` prints it.
---@class orca_term.Session
---@field id string the agent's own id for it
---@field agent string the agent it belongs to, `claude-code` or `opencode`
---@field first_prompt? string its first turn's prompt, first line; `nil` before its first turn
---@field turns integer how many turns it holds
---@field last_activity? integer when it was last active, in milliseconds since the epoch
---@field current boolean whether it is the worktree's current session

---What `session ls --json` prints: the worktree and its sessions, the most recently active first.
---@class orca_term.SessionList
---@field contract integer
---@field version string
---@field worktree orca_term.Worktree
---@field sessions orca_term.Session[]

---What `session resume --json` prints: `agent swap`'s document for the session's agent, and the
---session now current.
---@class orca_term.Resumed: orca_term.Swapped
---@field session string

---What `session new --json` prints: the worktree, the session discarded, and what restarting a
---running agent on a new session did.
---@class orca_term.Discarded
---@field worktree orca_term.Worktree
---@field discarded? string the session that was current, `nil` when there was none
---@field started? orca_term.Started `nil` when the agent was not running

---The sessions of the worktree `target` (its name or its path), without waiting:
---`orca-term session ls <target> --json`. It only reads.
---@param target string
---@param cwd string
---@param on_done fun(listed: orca_term.SessionList?, err: orca_term.Error?)
function M.session_ls(target, cwd, on_done)
  run_async({ 'session', 'ls', target }, cwd, on_done, TIMEOUT_SECONDS)
end

---Makes the session `id` of the worktree at `path` its current one and restarts its agent on it,
---without waiting: `orca-term session resume <path> <id> --rows R --cols C --json`, `size` being the
---agent's first terminal size. A session of the other agent swaps the worktree's agent. An agent
---in a turn fails as `busy`; a session that cannot be resumed as `session_gone`; extra writable
---paths from a committed `orca-term.yaml` not approved as `untrusted`, unless `approve` passes
---`--approve-sandbox`; the other refusals are `agent start`'s.
---@param path string
---@param id string
---@param size { rows: integer, cols: integer }
---@param approve boolean
---@param cwd string
---@param on_done fun(resumed: orca_term.Resumed?, err: orca_term.Error?)
function M.session_resume(path, id, size, approve, cwd, on_done)
  local rows, cols = tostring(size.rows), tostring(size.cols)
  local args = { 'session', 'resume', path, id, '--rows', rows, '--cols', cols }
  if approve then
    table.insert(args, '--approve-sandbox')
  end
  run_async(args, cwd, on_done)
end

---Discards the current session of the worktree at `path`, without waiting: `orca-term session new
---<path> --rows R --cols C --json`. A running agent is restarted on a new session (`started`);
---otherwise its next start begins one. An agent in a turn fails as `busy`; the restart refuses as
---`agent start` does, extra writable paths not approved as `untrusted` unless `approve` passes
---`--approve-sandbox`.
---@param path string
---@param size { rows: integer, cols: integer }
---@param approve boolean
---@param cwd string
---@param on_done fun(discarded: orca_term.Discarded?, err: orca_term.Error?)
function M.session_new(path, size, approve, cwd, on_done)
  local rows, cols = tostring(size.rows), tostring(size.cols)
  local args = { 'session', 'new', path, '--rows', rows, '--cols', cols }
  if approve then
    table.insert(args, '--approve-sandbox')
  end
  run_async(args, cwd, on_done)
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

---One check of `doctor --json`.
---@class orca_term.Check
---@field name string `git`, `sandbox`, `claude`, `opencode` or `contract`
---@field status string `ok`, `warn`, `missing`, `too_old` or `info`
---@field found? string what was found, such as a version
---@field need? string what orca-term needs, such as the oldest version
---@field message string the report's line, naming the fix when there is one

---What `doctor --json` prints: every check, and the `prerequisite` error when one is missing or
---too old.
---@class orca_term.Report
---@field contract integer
---@field version string the core's version
---@field checks orca_term.Check[]
---@field error? { kind: string, message: string }

---The core's report on the programs it needs, waited for (10 seconds at most), for
---`:checkhealth orca-term` only: `orca-term doctor --json`, run in Neovim's current directory
---(doctor needs no repository). A report with a missing or too old program is still returned,
---its `error` inside; an error is returned only when there is no report to show.
---@return orca_term.Report? report
---@return orca_term.Error? err
function M.doctor()
  return run({ 'doctor' }, vim.fn.getcwd(), TIMEOUT_SECONDS, true)
end

return M
