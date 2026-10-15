-- orca-term's worktree actions: open a worktree (`<CR>` in the sidebar: its tab, and its agent's
-- terminal), create a worktree (`a`), the menu of the worktree under the cursor (`.`: run its setup
-- again, copy its path, remove it), and going to a worktree by picking it (`:OrcaTerm jump`) or by
-- its number (`:OrcaTerm worktree N`).
--
-- It asks the person through `vim.ui.input` and `vim.ui.select`, runs the core through `core.lua`,
-- moves between tabs through `tabs.lua` and shows agents through `terminal.lua`; it touches no
-- window or buffer itself. The commands that may run a setup, and starting an agent, never block
-- Neovim: the sidebar is read again shortly after a setup starts (so the worktree shows as setting
-- up) and once more when the command finishes, and the outcome is told with `vim.notify`. Removing
-- a worktree always asks first.

local core = require('orca-term.core')
local render = require('orca-term.render')
local tabs = require('orca-term.tabs')
local terminal = require('orca-term.terminal')

local M = {}

---How long after starting `worktree new` or `worktree setup` the sidebar is read again.
M.EARLY_REFRESH_MS = 500

---The entries of the worktree menu, in the order shown.
M.MENU = { 'Re-run setup', 'Copy path', 'Remove worktree' }

local levels = vim.log.levels

---@param message string
local function info(message)
  vim.notify('orca-term: ' .. message, levels.INFO)
end

---@param message string
local function warn(message)
  vim.notify('orca-term: ' .. message, levels.WARN)
end

---Tells the person what failed. The message of an `orca_term.Error` already starts with
---`orca-term: ` and names the fix.
---@param err? orca_term.Error
local function fail(err)
  local message = err and err.message or 'orca-term: the core failed without saying why'
  vim.notify(message, levels.ERROR)
end

---Reads the fleet again into the sidebar, when it is open.
local function refresh_sidebar()
  require('orca-term.sidebar').refresh()
end

---Reads the fleet again into the sidebar a moment from now, once the core has started.
local function refresh_shortly()
  vim.defer_fn(refresh_sidebar, M.EARLY_REFRESH_MS)
end

---The agents `a` offers, in the order shown: claude first, as Orca does, then none.
M.AGENTS = { 'claude-code', 'No agent' }

---Starts the agent of the worktree at `path` through the core, then shows it in the worktree's
---tab, attached, in terminal mode. Starting is idempotent: an agent already running is only
---attached to, and one whose holder is gone (after a reboot, or after the agent exited) starts
---again. A refusal, such as a worktree not ready or no sandbox, is told with the core's reason and
---opens no terminal. The caller has gone to the worktree's tab.
---@param path string
---@param primary string the primary checkout's path, where the core runs
local function start_and_attach(path, primary)
  core.agent_start(path, terminal.size(), primary, function(started, err)
    refresh_sidebar()
    if not started then
      fail(err)
      return
    end
    -- Starting takes a moment. A person who has moved to another tab meanwhile is left there: the
    -- terminal opens on their next `<CR>`.
    if not tabs.is_current(path) then
      return
    end
    -- Back to the tab's main window, should the person have moved to the sidebar meanwhile.
    tabs.go_to(path)
    local attach_err = terminal.open(path, primary)
    if attach_err then
      fail(attach_err)
    end
  end)
end

---Opens `worktree`: goes to its tab and, when it has an agent, shows the agent's terminal there,
---starting the agent first when its holder is gone. A worktree with no agent only gets its tab.
---@param worktree orca_term.Worktree
---@param primary string the primary checkout's path, where the core runs
function M.open(worktree, primary)
  tabs.go_to(worktree.path)
  if worktree.agent then
    start_and_attach(worktree.path, primary)
  end
end

---Creates the worktree `name` through the core, without waiting for its setup. Once created, even
---when its setup failed, the worktree's tab becomes current; then, when the setup did not fail and
---`with_agent` is true, its agent starts and shows there.
---@param name string
---@param with_agent boolean
---@param primary string
local function create_worktree(name, with_agent, primary)
  core.worktree_new(name, primary, function(created, err)
    refresh_sidebar()
    if not created then
      fail(err)
      return
    end
    local path = created.worktree.path
    tabs.go_to(path)
    if created.setup.status == 'failed' then
      local detail = created.worktree.detail or 'see its setup log'
      local message = ('created worktree %s, but its setup failed: %s'):format(name, detail)
      if with_agent then
        message = message .. '; its agent was not started'
      end
      warn(message)
      return
    end
    info('created worktree ' .. name)
    if with_agent then
      start_and_attach(path, primary)
    end
  end)
  refresh_shortly()
end

---Asks for a name, then for the agent (claude-code or none), and creates that worktree through
---the core, without waiting for its setup. Cancelling either question creates nothing.
---@param primary string the primary checkout's path, where the core runs
function M.create(primary)
  vim.ui.input({ prompt = 'New worktree name: ' }, function(input)
    local name = vim.trim(input or '')
    if name == '' then
      return
    end
    vim.ui.select(M.AGENTS, { prompt = 'Agent for ' .. name }, function(choice)
      if choice then
        create_worktree(name, choice == M.AGENTS[1], primary)
      end
    end)
  end)
end

