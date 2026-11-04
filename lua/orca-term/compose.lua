-- orca-term's pull request composer (#133): a buffer to write a worktree's new pull request in, as
-- fugitive's buffer for a commit message.
--
-- `Create pull request` and `Create draft pull request` in the worktree menu (`.` in the sidebar),
-- and `:OrcaTerm pr-create` (`actions.lua`), open it. It asks the core for the message the pull
-- request starts from (`pr compose`, through `core.lua`) and shows it in a split, in a buffer named
-- `orca-term://pr/<worktree>` with the filetype `markdown`: the first line is the title, then a
-- blank line, then the body. There are no comment lines, so a template's `##` headings stay.
--
-- `:w` creates the pull request through the core (`pr create`, into the default branch), without
-- blocking Neovim. On success the buffer is wiped, the pull request's number and address are told,
-- and the sidebar is read again, so its row shows `PR`. On a refusal or a failure the buffer stays,
-- with its text, and the core's reason is told. An empty title is refused before anything runs.
-- `:q!` or wiping the buffer creates nothing.
--
-- This module, `sidebar.lua`, `detail.lua`, `tabs.lua`, `terminal.lua`, `review.lua` and
-- `notes.lua` are the only ones that touch windows and buffers (`cargo xtask client-boundary`
-- checks it).

local core = require('orca-term.core')

local M = {}

---The start of a composer buffer's name; the worktree's name follows.
M.PREFIX = 'orca-term://pr/'

---What `:w` says when the first line is empty.
M.EMPTY_TITLE = 'the pull request needs a title on its first line; nothing was created'

---A composer buffer, and what `:w` needs to create its pull request.
---@class orca_term.Composer
---@field worktree orca_term.Worktree the worktree whose pull request it writes
---@field primary string the primary checkout's path, where the core runs
---@field draft boolean whether the pull request is created as a draft
---@field creating boolean whether the core is creating it right now

---Each composer buffer that is open, by its number.
---@type table<integer, orca_term.Composer>
local composers = {}

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

