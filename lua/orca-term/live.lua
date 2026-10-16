-- orca-term's live fleet: the fleet last read, kept up to date for whoever subscribes to it.
--
-- A subscriber (the sidebar, the status strip) gives a name and a function, which is called with
-- the fleet after every read that is kept. The first subscription makes the autocommands below;
-- every subscription registers this Neovim with the core (`nvim register`), which then fires
-- `User OrcaTermWorktreeChanged` here after every change to a worktree. That event reads the
-- fleet without waiting, 100 ms later so a burst of events makes one read, one read at a time:
-- events during a read make one more read after it. Every subscriber gets the fleet of that one
-- read. A read that ends after a newer one started is dropped, so a subscriber never goes back to
-- an older fleet. Neovim gaining focus reads the fleet again, waiting for it, and registers again.
--
-- Unsubscribing unregisters nothing: Neovim unregisters as it exits. While nobody is subscribed
-- the events and the focus are ignored, and nothing is read. The fleet is never polled.
--
-- This module touches no window, tab or buffer and starts no process (`cargo xtask
-- client-boundary` checks it): it reads the fleet and registers through `core.lua`.

local core = require('orca-term.core')

local M = {}

---The `User` autocommand the core fires after every change to a worktree.
M.EVENT = 'OrcaTermWorktreeChanged'

---How long after the core says a worktree changed the fleet is read, in milliseconds. The events of
---a burst (a hook for every tool an agent runs) arrive within it and make one read.
M.DELAY_MS = 100

---Where this Neovim registered with the core: its address and the directory the core ran in.
---@class orca_term.Registration
---@field address string
---@field cwd string

---What the live fleet holds between calls.
---@class orca_term.LiveState
---@field subscribers table<string, fun(fleet: orca_term.Fleet?, err: orca_term.Error?)> by name
---@field fleet? orca_term.Fleet the fleet last kept, `nil` before the first read or when it failed
---@field error? orca_term.Error why the fleet last kept could not be read
---@field group? integer the autocommand group, made at the first subscription, never deleted
---@field timer? uv.uv_timer_t the timer waiting `DELAY_MS` to read the fleet after an event
---@field reading boolean whether a read started by an event is running
---@field again boolean whether an event came during that read, so one more read follows it
---How many reads of the fleet have started; a read that ends after a newer one started is dropped.
---@field reads integer
---@field registered orca_term.Registration[] every registration, undone as Neovim exits

---@type orca_term.LiveState
local state = { subscribers = {}, reading = false, again = false, reads = 0, registered = {} }

---Whether anyone is subscribed.
---@return boolean
local function subscribed()
  return next(state.subscribers) ~= nil
end

---Keeps the fleet (or why it could not be read) and hands it to every subscriber, in the order of
---their names.
---@param fleet? orca_term.Fleet
---@param err? orca_term.Error
local function keep(fleet, err)
  state.fleet, state.error = fleet, err
  -- The names are taken first, so a subscriber may unsubscribe (or subscribe) while being told.
  local names = vim.tbl_keys(state.subscribers)
  table.sort(names)
  for _, name in ipairs(names) do
    local on_change = state.subscribers[name]
    if on_change then
      on_change(fleet, err)
    end
  end
end

---The primary checkout's path of the fleet last kept, where the core runs to register; the current
---directory when the fleet was not read.
---@return string
local function primary_path()
  return state.fleet and state.fleet.primary.path or vim.fn.getcwd()
end

---Where the core can reach this Neovim: `v:servername` when it is the absolute path of a unix
---socket, the only kind of address the core takes; `nil` otherwise (no server, or a TCP address).
---@return string?
local function server_address()
  local address = vim.v.servername
  if type(address) == 'string' and vim.startswith(address, '/') then
    return address
  end
  return nil
end

---Registers this Neovim with the core, without waiting, from the primary checkout of the fleet
---last kept, and remembers it for `unregister()`. It is remembered before the core answers, so
---Neovim exiting meanwhile still unregisters it; unregistering a registration that failed is
---harmless. A failure is silent: the subscribers then get the fleet on focus and when they ask
---only. It runs again on `FocusGained`, as the core forgets a Neovim it could not reach (one
---suspended long enough stops answering); registering again changes nothing.
local function register()
  local address = server_address()
  if not address then
    return
  end
  local cwd = primary_path()
  local known = false
  for _, registration in ipairs(state.registered) do
    if registration.address == address and registration.cwd == cwd then
      known = true
    end
  end
  if not known then
    table.insert(state.registered, { address = address, cwd = cwd })
  end
  core.nvim_register(address, cwd, function() end)
