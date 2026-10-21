-- orca-term's pull requests: when the client asks the core to refresh them (#87).
--
-- The core asks the GitHub CLI about each worktree's pull request and keeps the answer
-- (`orca-term pr refresh`, through `core.lua`; the client never runs gh), and `worktree ls` prints
-- it as each worktree's `pr`. This module decides when to ask, never blocking Neovim, as #22 ruled:
--
-- - when the sidebar opens or Neovim gains focus while it is open, for each worktree whose pull
--   request was last refreshed (or asked for) a minute ago or more (`MIN_AGE_MS`);
-- - when a turn of a worktree ends, for that worktree;
-- - every minute (`POLL_MS`), for the worktrees whose checks are pending, while the sidebar is open
--   and some are; the timer stops otherwise;
-- - and whenever the person asks: `R` in the sidebar for the worktree under the cursor,
--   `:OrcaTerm pr-refresh` (`<Plug>(OrcaTermPrRefresh)`) for every worktree.
--
-- After a refresh that changes what the sidebar shows (a pull request, its state or its checks),
-- the live fleet is read again (`live.read_soon()`), so the sidebar shows it even in a Neovim the
-- core cannot notify; one that changes nothing shown reads nothing. This module touches no window,
-- tab or buffer and starts no process (`cargo xtask client-boundary` checks it).

local core = require('orca-term.core')
local live = require('orca-term.live')

local M = {}

---How long after a worktree's pull request was refreshed opening or focusing the sidebar asks
---again, in milliseconds.
M.MIN_AGE_MS = 60 * 1000

---How often the pull requests whose checks are pending are refreshed, in milliseconds.
M.POLL_MS = 60 * 1000

---What this module holds between calls.
---@class orca_term.PrState
---@field asked table<string, integer> each worktree's path, and when this client last asked (ms)
---@field states table<string, string> each worktree's path, and its state in the fleet last seen
---@field fleet? orca_term.Fleet the fleet last seen while the sidebar is open
---@field cwd? string where the core runs
---@field timer? uv.uv_timer_t the timer refreshing pending checks, while it runs

---@type orca_term.PrState
local state = { asked = {}, states = {} }

---The time now, in milliseconds since the Unix epoch.
---@return integer
local function now_ms()
  return os.time() * 1000
end

---Whether the pull request of `worktree` is due for a refresh at `now`: neither the core's last
---answer (`fetched_at`) nor this client's last request is less than `MIN_AGE_MS` old.
---@param worktree orca_term.Worktree
---@param now integer milliseconds since the Unix epoch
---@return boolean
function M.due(worktree, now)
  local fetched = worktree.pr and worktree.pr.fetched_at or 0
  local last = math.max(fetched, state.asked[worktree.path] or 0)
  return now - last >= M.MIN_AGE_MS
end

---Whether the checks of the pull request of `worktree` are pending.
---@param worktree orca_term.Worktree
---@return boolean
function M.pending(worktree)
  return worktree.pr ~= nil and worktree.pr.checks == 'pending'
end

---The states of a turn in progress.
local IN_TURN = { working = true, waiting = true }

---The states a turn ends in.
local TURN_ENDS = { finished = true, failed = true, interrupted = true }

---Whether a turn ended between two readings of a worktree's state, `before` (`nil` when it was
---not seen) and `after`: it left working or waiting, or came to a turn's end from another state.
---@param before? string
---@param after string
---@return boolean
function M.turn_ended(before, after)
  if before == nil or before == after then
    return false
  end
  if IN_TURN[before] then
    return not IN_TURN[after]
  end
  return TURN_ENDS[after] == true
end

---Whether the core can ask about `worktree`: one whose directory is gone it refuses.
---@param worktree orca_term.Worktree
---@return boolean
local function askable(worktree)
  return worktree.state ~= 'broken'
end

---What the sidebar shows of a pull request, as one string: its number, state and checks, or
---nothing when there is no pull request (none, unavailable, or never asked about).
---@param pr? orca_term.Pr
---@return string
function M.shown(pr)
  if not (pr and pr.number) then
    return ''
  end
  return ('%d %s %s'):format(pr.number, pr.state or '', pr.checks or '')
end