---The lines a composer starts with, out of what `pr compose` printed: the title, a blank line,
---then the body's lines (none when the body is empty).
---@param composed orca_term.Composed
---@return string[]
function M.lines(composed)
  local lines = { composed.title, '' }
  if composed.body == '' then
    return lines
  end
  local body = vim.split(composed.body, '\n', { plain = true })
  -- A body that ends with a newline would show an empty last line.
  if body[#body] == '' then
    table.remove(body)
  end
  return vim.list_extend(lines, body)
end

---Whether `line` holds nothing but blanks.
---@param line string
---@return boolean
local function is_blank(line)
  return vim.trim(line) == ''
end

---The title and the body a composer's `lines` hold: the first line, trimmed, and every line after
---it, the blank lines at its start and at its end left out.
---@param lines string[]
---@return string title `''` when the first line is blank
---@return string body `''` when there is none
function M.parse(lines)
  local title = vim.trim(lines[1] or '')
  local body = vim.list_slice(lines, 2)
  while body[1] and is_blank(body[1]) do
    table.remove(body, 1)
  end
  while body[#body] and is_blank(body[#body]) do
    table.remove(body)
  end
  return title, table.concat(body, '\n')
end

---What the person is told once the pull request is created: its number, `(draft)` for a draft,
---and its address.
---@param created orca_term.PrCreated
---@return string
function M.created_message(created)
  local pr = created.pr or {}
  if not pr.number then
    return 'created the pull request of ' .. created.name
  end
  local draft = pr.state == 'draft' and ' (draft)' or ''
  return ('created PR #%d%s: %s'):format(pr.number, draft, pr.url)
end

---What the person is warned of when files with changes not committed were left out, or `nil`
---when there were none.
---@param uncommitted integer
---@return string?
function M.uncommitted_message(uncommitted)
  if uncommitted == 0 then
    return nil
  elseif uncommitted == 1 then
    return '1 file with uncommitted changes was left out of the pull request'
  end
  return ('%d files with uncommitted changes were left out of the pull request'):format(uncommitted)
end

---What follows the core's answer to `:w` in the composer `buf`: on success the buffer is wiped
---(when it is still there), the pull request is told and the sidebar read again; on a refusal or a
---failure the buffer stays as it is and the reason is told.
---@param buf integer
---@param composer orca_term.Composer
---@param created? orca_term.PrCreated
---@param err? orca_term.Error
local function on_created(buf, composer, created, err)
  composer.creating = false
  if not created then
    fail(err)
    return
  end
  if vim.api.nvim_buf_is_valid(buf) then
    vim.api.nvim_buf_delete(buf, { force = true })
  end
  info(M.created_message(created))
  local left_out = M.uncommitted_message(created.uncommitted)
  if left_out then
    warn(left_out)
  end
  require('orca-term.sidebar').refresh()
end

---`:w` in the composer `buf`: creates the pull request its lines say, through the core, without
---blocking. An empty title is refused before anything runs, and a second `:w` while the core is
---still creating it only says so.
---@param buf integer
local function write(buf)
  local composer = composers[buf]
  if not composer then
    return
  end
  local name = composer.worktree.name
  if composer.creating then
    warn(('the pull request of %s is being created; wait for it'):format(name))
    return
  end
  local title, body = M.parse(vim.api.nvim_buf_get_lines(buf, 0, -1, false))
  if title == '' then
    fail({ kind = 'usage', message = 'orca-term: ' .. M.EMPTY_TITLE })
    return
  end
  composer.creating = true
  info(('creating the pull request of %s'):format(name))
  local path = composer.worktree.path
  core.pr_create(path, title, body, composer.draft, composer.primary, function(created, err)
    on_created(buf, composer, created, err)
  end)
end

---The composer buffer of the worktree named `name`, when one is open.
---@param name string
---@return integer?
local function find(name)
  for buf, composer in pairs(composers) do
    if composer.worktree.name == name and vim.api.nvim_buf_is_valid(buf) then
      return buf
    end
  end
  return nil
end

---Shows the composer `buf` in the current tab: goes to its window when one shows it, else opens
---a split along the whole bottom of the tab for it (under the sidebar too, so it is never as
---narrow as the sidebar).
---@param buf integer
local function show(buf)
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_buf(win) == buf then
      vim.api.nvim_set_current_win(win)
      return
    end
  end
  -- `win = -1` splits the whole tab, as `:botright split` does.
  vim.api.nvim_open_win(buf, true, { split = 'below', win = -1 })
end

---A new composer buffer for `worktree`, holding `lines`, unmodified, its `:w` creating the pull
---request.
---@param worktree orca_term.Worktree
---@param lines string[]
---@return integer buf
local function make_buffer(worktree, lines)
  local buf = vim.api.nvim_create_buf(false, false)
  vim.api.nvim_buf_set_name(buf, M.PREFIX .. worktree.name)
  vim.bo[buf].buftype = 'acwrite'
  vim.bo[buf].bufhidden = 'wipe'
  vim.bo[buf].swapfile = false
  -- No undo step for the text it starts with: `u` never empties the buffer.
  vim.bo[buf].undolevels = -1
  vim.api.nvim_buf_set_lines(buf, 0, -1, false, lines)
  -- -123456 is Neovim's value for "use the global 'undolevels'".
  vim.bo[buf].undolevels = -123456
  vim.bo[buf].modified = false
  vim.api.nvim_create_autocmd('BufWriteCmd', {
    buffer = buf,
    desc = 'orca-term: create the pull request this buffer holds',
    callback = function()
      write(buf)
    end,
  })
  vim.api.nvim_create_autocmd('BufWipeout', {
    buffer = buf,
    desc = 'orca-term: forget the pull request composer',
    callback = function()
      composers[buf] = nil
    end,
  })
  return buf
end

---Opens the composer of a new pull request of `worktree` (a draft when `draft`), prefilled with
---what `pr compose` prints, in a split. A composer already open for the worktree is shown again
---with its text, and creates a draft or not as `draft` now says. A refusal of the core is told and
---opens nothing.
---@param worktree orca_term.Worktree
---@param primary string the primary checkout's path, where the core runs
---@param draft boolean
function M.open(worktree, primary, draft)
  local buf = find(worktree.name)
  if buf then
    composers[buf].draft = draft
    show(buf)
    return
  end
  local composed, err = core.pr_compose(worktree.path, primary)
  if not composed then
    fail(err)
    return
  end
  buf = make_buffer(worktree, M.lines(composed))
  composers[buf] = { worktree = worktree, primary = primary, draft = draft, creating = false }
  show(buf)
  -- Set once the buffer is in its window, so the filetype's window options land there.
  vim.bo[buf].filetype = 'markdown'
  vim.api.nvim_win_set_cursor(0, { 1, 0 })
end

---The composer buffer of the worktree named `name`, when one is open; for the specs.
---@param name string
---@return integer?
function M.buffer(name)
  return find(name)
end

---Whether the composer `buf` creates a draft; for the specs.
---@param buf integer
---@return boolean?
function M.is_draft(buf)
  return composers[buf] and composers[buf].draft
end

return M
