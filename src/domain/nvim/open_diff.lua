-- The code the core sends to a Neovim to open a changed file's diff (`nvim_exec_lua`, #194):
-- a new tab with the file's version at the merge base on the left and the file on disk on the
-- right, both in Neovim's built-in diff mode (`:diffthis`). It needs no plugin: everything here
-- is Neovim's own API, and the core hands it all it needs to know.
--
-- The arguments, in order (`open_diff` in `src/domain/nvim.rs` sends them):
--   worktree   the worktree's path, as `orca-term worktree ls` prints it
--   file       the changed file, relative to the worktree
--   base       the merge base's short commit id, or '' when the worktree has no merge base
--   base_text  the file's content at the merge base, or '' when the file is not there
--
-- It answers at once, `true` when this Neovim works in the worktree and `false` otherwise, and
-- opens the diff only after it answered, so the core never waits for it.
local worktree, file, base, base_text = ...

-- The tab variable that marks a diff tab, holding the path of the file it shows.
local MARK = 'orca_term_diff'

-- `path` with its symlinks resolved, or only normalized when it does not exist. The core prints
-- paths with symlinks resolved, so both forms compare equal.
local function resolved(path)
  return vim.uv.fs_realpath(path) or vim.fs.normalize(path)
end

-- Whether this Neovim works in the worktree: one of its tabs has the worktree as its directory.
-- A Neovim started in the worktree, as the application starts its own, always has.
local function works_in_worktree()
  local wanted = resolved(worktree)
  for _, tab in ipairs(vim.api.nvim_list_tabpages()) do
    local directory = vim.fn.getcwd(-1, vim.api.nvim_tabpage_get_number(tab))
    if resolved(directory) == wanted then
      return true
    end
  end
  return false
end

-- Closes the diff tabs already showing `path`, all but the current tab, so asking for the same
-- file twice leaves one diff, with the base read again.
local function close_earlier_diffs(path)
  local current = vim.api.nvim_get_current_tabpage()
  for _, tab in ipairs(vim.api.nvim_list_tabpages()) do
    if tab ~= current and vim.t[tab][MARK] == path then
      pcall(vim.cmd.tabclose, vim.api.nvim_tabpage_get_number(tab))
    end
  end
end

-- The lines of `text`, without the empty one a final line break leaves.
local function lines_of(text)
  local lines = vim.split(text, '\n', { plain = true })
  if lines[#lines] == '' then
    table.remove(lines)
  end
  return lines
end

-- Opens the diff in a new tab: the file on disk, then its base version in a window on its left.
local function open()
  local path = worktree .. '/' .. file

  -- The right window: the file on disk, in its own buffer, which stays editable.
  vim.cmd.tabnew()
  close_earlier_diffs(path)
  vim.t[MARK] = path
  vim.cmd.tcd(vim.fn.fnameescape(worktree))
  vim.cmd.edit(vim.fn.fnameescape(path))
  local file_window = vim.api.nvim_get_current_win()
  local filetype = vim.bo.filetype
  vim.cmd.diffthis()

  -- The left window: the base version, in a buffer of its own that is never written anywhere and
  -- goes away with its window.
  vim.cmd('leftabove vnew')
  local buffer = vim.api.nvim_get_current_buf()
  vim.api.nvim_buf_set_lines(buffer, 0, -1, false, lines_of(base_text))
  vim.bo[buffer].buftype = 'nofile'
  vim.bo[buffer].bufhidden = 'wipe'
  vim.bo[buffer].swapfile = false
  vim.bo[buffer].modifiable = false
  vim.bo[buffer].filetype = filetype
  local shown_base = base ~= '' and base or 'no merge base'
  vim.api.nvim_buf_set_name(buffer, ('orca-term://%s/%s'):format(shown_base, file))
  vim.cmd.diffthis()

  -- The person lands on the file, where they can edit.
  vim.api.nvim_set_current_win(file_window)
end

if not works_in_worktree() then
  return false
end
vim.schedule(open)
return true