---Whether what `refreshed` found changes what the sidebar shows of the fleet last read. A
---worktree that fleet does not hold (another repository's, or one removed since) changes nothing.
---@param refreshed orca_term.PrRefreshed
---@return boolean
local function shows_differently(refreshed)
  local before = {}
  for _, worktree in ipairs((live.fleet() or { worktrees = {} }).worktrees) do
    before[worktree.path] = M.shown(worktree.pr)
  end
  for _, worktree in ipairs(refreshed.worktrees) do
    local shown = before[worktree.path]
    if shown ~= nil and shown ~= M.shown(worktree.pr) then
      return true
    end
  end
  return false
end

---Asks the core, without waiting, to refresh the pull requests of `paths` (every worktree when
---empty), from `cwd`; then, when what it found changes what the sidebar shows, reads the live
---fleet again; and hands the core's answer to `on_done`.
---@param paths string[]
---@param cwd string
---@param on_done? fun(refreshed: orca_term.PrRefreshed?, err: orca_term.Error?)
function M.refresh(paths, cwd, on_done)
  local now = now_ms()
  for _, path in ipairs(paths) do
    state.asked[path] = now
  end
  core.pr_refresh(paths, cwd, function(refreshed, err)
    if refreshed then
      for _, worktree in ipairs(refreshed.worktrees) do
        state.asked[worktree.path] = now
      end
      if shows_differently(refreshed) then
        live.read_soon()
      end
    end
    if on_done then
      on_done(refreshed, err)
    end
  end)
end

---The paths of the worktrees of `fleet` for which `wanted(worktree)` holds, leaving out those the
---core cannot ask about.
---@param fleet orca_term.Fleet
---@param wanted fun(worktree: orca_term.Worktree): boolean
---@return string[]
local function paths_where(fleet, wanted)
  local paths = {}
  for _, worktree in ipairs(fleet.worktrees) do
    if askable(worktree) and wanted(worktree) then
      table.insert(paths, worktree.path)
    end
  end
  return paths
end

---Opening the sidebar, or Neovim gaining focus while it is open: refreshes, without waiting, every
---worktree of `fleet` that is due (`M.due`), from `cwd`.
---@param fleet? orca_term.Fleet
---@param cwd string
function M.refresh_due(fleet, cwd)
  if not fleet then
    return
  end
  local now = now_ms()
  local paths = paths_where(fleet, function(worktree)
    return M.due(worktree, now)
  end)
  if #paths > 0 then
    M.refresh(paths, cwd)
  end
end

---Stops the timer refreshing pending checks, if it runs.
local function stop_timer()
  if state.timer then
    state.timer:stop()
    state.timer:close()
    state.timer = nil
  end
end

---One tick of the timer: refreshes the worktrees of the fleet last seen whose checks are pending.
local function tick()
  local fleet = state.fleet
  if not (fleet and state.cwd) then
    return
  end
  local paths = paths_where(fleet, M.pending)
  if #paths > 0 then
    M.refresh(paths, state.cwd)
  end
end

---Starts the timer while some worktree of the fleet last seen has pending checks, and stops it
---otherwise. There is one timer at most.
local function update_timer()
  local fleet = state.fleet
  local wanted = fleet ~= nil and #paths_where(fleet, M.pending) > 0
  if not wanted then
    stop_timer()
    return
  end
  if state.timer then
    return
  end
  local timer = assert(vim.uv.new_timer())
  timer:start(M.POLL_MS, M.POLL_MS, vim.schedule_wrap(tick))
  state.timer = timer
end

---Every fleet the sidebar shows, while it is open: refreshes, without waiting, each worktree whose
---turn ended since the fleet before, and runs the timer while some checks are pending.
---@param fleet? orca_term.Fleet
---@param cwd string
function M.follow(fleet, cwd)
  if not fleet then
    return
  end
  local ended = paths_where(fleet, function(worktree)
    return M.turn_ended(state.states[worktree.path], worktree.state)
  end)
  state.states = {}
  for _, worktree in ipairs(fleet.worktrees) do
    state.states[worktree.path] = worktree.state
  end
  state.fleet, state.cwd = fleet, cwd
  if #ended > 0 then
    M.refresh(ended, cwd)
  end
  update_timer()
end

---Whether the timer refreshing pending checks runs.
---@return boolean
function M.polling()
  return state.timer ~= nil
end

---The sidebar closed: the timer stops and the fleet seen is forgotten.
function M.stop()
  stop_timer()
  state.fleet, state.cwd, state.states = nil, nil, {}
end

---`:OrcaTerm pr-refresh`: refreshes the pull request of every worktree of the repository Neovim's
---current directory is in (the fleet the sidebar shows), without waiting, then says how many it
---refreshed, or why it could not.
function M.refresh_all()
  M.refresh({}, vim.fn.getcwd(), function(refreshed, err)
    if not refreshed then
      vim.notify(err and err.message or 'orca-term: pr refresh failed', vim.log.levels.ERROR)
      return
    end
    local count = #refreshed.worktrees
    vim.notify(
      ('orca-term: refreshed the pull request of %d worktree%s'):format(
        count,
        count == 1 and '' or 's'
      ),
      vim.log.levels.INFO
    )
  end)
end

---Forgets everything, for the specs.
function M._reset()
  M.stop()
  state.asked = {}
end

return M
