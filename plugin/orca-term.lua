-- orca-term's Neovim client: the `:OrcaTerm <subcommand>` command. Neovim runs this file at startup.
--
-- It defines the command, the `<Plug>` maps a person can bind keys of their own to, and the two
-- autocommands that bring the sidebar back when a session is sourced, and nothing else: no global
-- key, and no work until the command runs or a session restores the sidebar. Each subcommand calls
-- a module under lua/orca-term/.

---Tells the person that `:OrcaTerm` was used wrongly.
---@param message string
local function usage_error(message)
  vim.notify('orca-term: ' .. message, vim.log.levels.ERROR)
end

---Each subcommand's name and what it does with the arguments after it.
---@type table<string, fun(args: string[])>
local subcommands = {
  sidebar = function()
    require('orca-term.sidebar').toggle()
  end,
  jump = function()
    require('orca-term.actions').jump()
  end,
  review = function()
    require('orca-term.actions').review_current()
  end,
  notes = function()
    require('orca-term.actions').notes_current()
  end,
  turns = function()
    require('orca-term.actions').turns_current()
  end,
  sessions = function()
    require('orca-term.actions').sessions_current()
  end,
  ['install-cli'] = function()
    require('orca-term.install').install_cli()
  end,
  ['pr-refresh'] = function()
    require('orca-term.pr').refresh_all()
  end,
  worktree = function(args)
    local n = tonumber(args[1])
    if not n or n < 1 or n ~= math.floor(n) then
      usage_error(
        "`:OrcaTerm worktree` takes the worktree's number in the sidebar, from 1, "
          .. 'such as `:OrcaTerm worktree 2`'
      )
      return
    end
    require('orca-term.actions').go_to_nth(n)
  end,
}

---The subcommand names, sorted.
---@return string[]
local function names()
  local list = vim.tbl_keys(subcommands)
  table.sort(list)
  return list
end

vim.api.nvim_create_user_command('OrcaTerm', function(command)
  local name = command.fargs[1]
  local subcommand = subcommands[name]
  if not subcommand then
    usage_error(
      ('unknown subcommand `%s`; use one of: %s'):format(name, table.concat(names(), ', '))
    )
    return
  end
  subcommand(vim.list_slice(command.fargs, 2))
end, {
  nargs = '+',
  desc = 'orca-term: run a subcommand, such as `:OrcaTerm sidebar` or `:OrcaTerm worktree 2`',
  complete = function(arg_lead, cmd_line)
    -- Only the first argument, the subcommand, is completed.
    if not cmd_line:match('^%s*%S+%s+%S*$') then
      return {}
    end
    return vim.tbl_filter(function(name)
      return vim.startswith(name, arg_lead)
    end, names())
  end,
})

-- No key reaches these maps until a person binds one, e.g.
-- `vim.keymap.set('n', '<leader>o', '<Plug>(OrcaTermSidebar)')`.
vim.keymap.set('n', '<Plug>(OrcaTermSidebar)', function()
  require('orca-term.sidebar').toggle()
end, { desc = 'orca-term: toggle the sidebar in every tab' })

vim.keymap.set('n', '<Plug>(OrcaTermJump)', function()
  require('orca-term.actions').jump()
end, { desc = 'orca-term: pick a worktree and go to its tab' })

vim.keymap.set('n', '<Plug>(OrcaTermReview)', function()
  require('orca-term.actions').review_current()
end, { desc = "orca-term: open the review of the current tab's worktree in diffview" })

vim.keymap.set('n', '<Plug>(OrcaTermNotes)', function()
  require('orca-term.actions').notes_current()
end, { desc = "orca-term: list the current tab's worktree's pending notes, to send them" })

vim.keymap.set(
  'n',
  '<Plug>(OrcaTermTurns)',
  function()
    require('orca-term.actions').turns_current()
  end,
  { desc = "orca-term: pick one of the current tab's worktree's turns and review it in diffview" }
)

vim.keymap.set('n', '<Plug>(OrcaTermSessions)', function()
  require('orca-term.actions').sessions_current()
end, {
  desc = 'orca-term: list the sessions of the worktree under the cursor in the sidebar, '
    .. "or of the current tab's worktree, to resume one",
})

vim.keymap.set('n', '<Plug>(OrcaTermPrRefresh)', function()
  require('orca-term.pr').refresh_all()
end, { desc = "orca-term: refresh every worktree's pull request and checks" })

vim.keymap.set('n', '<Plug>(OrcaTermInstallCli)', function()
  require('orca-term.install').install_cli()
end, { desc = 'orca-term: link the core binary into $XDG_BIN_HOME or ~/.local/bin, for the shell' })

for n = 1, 9 do
  vim.keymap.set('n', ('<Plug>(OrcaTermWorktree%d)'):format(n), function()
    require('orca-term.actions').go_to_nth(n)
  end, { desc = ('orca-term: go to the tab of worktree %d in the sidebar'):format(n) })
end

-- A session saved with the sidebar open (#113). `:mksession` with `blank` keeps each sidebar window
-- as an empty buffer named `orca-term://sidebar` (the sidebar's `BUFFER_NAME`). The sidebar module
-- renames each one as the session names it, and once the session is loaded, closes them and shows
-- the sidebar. Nothing is loaded until a session names such a buffer.
local session_group = vim.api.nvim_create_augroup('orca-term-session', { clear = true })

-- A session sourced while the sidebar's own buffer holds that name (#124): it gives the name up for
-- the length of the load. SessionLoadPre is new in Neovim 0.12, and only a session file written by
-- 0.12 or later triggers it; BufFilePre, which every `:file` of the session triggers before it
-- names its buffer, covers Neovim 0.11 and older session files. Doing it twice does nothing.
if vim.fn.exists('##SessionLoadPre') == 1 then
  vim.api.nvim_create_autocmd('SessionLoadPre', {
    group = session_group,
    desc = "orca-term: take the sidebar's name off its buffer while a session loads",
    callback = function()
      if package.loaded['orca-term.sidebar'] then
        require('orca-term.sidebar').session_loading()
      end
    end,
  })
end

vim.api.nvim_create_autocmd('BufFilePre', {
  group = session_group,
  desc = "orca-term: take the sidebar's name off its buffer before a session names a window",
  callback = function()
    if vim.g.SessionLoad == 1 and package.loaded['orca-term.sidebar'] then
      require('orca-term.sidebar').session_loading()
    end
  end,
})

vim.api.nvim_create_autocmd('BufFilePost', {
  group = session_group,
  pattern = 'orca-term://sidebar',
  desc = 'orca-term: rename an empty window a session keeps for the sidebar',
  callback = function(args)
    if vim.g.SessionLoad == 1 then
      require('orca-term.sidebar').name_taken(args.buf)
    end
  end,
})

vim.api.nvim_create_autocmd('SessionLoadPost', {
  group = session_group,
  desc = 'orca-term: bring back the sidebar a session was saved with',
  callback = function()
    -- The sidebar module is loaded when the session named a buffer after it, or was opened before.
    if package.loaded['orca-term.sidebar'] then
      require('orca-term.sidebar').session_loaded()
    end
  end,
})
