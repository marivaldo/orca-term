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
-- Every read first reads the project list, each project with its fleet, in one run of the core
-- (`fleets`), and keeps it: `M.projects()` gives it (#154). With projects on the list that is the
-- whole read (#156): the subscribers are handed no fleet, and take every project's from
-- `M.fleets()`. With an empty list, or one that could not be read, the read goes on to the fleet of
-- the current directory's repository (`worktree ls`), as before the list existed, and `M.fleets()`
-- is that one fleet.
--
-- This Neovim registers with the primary checkout of every fleet kept, so a change in any project
-- reaches it. After each read that is kept the registrations follow the project list: a project
-- added to the list is registered with, and one taken off it is unregistered from.
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
---The fleet of the current directory's repository last kept; `nil` before the first read, when it
---failed, and while the project list has projects
---@field fleet? orca_term.Fleet
---@field error? orca_term.Error why that fleet could not be read
---The project list last kept, each project with its fleet; empty when the list is empty, `nil`
---before the first read or when it could not be read
---@field projects? orca_term.ProjectFleet[]
---@field projects_error? orca_term.Error why the project list last kept could not be read
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

---Whether this Neovim, at `address`, is remembered as registered from `cwd`.
---@param address string
---@param cwd string
---@return boolean
local function is_registered(address, cwd)
  for _, registration in ipairs(state.registered) do
    if registration.address == address and registration.cwd == cwd then
      return true
    end
  end
  return false
end

---Registers this Neovim, at `address`, with the repository `cwd` is in, without waiting, and
---remembers it for `unregister()`. It is remembered before the core answers, so Neovim exiting
---meanwhile still unregisters it; unregistering a registration that failed is harmless. A failure
---is silent: the subscribers then get that repository's changes on focus and when they ask only.
---@param address string
---@param cwd string
local function register_from(address, cwd)
  if not is_registered(address, cwd) then
    table.insert(state.registered, { address = address, cwd = cwd })
  end
  core.nvim_register(address, cwd, function() end)
end

---Registers this Neovim with the core from the primary checkout of every fleet last kept, every
---project's when the project list has some; from the current directory when no fleet is kept.
---It runs again on `FocusGained`, as the core forgets a Neovim it could not reach (one suspended
---long enough stops answering); registering again changes nothing.
local function register()
  local address = server_address()
  if not address then
    return
  end
  local fleets = M.fleets()
  if #fleets == 0 then
    register_from(address, vim.fn.getcwd())
  end
  for _, fleet in ipairs(fleets) do
    register_from(address, fleet.primary.path)
  end
end

---Undoes every registration, waiting for each (1 second at most). Errors are ignored: Neovim is
---exiting, and the core forgets an address it cannot reach anyway.
local function unregister()
  for _, registration in ipairs(state.registered) do
    core.nvim_unregister(registration.address, registration.cwd)
  end
  state.registered = {}
end

---Undoes the registrations made from `cwd`, waiting for each (1 second at most), and forgets
---them. Errors are ignored, as in `unregister()`.
---@param cwd string
local function unregister_from(cwd)
  local others = {}
  for _, registration in ipairs(state.registered) do
    if registration.cwd == cwd then
      core.nvim_unregister(registration.address, cwd)
    else
      table.insert(others, registration)
    end
  end
  state.registered = others
end

---The primary checkout of every project whose fleet is kept, as a set. Empty while the project
---list is empty.
---@return table<string, true>
local function project_primaries()
  local paths = {}
  for _, project in ipairs(state.projects or {}) do
    if project.fleet then
      paths[project.fleet.primary.path] = true
    end
  end
  return paths
end

---Makes the registrations follow the fleets just kept. This Neovim unregisters from every
---project of `before` whose fleet is no longer kept: a project taken off the list, or one whose
---directory is gone. While anyone is subscribed, it registers from the primary checkout of every
---fleet it is not registered from yet: a project added to the list.
---@param before table<string, true> the primary checkouts of the projects kept before this read
local function follow_fleets(before)
  local fleets = M.fleets()
  local kept = {}
  for _, fleet in ipairs(fleets) do
    kept[fleet.primary.path] = true
  end
  for path in pairs(before) do
    if not kept[path] then
      unregister_from(path)
    end
  end
  local address = server_address()
  if not (address and subscribed()) then
    return
  end
  for _, fleet in ipairs(fleets) do
    if not is_registered(address, fleet.primary.path) then
      register_from(address, fleet.primary.path)
    end
  end
end

---Keeps the fleet (or why it could not be read) and the project list read before it (`listed`, or
---why it could not be read), makes the registrations follow them, then tells every subscriber, in
---the order of their names. With projects on the list there is no fleet to hand over: the
---subscribers take every project's from `M.fleets()`.
---@param fleet? orca_term.Fleet
---@param err? orca_term.Error
---@param listed? orca_term.Fleets
---@param listed_err? orca_term.Error
local function keep(fleet, err, listed, listed_err)
  local before = project_primaries()
  state.fleet, state.error = fleet, err
  state.projects, state.projects_error = listed and listed.projects, listed_err
  follow_fleets(before)
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

---Whether `listed`, what `fleets` printed, has projects: then it is the whole read, and
---`worktree ls` is not run.
---@param listed? orca_term.Fleets
---@return boolean
local function has_projects(listed)
  return listed ~= nil and #listed.projects > 0
end

---Reads the project list with every project's fleet, waiting for it, keeps it and tells every
---subscriber. With an empty list, or one that cannot be read, it then reads the fleet of the
---current directory's repository, waiting too, and hands it to them. A read without waiting that is
---still running is dropped as it ends.
---@return orca_term.Fleet? fleet the current directory's; `nil` when it could not be read, and
---with projects on the list
---@return orca_term.Error? err why it could not be read
function M.read()
  state.reads = state.reads + 1
  local cwd = vim.fn.getcwd()
  -- Both looked up on each call, not kept in locals: the specs count the reads by wrapping them.
  local listed, listed_err = core.fleets(cwd)
  if has_projects(listed) then
    keep(nil, nil, listed, nil)
    return nil, nil
  end
  local fleet, err = core.worktree_ls(cwd)
  keep(fleet, err, listed, listed_err)
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

---Reads the project list and, when it is empty or cannot be read, the fleet of the current
---directory's repository, without waiting, and keeps them as `read()` does. A read that ends after
---a newer read started (waited for or not), or once nobody is subscribed, is dropped. When an
---event came during the read, one more read follows.
read_now = function()
  if not subscribed() then
    return
  end
  state.reads = state.reads + 1
  local this_read = state.reads
  state.reading = true

  ---The read ended, with what it found.
  ---@param fleet? orca_term.Fleet
  ---@param err? orca_term.Error
  ---@param listed? orca_term.Fleets
  ---@param listed_err? orca_term.Error
  local function done(fleet, err, listed, listed_err)
    state.reading = false
    if subscribed() and state.reads == this_read then
      keep(fleet, err, listed, listed_err)
    end
    if state.again then
      state.again = false
      M.read_soon()
    end
  end

  local cwd = vim.fn.getcwd()
  core.fleets_async(cwd, function(listed, listed_err)
    if has_projects(listed) then
      done(nil, nil, listed, nil)
      return
    end
    core.worktree_ls_async(cwd, function(fleet, err)
      done(fleet, err, listed, listed_err)
    end)
  end)
end

---The fleet of the current directory's repository last kept and why it could not be read; both
---`nil` before the first read, and while the project list has projects (`M.fleets()` then).
---@return orca_term.Fleet? fleet
---@return orca_term.Error? err
function M.fleet()
  return state.fleet, state.error
end

---Every fleet last kept: each project's that could be read, in the project list's order; with an
---empty list (or one that could not be read), the fleet of the current directory's repository
---alone. Empty before the first read and when nothing could be read.
---@return orca_term.Fleet[]
function M.fleets()
  if state.projects and #state.projects > 0 then
    local fleets = {}
    for _, project in ipairs(state.projects) do
      if project.fleet then
        table.insert(fleets, project.fleet)
      end
    end
    return fleets
  end
  return { state.fleet }
end

---The project list last kept, each project with its fleet, and why it could not be read; both
---`nil` before the first read. An empty list is an empty table, never `nil`.
---@return orca_term.ProjectFleet[]? projects
---@return orca_term.Error? err
function M.projects()
  return state.projects, state.projects_error
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

---Calls `on_change` after every read that is kept, until `unsubscribe(name)`, with the fleet of
---the current directory's repository; with projects on the project list it is called with
---nothing, and takes every project's fleet from `M.fleets()`. Subscribing again under the same
---name replaces the function. Every subscription registers this Neovim with the core, from the
---primary checkout of every fleet last kept (the current directory before the first read); the
---registrations then follow each read that is kept. Nothing is read here.
---@param name string who subscribes, such as `sidebar`
---@param on_change fun(fleet: orca_term.Fleet?, err: orca_term.Error?) `fleet` is `nil` when it
---could not be read, and `err` says why; both are `nil` with projects on the list
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