end

---Undoes every registration, waiting for each (1 second at most). Errors are ignored: Neovim is
---exiting, and the core forgets an address it cannot reach anyway.
local function unregister()
  for _, registration in ipairs(state.registered) do
    core.nvim_unregister(registration.address, registration.cwd)
  end
  state.registered = {}
end

---Reads the fleet of the current directory's repository, waiting for it, keeps it and hands it to
---every subscriber. A read without waiting that is still running is dropped as it ends.
---@return orca_term.Fleet? fleet `nil` when it could not be read
---@return orca_term.Error? err why it could not be read
function M.read()
  state.reads = state.reads + 1
  -- Looked up on each call, not kept in a local: the specs count the reads by wrapping it.
  local fleet, err = core.worktree_ls(vim.fn.getcwd())
  keep(fleet, err)
  return fleet, err
end

---Reads the fleet without waiting, set below: it and `read_soon` start each other.
---@type fun()
local read_now

---Reads the fleet `DELAY_MS` from now, without waiting, while anyone is subscribed: unless a read
---is already waiting, which the events of a burst share, or running, which one more read then
---follows.
function M.read_soon()
  if not subscribed() then
    return
  end
  if state.reading then
    state.again = true
    return
  end
  state.timer = state.timer or assert(vim.uv.new_timer())
  if not state.timer:is_active() then
    state.timer:start(M.DELAY_MS, 0, vim.schedule_wrap(read_now))
  end
end

---Reads the fleet without waiting, then keeps it as `read()` does. A read that ends after a newer
---read started (waited for or not), or once nobody is subscribed, is dropped. When an event came
---during the read, one more read follows.
read_now = function()
  if not subscribed() then
    return
  end
  state.reads = state.reads + 1
  local this_read = state.reads
  state.reading = true
  core.worktree_ls_async(vim.fn.getcwd(), function(fleet, err)
    state.reading = false
    if subscribed() and state.reads == this_read then
      keep(fleet, err)
    end
    if state.again then
      state.again = false
      M.read_soon()
    end
  end)
end

---The fleet last kept and why it could not be read; both `nil` before the first read.
---@return orca_term.Fleet? fleet
---@return orca_term.Error? err
function M.fleet()
  return state.fleet, state.error
end

---Makes the autocommands of the live fleet, the first time anyone subscribes. Their group outlives
---every subscriber: unsubscribing unregisters nothing, and the events and the focus are ignored
---while nobody is subscribed.
local function ensure_autocmds()
  if state.group then
    return
  end
  state.group = vim.api.nvim_create_augroup('orca-term-live', { clear = true })
  vim.api.nvim_create_autocmd('User', {
    group = state.group,
    pattern = M.EVENT,
    desc = 'orca-term: read the fleet again, a worktree changed',
    callback = function()
      M.read_soon()
    end,
  })
  vim.api.nvim_create_autocmd('FocusGained', {
    group = state.group,
    desc = 'orca-term: read the fleet again, and register with the core again',
    callback = function()
      if subscribed() then
        M.read()
        register()
      end
    end,
  })
  vim.api.nvim_create_autocmd('VimLeavePre', {
    group = state.group,
    desc = 'orca-term: tell the core this Neovim is gone',
    callback = function()
      unregister()
    end,
  })
end

---Calls `on_change` with the fleet after every read that is kept, until `unsubscribe(name)`.
---Subscribing again under the same name replaces the function. Every subscription registers this
---Neovim with the core, from the primary checkout of the fleet last kept: read the fleet first
---(`read()`) to register from the right repository. Nothing is read here.
---@param name string who subscribes, such as `sidebar`
---@param on_change fun(fleet: orca_term.Fleet?, err: orca_term.Error?) `fleet` is `nil` when it
---could not be read, and `err` says why
function M.subscribe(name, on_change)
  state.subscribers[name] = on_change
  ensure_autocmds()
  register()
end

---Stops calling the function subscribed as `name`. When nobody is left, a read waiting after an
---event is dropped, and one running is dropped as it ends. Nothing is unregistered.
---@param name string
function M.unsubscribe(name)
  state.subscribers[name] = nil
  if subscribed() then
    return
  end
  if state.timer then
    state.timer:stop()
  end
  state.again = false
end

return M
