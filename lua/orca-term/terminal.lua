-- orca-term's agent terminals: the terminal buffer that shows a worktree's agent, attached to its
-- holder through `orca-term agent attach` (run by `core.lua`).
--
-- This module, `sidebar.lua` and `tabs.lua` are the only ones that touch windows and buffers
-- (`cargo xtask client-boundary` checks it). A terminal is never remembered: it is found again,
-- each time, as the buffer whose `b:orca_term_agent` is the worktree's path, compared with symlinks
-- resolved. Closing the terminal only detaches: `:q` on its window hides it, still attached, and
-- `:bwipeout!` ends the attach; the agent keeps running either way, in its holder.
--
-- Neovim refuses `:wqa` and `:xa` (E948) while a terminal's job runs, although ending the attach
-- only detaches. So the first terminal opened adds an `ExitPre` autocommand that stops the attach
-- of every agent terminal before Neovim checks for running jobs. If Neovim then still refuses to
-- quit (a buffer with unsaved changes), the terminals stay detached until `<CR>` attaches again.

local core = require('orca-term.core')

local M = {}

---The buffer variable that marks an agent terminal, holding its worktree's path.
M.MARK = 'orca_term_agent'

---The autocommand group of the `ExitPre` autocommand.
local GROUP = 'orca-term-terminal'

---`path` with its symlinks resolved, or only normalized when it does not exist, as tabs.lua does.
---@param path string
---@return string
local function resolved(path)
  return vim.uv.fs_realpath(path) or vim.fs.normalize(path)
end

---The worktree path `buf` is the agent terminal of, or `nil` when it is no agent terminal.
---@param buf integer
---@return string?
local function worktree_of(buf)
  return vim.b[buf][M.MARK]
end

---Every agent terminal, in every tab, shown or hidden.
---@return integer[]
local function all_terminals()
  local found = {}
  for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    if worktree_of(buf) then
      table.insert(found, buf)
    end
  end
  return found
end

---The agent terminal of the worktree at `path`, if any, running or not.
---@param path string
---@return integer?
function M.find(path)
  local wanted = resolved(path)
  for _, buf in ipairs(all_terminals()) do
    if worktree_of(buf) == wanted then
      return buf
    end
  end
  return nil
end

---The job of the terminal `buf` while its attach runs, or `nil` once it has ended.
---@param buf integer
---@return integer?
function M.running_job(buf)
  local job = vim.bo[buf].channel
  -- jobwait() with no wait gives -1 for a job still running.
  if job > 0 and vim.fn.jobwait({ job }, 0)[1] == -1 then
    return job
  end
  return nil
end

---The size of the current window, the first terminal size of an agent started for it.
---@return { rows: integer, cols: integer }
function M.size()
  return { rows = vim.api.nvim_win_get_height(0), cols = vim.api.nvim_win_get_width(0) }
end

---Stops the attach of every agent terminal, which only detaches: the agents keep running.
local function detach_all()
  for _, buf in ipairs(all_terminals()) do
    local job = M.running_job(buf)
    if job then
      vim.fn.jobstop(job)
    end
  end
end

---Adds the `ExitPre` autocommand that detaches every agent terminal before Neovim quits, so a
---running attach never blocks `:wqa` or `:xa`. Adding it again replaces it.
local function detach_on_exit()
  local group = vim.api.nvim_create_augroup(GROUP, { clear = true })
  vim.api.nvim_create_autocmd('ExitPre', {
    group = group,
    desc = 'orca-term: detach the agent terminals so they never block quitting',
    callback = detach_all,
  })
end

---Shows a new buffer in the current window, attached to the agent of the worktree at `path`, and
---marks it. When the attach cannot start, the window gets its buffer back.
---@param path string
---@param cwd string where the core runs
---@return integer? buf
---@return orca_term.Error? err
local function attach_in_new_buffer(path, cwd)
  local previous = vim.api.nvim_get_current_buf()
  local buf = vim.api.nvim_create_buf(true, false)
  vim.api.nvim_win_set_buf(0, buf)
  local job, err = core.agent_attach(path, cwd)
  if not job then
    vim.api.nvim_win_set_buf(0, previous)
    vim.api.nvim_buf_delete(buf, { force = true })
    return nil, err
  end
  -- `:q` on the terminal's window hides it, still attached, whatever 'hidden' says.
  vim.bo[buf].bufhidden = 'hide'
  vim.b[buf][M.MARK] = resolved(path)
  return buf
end

---Shows the agent of the worktree at `path` in the current window, in terminal mode: its
---terminal when one is still attached, shown elsewhere or hidden, else a new one attached to its
---holder (a terminal whose attach ended is wiped). The caller has made the worktree's main window
---current, and has started the agent.
---@param path string
---@param cwd string where the core runs
---@return orca_term.Error? err
function M.open(path, cwd)
  detach_on_exit()
  local existing = M.find(path)
  if existing and M.running_job(existing) then
    vim.api.nvim_win_set_buf(0, existing)
  else
    local _, err = attach_in_new_buffer(path, cwd)
    if err then
      return err
    end
    -- Wiped only now: wiping the buffer the window still showed would close the window.
    if existing then
      vim.api.nvim_buf_delete(existing, { force = true })
    end
  end
  vim.cmd.startinsert()
  return nil
end

return M
