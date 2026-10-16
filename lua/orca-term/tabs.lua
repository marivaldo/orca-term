-- orca-term's worktree tabs: each worktree gets a tab of its own, whose tab-local directory is the
-- worktree.
--
-- This module, `sidebar.lua`, `terminal.lua`, `review.lua` and `notes.lua` are the only ones that
-- touch windows and tabs (`cargo xtask client-boundary` checks it). A worktree's tab is never
-- remembered: it is found again, each time, as the tab whose directory (`getcwd(-1, tabnr)`) is the
-- worktree's path, compared with symlinks resolved. A review tab (`review.lua`) is never a
-- worktree's tab, although it has the worktree's directory: diffview makes it from the worktree's
-- tab, whose tab-local directory a new tab inherits. Closing a tab only closes the tab; the
-- worktree stays as it is.

local M = {}

---`path` with its symlinks resolved, or only normalized when it does not exist, as for a worktree
---just removed. The core prints paths with symlinks resolved, so both forms compare equal.
---@param path string
---@return string
local function resolved(path)
  return vim.uv.fs_realpath(path) or vim.fs.normalize(path)
end

---The directory of `tab`: its tab-local directory, or Neovim's global one when it has none.
---@param tab integer
---@return string
local function directory_of(tab)
  return vim.fn.getcwd(-1, vim.api.nvim_tabpage_get_number(tab))
end

---Whether `tab` is a review tab, which is never a worktree's tab.
---@param tab integer
---@return boolean
local function is_review(tab)
  return require('orca-term.review').is_review(tab)
end

---Every tab whose directory is `path`, first to last, review tabs left out.
---@param path string
---@return integer[]
function M.find_all(path)
  local wanted = resolved(path)
  local found = {}
  for _, tab in ipairs(vim.api.nvim_list_tabpages()) do
    if not is_review(tab) and resolved(directory_of(tab)) == wanted then
      table.insert(found, tab)
    end
  end
  return found
end

---Whether the current tab is the tab of the worktree at `path`: its directory is `path`, and it is
---no review tab.
---@param path string
---@return boolean
function M.is_current(path)
  local tab = vim.api.nvim_get_current_tabpage()
  return not is_review(tab) and resolved(directory_of(tab)) == resolved(path)
end

---Whether `win` shows the sidebar.
---@param win integer
---@return boolean
local function shows_sidebar(win)
  local sidebar = require('orca-term.sidebar').buffer()
  return sidebar ~= nil and vim.api.nvim_win_get_buf(win) == sidebar
end

---Makes the current tab's main window current: the window last used in the tab, unless that is
---the sidebar, then the first window that is neither the sidebar nor floating.
local function focus_main_window()
  if not shows_sidebar(vim.api.nvim_get_current_win()) then
    return
  end
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    local floating = vim.api.nvim_win_get_config(win).relative ~= ''
    if not floating and not shows_sidebar(win) then
      vim.api.nvim_set_current_win(win)
      return
    end
  end
end

---Goes to the tab of the worktree at `path` and makes its main window current. When no tab has
---the worktree's directory, opens one after the last tab and points it at the worktree with
---`:tcd`; the sidebar, when open, shows itself there.
---@param path string
function M.go_to(path)
  local tab = M.find_all(path)[1]
  if tab then
    vim.api.nvim_set_current_tabpage(tab)
  else
    vim.cmd('$tabnew')
    vim.cmd.tcd(vim.fn.fnameescape(path))
  end
  focus_main_window()
end

---Closes every tab of the worktree at `path`, as after the worktree is removed. A buffer with
---unsaved changes is kept, hidden. Neovim cannot close its last tab, so when that is one of them it
---stays open and its directory becomes `fallback`, the primary checkout.
---@param path string
---@param fallback string
function M.close_all(path, fallback)
  for _, tab in ipairs(M.find_all(path)) do
    if #vim.api.nvim_list_tabpages() == 1 then
      -- The only tab left is the current one.
      vim.cmd.tcd(vim.fn.fnameescape(fallback))
    else
      vim.cmd('tabclose! ' .. vim.api.nvim_tabpage_get_number(tab))
    end
  end
end

return M