---Runs the setup of `worktree` again through the core, without waiting for it.
---@param worktree orca_term.Worktree
---@param primary string the primary checkout's path, where the core runs
function M.run_setup(worktree, primary)
  core.worktree_setup(worktree.path, primary, function(set_up, err)
    refresh_sidebar()
    if not set_up then
      fail(err)
    elseif set_up.setup.status == 'failed' then
      local detail = set_up.worktree.detail or 'see its setup log'
      warn(('the setup of %s failed: %s'):format(worktree.name, detail))
    else
      info(('worktree %s is ready'):format(worktree.name))
    end
  end)
  refresh_shortly()
end

---Puts the path of `worktree` in the unnamed register, and in the clipboard when Neovim has one.
---@param worktree orca_term.Worktree
function M.copy_path(worktree)
  vim.fn.setreg('"', worktree.path)
  if vim.fn.has('clipboard') == 1 then
    vim.fn.setreg('+', worktree.path)
  end
  info('copied ' .. worktree.path)
end

---What a removal did, for the person: the worktree, and its branch's fate.
---@param removed orca_term.Removed
---@return string
local function removal_message(removed)
  local branch = removed.branch
  if branch.status == 'deleted' then
    return ('removed worktree %s and its branch %s (%s)'):format(
      removed.name,
      branch.name,
      branch.reason
    )
  end
  if branch.status == 'kept' then
    return ('removed worktree %s; kept its branch %s: %s'):format(
      removed.name,
      branch.name,
      branch.reason
    )
  end
  return 'removed worktree ' .. removed.name
end

---Asks whether to remove `worktree` although it has changes. Forcing discards them and also
---deletes the worktree's branch, merged or not, so the question says both.
---@param worktree orca_term.Worktree
---@param on_yes fun()
local function confirm_discarding(worktree, on_yes)
  local prompt = (
    '%s has uncommitted changes. Remove anyway, discarding them and deleting its branch even if '
    .. 'unmerged?'
  ):format(worktree.name)
  vim.ui.select({ 'Remove anyway', 'Cancel' }, { prompt = prompt }, function(choice)
    if choice == 'Remove anyway' then
      on_yes()
    end
  end)
end

---Removes `worktree` through the core, `force`d or not. On success its tabs close; a worktree
---with changes asks once more before it is forced; a busy one only says why.
---@param worktree orca_term.Worktree
---@param primary string
---@param force boolean
local function run_removal(worktree, primary, force)
  core.worktree_rm(worktree.path, force, primary, function(removed, err)
    if removed then
      tabs.close_all(worktree.path, primary)
      refresh_sidebar()
      info(removal_message(removed))
    elseif err and err.kind == 'dirty' and not force then
      confirm_discarding(worktree, function()
        run_removal(worktree, primary, true)
      end)
    else
      fail(err)
    end
  end)
end

---Asks for confirmation, then removes `worktree` through the core and closes its tabs. The last
---tab is kept, pointed at the primary checkout.
---@param worktree orca_term.Worktree
---@param primary string the primary checkout's path, where the core runs
function M.remove(worktree, primary)
  local prompt = ('Remove worktree %s? Its branch is deleted only if merged.'):format(worktree.name)
  vim.ui.select({ 'Remove', 'Cancel' }, { prompt = prompt }, function(choice)
    if choice == 'Remove' then
      run_removal(worktree, primary, false)
    end
  end)
end

---The menu of what can be done with `worktree`.
---@param worktree orca_term.Worktree
---@param primary string the primary checkout's path, where the core runs
function M.menu(worktree, primary)
  vim.ui.select(M.MENU, { prompt = 'Worktree ' .. worktree.name }, function(choice)
    if choice == M.MENU[1] then
      M.run_setup(worktree, primary)
    elseif choice == M.MENU[2] then
      M.copy_path(worktree)
    elseif choice == M.MENU[3] then
      M.remove(worktree, primary)
    end
  end)
end

---The worktrees of the current directory's repository, in the sidebar's order, or `nil` after
---telling the person why the fleet cannot be read.
---@return orca_term.Worktree[]?
local function worktrees_in_order()
  local fleet, err = core.worktree_ls(vim.fn.getcwd())
  if not fleet then
    fail(err)
    return nil
  end
  local by_path = {}
  for _, worktree in ipairs(fleet.worktrees) do
    by_path[worktree.path] = worktree
  end
  local ordered = {}
  for _, path in ipairs(render.fleet(fleet).order) do
    table.insert(ordered, by_path[path])
  end
  return ordered
end

---A worktree as the jump list shows it: its name, then its state in words.
---@param worktree orca_term.Worktree
---@return string
function M.jump_item(worktree)
  local state = worktree.state:gsub('_', ' ')
  return worktree.name .. '  ' .. state
end

---Lets the person pick a worktree, in the sidebar's order, and goes to its tab.
function M.jump()
  local worktrees = worktrees_in_order()
  if not worktrees then
    return
  end
  if #worktrees == 0 then
    info('no worktrees yet')
    return
  end
  local options = { prompt = 'Go to worktree', format_item = M.jump_item }
  vim.ui.select(worktrees, options, function(choice)
    if choice then
      tabs.go_to(choice.path)
    end
  end)
end

---Goes to the tab of the worktree numbered `n` (from 1) in the sidebar's order.
---@param n integer
function M.go_to_nth(n)
  local worktrees = worktrees_in_order()
  if not worktrees then
    return
  end
  local worktree = worktrees[n]
  if not worktree then
    local message = ('orca-term: there is no worktree %d; this repository has %d'):format(
      n,
      #worktrees
    )
    vim.notify(message, levels.ERROR)
    return
  end
  tabs.go_to(worktree.path)
end

return M
