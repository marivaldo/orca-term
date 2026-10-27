-- Seam 2: a session manager restoring worktree tabs (#109), in headless Neovim against the real
-- binary and the fake claude of `support.agent_repo()`. orca-term keeps no layout state of its own:
-- a worktree's tab is found by its tab-local directory (`tabs.lua`), so a session that brings back
-- each tab's `:tcd` hands those tabs back to orca-term.
--
-- The spec does what a session manager does. This Neovim saves the session with `:mksession!`
-- into a temporary directory, under the `sessionoptions` being tested, then wipes its tabs and
-- terminals (which only detaches them, as quitting does). A fresh child Neovim, embedded and driven
-- over RPC, loads the client and sources the file with `silent! source`, as persistence.nvim does.
-- A child, rather than this Neovim sourcing its own session, starts with none of the client's
-- state (the sidebar's buffer, the terminals' marks), as a Neovim started again does.
--
-- Two `sessionoptions` values are tested: Neovim's default, which holds `terminal` and `blank`, and
-- LazyVim's persistence.nvim value, which holds neither.
--
-- What `:mksession` does with the agent's terminal was measured, on Neovim 0.12: a terminal
-- started from a list of arguments is named `term://<cwd>//<pid>:<program>`, its first argument
-- only, so the restored `term://` buffer runs the bare `orca-term` binary, which prints its usage
-- and exits. It starts no holder and no agent, and it is no agent terminal of orca-term's (it has
-- no mark), so `<CR>` attaches a new terminal in its place. The specs below guard all of that.
--
-- With `blank` (#113), `:mksession` keeps each tab's sidebar window as `enew` followed by
-- `file orca-term://sidebar`. The client renames each such buffer as the session names it, so no
-- tab's `file` fails with E95, and once the session is loaded closes their windows, wipes them and
-- opens the sidebar. The specs check the child's `v:errmsg`, its buffers and each tab's windows.
--
-- A child can also open the sidebar before it sources the session (#124), as a person restoring a
-- session from a Neovim already in use does: the sidebar's own buffer then holds the name the
-- session's `file` lines give, and the specs check it comes back the same buffer, under its name.

local support = require('support')

---Neovim's default `sessionoptions`.
local DEFAULT = vim.api.nvim_get_option_info2('sessionoptions', {}).default --[[@as string]]

---The `sessionoptions` LazyVim gives persistence.nvim.
local LAZYVIM = 'buffers,curdir,tabpages,winsize,help,globals,skiprtp,folds'

assert(DEFAULT:find('terminal', 1, true), "Neovim's default sessionoptions hold `terminal`")

---The child Neovims the spec started, stopped after it.
---@type integer[]
local children = {}

---Lua run in a child Neovim: what it shows, tab by tab. For each tab, its tab-local directory,
---how many windows show the sidebar, and its other windows: the buffer's name, its 'buftype',
---whether it is an agent terminal of orca-term's (its mark), whether its job still runs, and its
---non-blank lines. Also the current tab's number, the sidebar's lines when it is open, and the
---names of the buffers other than the sidebar's whose name starts with the sidebar's.
local SHOWN = [[
  local terminal = require('orca-term.terminal')
  local sidebar = require('orca-term.sidebar')
  local tabs = {}
  for _, tab in ipairs(vim.api.nvim_list_tabpages()) do
    local number = vim.api.nvim_tabpage_get_number(tab)
    local windows = {}
    local sidebars = 0
    for _, win in ipairs(vim.api.nvim_tabpage_list_wins(tab)) do
      local buf = vim.api.nvim_win_get_buf(win)
      if buf == sidebar.buffer() then
        sidebars = sidebars + 1
      else
        local lines = vim.tbl_filter(function(line)
          return line ~= ''
        end, vim.api.nvim_buf_get_lines(buf, 0, -1, false))
        table.insert(windows, {
          name = vim.api.nvim_buf_get_name(buf),
          buftype = vim.bo[buf].buftype,
          marked = vim.b[buf][terminal.MARK] ~= nil,
          running = terminal.running_job(buf) ~= nil,
          lines = lines,
        })
      end
    end
    local localdir = vim.fn.haslocaldir(-1, number) == 1
    table.insert(tabs, {
      directory = localdir and vim.fn.getcwd(-1, number) or nil,
      sidebars = sidebars,
      windows = windows,
    })
  end
  local others = {}
  for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    local name = vim.api.nvim_buf_get_name(buf)
    if buf ~= sidebar.buffer() and vim.startswith(name, sidebar.BUFFER_NAME) then
      table.insert(others, name)
    end
  end
  local open = sidebar.buffer() and sidebar.is_open()
  return {
    tabs = tabs,
    current = vim.fn.tabpagenr(),
    sidebar = open and vim.api.nvim_buf_get_lines(sidebar.buffer(), 0, -1, false) or nil,
    named_like_sidebar = others,
  }
]]

---What a child Neovim shows, as `SHOWN` returns it.
---@class spec.Shown
---@field tabs { directory?: string, sidebars: integer, windows: { name: string, buftype: string, marked: boolean, running: boolean, lines: string[] }[] }[]
---@field current integer
---@field sidebar? string[]
---@field named_like_sidebar string[]

---What the child Neovim `child` shows.
---@param child integer
---@return spec.Shown
local function shown(child)
  return vim.rpcrequest(child, 'nvim_exec_lua', SHOWN, {}) --[[@as spec.Shown]]
end

---Lua run in a child Neovim: the sidebar's buffer, its name and its lines.
local SIDEBAR_BUFFER = [[
  local buf = assert(require('orca-term.sidebar').buffer())
  return {
    buf = buf,
    name = vim.api.nvim_buf_get_name(buf),
    lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false),
  }
]]

---The sidebar's buffer in a child Neovim, as `SIDEBAR_BUFFER` returns it.
---@class spec.SidebarBuffer
---@field buf integer
---@field name string
---@field lines string[]

---The tab-local directory of each tab the child shows, `false` for a tab with none.
---@param child integer
---@return (string|false)[]
local function directories(child)
  return vim.tbl_map(function(tab)
    return tab.directory or false
  end, shown(child).tabs)
end

---Whether the sidebar of the child marks the worktree named `name` current: its card, boxed.
---@param child integer
---@param name string
---@return boolean
local function marks_current(child, name)
  for _, line in ipairs(shown(child).sidebar or {}) do
    if line:find('│', 1, true) and line:find(' ' .. name .. ' ', 1, true) then
      return true
    end
  end
  return false
end

---Waits until the sidebar of the child is open. A session saved with `blank` in 'sessionoptions'
---while the sidebar was open brings it back by itself once it is sourced (#113).
---@param child integer
local function wait_for_sidebar(child)
  support.wait_for(function()
    return shown(child).sidebar ~= nil
  end, 'the sidebar to come back in the child')
end

---Opens the sidebar of the child after a session saved under `sessionoptions` was sourced: the
---session brought it back when `sessionoptions` holds `blank`, and the spec opens it otherwise.
---@param child integer
---@param sessionoptions string
local function sidebar_after_restore(child, sessionoptions)
  if vim.tbl_contains(vim.split(sessionoptions, ','), 'blank') then
    wait_for_sidebar(child)
  else
    vim.rpcrequest(child, 'nvim_command', 'OrcaTerm sidebar')
  end
end

---Lua run in a child Neovim: puts the cursor on the sidebar line holding `...`.
local CURSOR_ON_SIDEBAR_LINE = [[
  local text = ...
  local sidebar = require('orca-term.sidebar')
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_buf(win) == sidebar.buffer() then
      vim.api.nvim_set_current_win(win)
      for index, line in ipairs(vim.api.nvim_buf_get_lines(0, 0, -1, false)) do
        if line:find(text, 1, true) then
          vim.api.nvim_win_set_cursor(win, { index, 0 })
          return true
        end
      end
    end
  end
  return false
]]

---Types `key` in the sidebar of the child Neovim, on the line holding `text`.
---@param child integer
---@param key string
---@param text string
local function press_in_child(child, key, text)
  local found = vim.rpcrequest(child, 'nvim_exec_lua', CURSOR_ON_SIDEBAR_LINE, { text })
  assert(found, 'no sidebar line holding ' .. text .. ' in the child')
  vim.rpcrequest(child, 'nvim_input', key)
end

---The sidebar buffer's lines, in this Neovim.
---@return string[]
local function lines()
  return vim.api.nvim_buf_get_lines(assert(require('orca-term.sidebar').buffer()), 0, -1, false)
end

---Presses `key` in the sidebar of this Neovim's current tab, on the line holding `text`.
---@param key string
---@param text string
local function press_on(key, text)
  local sidebar = require('orca-term.sidebar')
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_buf(win) == sidebar.buffer() then
      vim.api.nvim_set_current_win(win)
      for index, line in ipairs(lines()) do
        if line:find(text, 1, true) then
          vim.api.nvim_win_set_cursor(win, { index, 0 })
          vim.cmd('normal ' .. vim.keycode(key))
          return
        end
      end
      error('no sidebar line holding ' .. text)
    end
  end
  error('no sidebar in the current tab')
end

---Waits until the agent terminal of the worktree at `path` is attached in this Neovim, and
---returns its job.
---@param path string
---@return integer job
local function wait_for_terminal(path)
  local terminal = require('orca-term.terminal')
  support.wait_for(function()
    local buf = terminal.find(path)
    return buf ~= nil and terminal.running_job(buf) ~= nil
  end, 'the agent terminal of ' .. path)
  return assert(terminal.running_job(assert(terminal.find(path))))
end

---A repository whose agents are the fake claude, with the worktrees `alpha` and `beta` made by the
---core; those in `with_agent` get their agent started. This Neovim is in the primary checkout with
---the sidebar open.
---@param with_agent string[]
---@return spec.AgentRepo
local function repo_with(with_agent)
  support.load_client()
  local repo = support.agent_repo()
  for _, name in ipairs({ 'alpha', 'beta' }) do
    support.run({ support.binary, 'worktree', 'new', name }, repo.root)
  end
  for _, name in ipairs(with_agent) do
    support.run({ support.binary, 'agent', 'start', name }, repo.root)
  end
  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  return repo
end

---Saves the session of this Neovim under `sessionoptions` into a file of the repository's fixture
---directory, then wipes its tabs and its terminals (which detaches them), as quitting would.
---@param repo spec.Repo
---@param sessionoptions string
---@return string session the session file
local function save_session(repo, sessionoptions)
  local session = repo.tmp .. '/Session.vim'
  local saved = vim.o.sessionoptions
  vim.o.sessionoptions = sessionoptions
  vim.cmd('mksession! ' .. vim.fn.fnameescape(session))
  vim.o.sessionoptions = saved
  support.reset_editor()
  return session
end

---A fresh child Neovim, embedded and driven over RPC, in the primary checkout of `repo`, with the
---client loaded and set up. It inherits the fake claude's PATH, so it is started by its full path.
---With `sidebar_open`, the child has opened the sidebar, as a person restoring a session from it
---would (#124).
---@param repo spec.Repo
---@param sidebar_open? boolean
---@return integer channel
---@return spec.SidebarBuffer? sidebar the sidebar's buffer, with `sidebar_open`
local function start_child(repo, sidebar_open)
  local child = vim.fn.jobstart({
    vim.v.progpath,
    '--embed',
    '--headless',
    '--clean',
    '--cmd',
    'set runtimepath^=' .. vim.fn.fnameescape(support.root),
  }, { rpc = true, cwd = repo.root })
  assert(child > 0, 'the child Neovim did not start')
  table.insert(children, child)
  vim.rpcrequest(child, 'nvim_exec_lua', 'vim.o.columns = 160', {})
  local setup = "require('orca-term').setup({ binary = ... })"
  vim.rpcrequest(child, 'nvim_exec_lua', setup, { support.binary })
  local sidebar
  if sidebar_open then
    vim.rpcrequest(child, 'nvim_command', 'OrcaTerm sidebar')
    sidebar = vim.rpcrequest(child, 'nvim_exec_lua', SIDEBAR_BUFFER, {})
  end
  return child, sidebar
end

---A fresh child Neovim, as `start_child` makes it, that has sourced `session` as a session manager
---does.
---@param repo spec.Repo
---@param session string
---@param sidebar_open? boolean
---@return integer channel
---@return spec.SidebarBuffer? sidebar the sidebar's buffer before the session, with `sidebar_open`
local function restore_in_child(repo, session, sidebar_open)
  local child, sidebar = start_child(repo, sidebar_open)
  vim.rpcrequest(child, 'nvim_command', 'silent! source ' .. vim.fn.fnameescape(session))
  return child, sidebar
end

---Every file under the repository's git directory that belongs to orca-term (a path with an
---`orca-term` directory in it), relative to it and sorted, the registry of running Neovims left out:
---its files come and go with the Neovims, never with tabs.
---@param repo spec.Repo
---@return string[]
local function admin_files(repo)
  local git_dir = repo.root .. '/.git'
  local found = {}
  for name, kind in vim.fs.dir(git_dir, { depth = math.huge }) do
    local in_admin = ('/' .. name):find('/orca-term/', 1, true)
    if kind == 'file' and in_admin and not name:find('orca-term/nvim/', 1, true) then
      table.insert(found, name)
    end
  end
  table.sort(found)
  return found
end

describe('a session manager restoring worktree tabs', function()
  local columns = vim.o.columns

  before_each(function()
    vim.o.columns = 160
  end)

  after_each(function()
    for _, child in ipairs(children) do
      vim.fn.jobstop(child)
    end
    children = {}
    support.reset_editor()
    support.cleanup()
    vim.o.columns = columns
  end)

  for _, case in ipairs({ { 'the default', DEFAULT }, { "LazyVim's", LAZYVIM } }) do
    local which, sessionoptions = case[1], case[2]

    it(
      'brings back the worktree tabs, which <CR> and the number maps reuse and the sidebar marks, '
        .. ('with %s sessionoptions'):format(which),
      function()
        local repo = repo_with({})
        local alpha = support.worktree_path(repo, 'alpha')
        local beta = support.worktree_path(repo, 'beta')
        press_on('<CR>', '· alpha')
        press_on('<CR>', '· beta')
        -- Opening the sidebar refreshes the pull requests, whose answers are cached on their own.
        support.wait_for(function()
          local files = admin_files(repo)
          return vim.tbl_contains(files, 'worktrees/alpha/orca-term/pr.json')
            and vim.tbl_contains(files, 'worktrees/beta/orca-term/pr.json')
        end, 'the pull requests to be cached')
        local before = admin_files(repo)

        local child = restore_in_child(repo, save_session(repo, sessionoptions))

        assert.are.same({ false, alpha, beta }, directories(child))
        assert.are.equal(3, shown(child).current)
        sidebar_after_restore(child, sessionoptions)
        assert(marks_current(child, 'beta'), 'the sidebar marks the current tab')

        -- `<CR>` in the primary checkout's tab goes to alpha's restored tab, making none.
        vim.rpcrequest(child, 'nvim_command', 'tabfirst')
        press_in_child(child, '<CR>', '· alpha')
        support.wait_for(function()
          return shown(child).current == 2
        end, "alpha's restored tab to be current")
        assert.are.same({ false, alpha, beta }, directories(child))
        assert.is_true(marks_current(child, 'alpha'))

        -- The number maps go to the restored tabs too.
        vim.rpcrequest(child, 'nvim_input', '<Plug>(OrcaTermWorktree2)')
        support.wait_for(function()
          return shown(child).current == 3
        end, "beta's restored tab to be current")
        vim.rpcrequest(child, 'nvim_command', 'OrcaTerm worktree 1')
        assert.are.equal(2, shown(child).current)
        assert.are.same({ false, alpha, beta }, directories(child))

        -- orca-term recorded no tab and no window: its files are the ones it had before.
        assert.are.same(before, admin_files(repo))
      end
    )
  end

  it(
    'with blank in sessionoptions, brings the sidebar back in every tab without E95 or the empty '
      .. "windows the session kept for it, and keeps the person's blank windows",
    function()
      local repo = repo_with({})
      local alpha = support.worktree_path(repo, 'alpha')
      local beta = support.worktree_path(repo, 'beta')
      press_on('<CR>', '· alpha')
      press_on('<CR>', '· beta')
      -- A blank window of the person's own, next to alpha's in its tab.
      vim.cmd('tabnext 2')
      vim.cmd('belowright new')
      vim.cmd('tabnext 3')
      assert(DEFAULT:find('blank', 1, true), "Neovim's default sessionoptions hold `blank`")

      local child = restore_in_child(repo, save_session(repo, DEFAULT))

      local errmsg = vim.rpcrequest(child, 'nvim_get_vvar', 'errmsg') --[[@as string]]
      assert(not errmsg:find('E95', 1, true), errmsg)
      wait_for_sidebar(child)
      local now = shown(child)
      assert.are.same({}, now.named_like_sidebar, 'no buffer the session made for the sidebar')
      assert.are.same({ false, alpha, beta }, directories(child))
      assert.are.equal(3, now.current)
      for number, tab in ipairs(now.tabs) do
        assert.are.equal(1, tab.sidebars, 'one sidebar window in tab ' .. number)
        for _, window in ipairs(tab.windows) do
          assert.are.same({ '', '' }, { window.name, window.buftype }, 'a blank window')
        end
      end
      assert.are.same({ 1, 2, 1 }, {
        #now.tabs[1].windows,
        #now.tabs[2].windows,
        #now.tabs[3].windows,
      }, "each tab's own windows, the blank one in alpha's tab with them")
      assert(marks_current(child, 'beta'), 'the sidebar marks the current tab')

      -- `<CR>` in the primary checkout's tab goes to alpha's restored tab, making none.
      vim.rpcrequest(child, 'nvim_command', 'tabfirst')
      press_in_child(child, '<CR>', '· alpha')
      support.wait_for(function()
        return shown(child).current == 2
      end, "alpha's restored tab to be current")
      assert.are.same({ false, alpha, beta }, directories(child))
      assert.is_true(marks_current(child, 'alpha'))
    end
  )

  it(
    'with blank but not terminal in sessionoptions, a tab left with only the window kept for the '
      .. 'sidebar keeps an empty window beside it',
    function()
      local repo = repo_with({ 'alpha' })
      local alpha = support.worktree_path(repo, 'alpha')
      press_on('<CR>', '○ alpha')
      wait_for_terminal(alpha)

      -- The agent's terminal is not saved, so alpha's tab is saved with only the sidebar's window.
      local child = restore_in_child(repo, save_session(repo, 'blank,curdir,tabpages,winsize'))

      wait_for_sidebar(child)
      local now = shown(child)
      assert.are.same({}, now.named_like_sidebar)
      assert.are.same({ false, alpha }, directories(child))
      assert.are.same({ 1, 1 }, { now.tabs[1].sidebars, now.tabs[2].sidebars })
      assert.are.equal(1, #now.tabs[2].windows)
      assert.are.same({ '', '' }, { now.tabs[2].windows[1].name, now.tabs[2].windows[1].buftype })
    end
  )

  it(
    'with blank in sessionoptions, a session saved with the sidebar closed leaves it closed',
    function()
      local repo = repo_with({})
      press_on('<CR>', '· alpha')
      vim.cmd('OrcaTerm sidebar')

      local child = restore_in_child(repo, save_session(repo, DEFAULT))

      -- Anything the sidebar would do once the session is loaded has had its turn by then.
      vim.rpcrequest(child, 'nvim_exec_lua', 'vim.wait(200)', {})
      local now = shown(child)
      assert.is_nil(now.sidebar)
      assert.are.same({}, now.named_like_sidebar)
      assert.are.same({ 1, 1 }, { #now.tabs[1].windows, #now.tabs[2].windows })
    end
  )

  it(
    'with blank in sessionoptions, a session sourced with the sidebar already open brings the same '
      .. 'sidebar back in every tab, without E95 or the empty windows the session kept for it',
    function()
      local repo = repo_with({})
      local alpha = support.worktree_path(repo, 'alpha')
      local beta = support.worktree_path(repo, 'beta')
      press_on('<CR>', '· alpha')
      press_on('<CR>', '· beta')
      -- A blank window of the person's own, next to alpha's in its tab.
      vim.cmd('tabnext 2')
      vim.cmd('belowright new')
      vim.cmd('tabnext 3')

      local child, before = restore_in_child(repo, save_session(repo, DEFAULT), true)

      local errmsg = vim.rpcrequest(child, 'nvim_get_vvar', 'errmsg') --[[@as string]]
      assert(not errmsg:find('E95', 1, true), errmsg)
      wait_for_sidebar(child)
      local after = vim.rpcrequest(child, 'nvim_exec_lua', SIDEBAR_BUFFER, {}) --[[@as spec.SidebarBuffer]]
      assert.are.equal(assert(before).buf, after.buf, 'the sidebar keeps its buffer')
      assert.are.equal('orca-term://sidebar', after.name)
      local now = shown(child)
      assert.are.same({}, now.named_like_sidebar, 'no buffer the session made for the sidebar')
      assert.are.same({ false, alpha, beta }, directories(child))
      assert.are.equal(3, now.current)
      for number, tab in ipairs(now.tabs) do
        assert.are.equal(1, tab.sidebars, 'one sidebar window in tab ' .. number)
        for _, window in ipairs(tab.windows) do
          assert.are.same({ '', '' }, { window.name, window.buftype }, 'a blank window')
        end
      end
      assert.are.same({ 1, 2, 1 }, {
        #now.tabs[1].windows,
        #now.tabs[2].windows,
        #now.tabs[3].windows,
      }, "each tab's own windows, the blank one in alpha's tab with them")
      assert(marks_current(child, 'beta'), 'the sidebar marks the current tab')

      -- `<CR>` in the primary checkout's tab goes to alpha's restored tab, making none.
      vim.rpcrequest(child, 'nvim_command', 'tabfirst')
      press_in_child(child, '<CR>', '· alpha')
      support.wait_for(function()
        return shown(child).current == 2
      end, "alpha's restored tab to be current")
      assert.are.same({ false, alpha, beta }, directories(child))
      assert.is_true(marks_current(child, 'alpha'))
    end
  )

  it(
    'a session with no sidebar window, sourced with the sidebar open, leaves the sidebar its buffer '
      .. 'and its name, and shows it in every tab',
    function()
      local repo = repo_with({})
      local alpha = support.worktree_path(repo, 'alpha')
      press_on('<CR>', '· alpha')
      vim.cmd('OrcaTerm sidebar')

      local child, before = restore_in_child(repo, save_session(repo, DEFAULT), true)

      local errmsg = vim.rpcrequest(child, 'nvim_get_vvar', 'errmsg') --[[@as string]]
      assert(not errmsg:find('E95', 1, true), errmsg)
      -- Anything the sidebar would do once the session is loaded has had its turn by then.
      vim.rpcrequest(child, 'nvim_exec_lua', 'vim.wait(200)', {})
      local after = vim.rpcrequest(child, 'nvim_exec_lua', SIDEBAR_BUFFER, {}) --[[@as spec.SidebarBuffer]]
      assert.are.equal(assert(before).buf, after.buf, 'the sidebar keeps its buffer')
      assert.are.equal('orca-term://sidebar', after.name)
      local now = shown(child)
      assert.are.same({}, now.named_like_sidebar)
      -- The sidebar, open, is shown in every tab the session made, beside the session's windows.
      assert.are.same({ false, alpha }, directories(child))
      assert.are.same({ 1, 1 }, { now.tabs[1].sidebars, now.tabs[2].sidebars })
      assert.are.same({ 1, 1 }, { #now.tabs[1].windows, #now.tabs[2].windows })
    end
  )

  it(
    'a session file written before Neovim 0.12, which triggers no SessionLoadPre, sourced with the '
      .. 'sidebar open, brings the same sidebar back in every tab without E95',
    function()
      local repo = repo_with({})
      local alpha = support.worktree_path(repo, 'alpha')
      press_on('<CR>', '· alpha')
      local session = save_session(repo, DEFAULT)
      -- The line Neovim 0.12 added: without it, the file is what Neovim 0.11 writes.
      local file_lines = vim.fn.readfile(session)
      local older = vim.tbl_filter(function(line)
        return line ~= 'doautoall SessionLoadPre'
      end, file_lines)
      assert.are.equal(#file_lines - 1, #older, 'the session triggers SessionLoadPre')
      vim.fn.writefile(older, session)

      local child, before = restore_in_child(repo, session, true)

      local errmsg = vim.rpcrequest(child, 'nvim_get_vvar', 'errmsg') --[[@as string]]
      assert(not errmsg:find('E95', 1, true), errmsg)
      wait_for_sidebar(child)
      local after = vim.rpcrequest(child, 'nvim_exec_lua', SIDEBAR_BUFFER, {}) --[[@as spec.SidebarBuffer]]
      assert.are.equal(assert(before).buf, after.buf, 'the sidebar keeps its buffer')
      assert.are.equal('orca-term://sidebar', after.name)
      local now = shown(child)
      assert.are.same({}, now.named_like_sidebar)
      assert.are.same({ false, alpha }, directories(child))
      assert.are.same({ 1, 1 }, { now.tabs[1].sidebars, now.tabs[2].sidebars })
      assert.are.same({ 1, 1 }, { #now.tabs[1].windows, #now.tabs[2].windows })
    end
  )

  it('a session that fails part way leaves the sidebar its name at its next open', function()
    local repo = repo_with({})
    -- A session that stops with an error before it is loaded: SessionLoadPost never runs.
    local session = repo.tmp .. '/Failing.vim'
    vim.fn.writefile({
      'let SessionLoad = 1',
      'doautoall SessionLoadPre',
      "throw 'the session failed'",
    }, session)

    local child, before = start_child(repo, true)
    local source = 'silent! source ' .. vim.fn.fnameescape(session)
    local sourced = pcall(vim.rpcrequest, child, 'nvim_command', source)
    assert(not sourced, 'the session fails')
    local during = vim.rpcrequest(child, 'nvim_exec_lua', SIDEBAR_BUFFER, {}) --[[@as spec.SidebarBuffer]]
    assert.are_not.equal('orca-term://sidebar', during.name, 'the session took the name off')

    -- The sidebar, still open, is closed and opened again.
    vim.rpcrequest(child, 'nvim_command', 'OrcaTerm sidebar')
    vim.rpcrequest(child, 'nvim_command', 'OrcaTerm sidebar')
    local after = vim.rpcrequest(child, 'nvim_exec_lua', SIDEBAR_BUFFER, {}) --[[@as spec.SidebarBuffer]]
    assert.are.equal(assert(before).buf, after.buf, 'the sidebar keeps its buffer')
    assert.are.equal('orca-term://sidebar', after.name)
    local now = shown(child)
    assert.are.same({}, now.named_like_sidebar)
    assert.are.equal(1, now.tabs[1].sidebars)
  end)

  it(
    "without terminal in sessionoptions, the first <CR> opens the agent's terminal in the restored "
      .. 'tab',
    function()
      local repo = repo_with({ 'alpha' })
      local alpha = support.worktree_path(repo, 'alpha')
      press_on('<CR>', '○ alpha')
      vim.api.nvim_chan_send(wait_for_terminal(alpha), 'before saving\r')
      support.wait_for(function()
        return vim.tbl_contains(support.claude_input(repo), 'before saving')
      end, 'the agent to read what was typed')

      local child = restore_in_child(repo, save_session(repo, LAZYVIM))

      local restored = shown(child).tabs[2]
      assert.are.equal(alpha, restored.directory)
      assert.are.equal(1, #restored.windows, 'the terminal window is not saved')
      assert.are.equal('', restored.windows[1].buftype)

      sidebar_after_restore(child, LAZYVIM)
      press_in_child(child, '<CR>', '○ alpha')
      support.wait_for(function()
        local window = shown(child).tabs[2].windows[1]
        return window.marked and window.running
      end, "the agent's terminal in the restored tab")

      local now = shown(child)
      assert.are.equal(2, #now.tabs)
      assert.are.equal(2, now.current)
      assert.are.equal(1, #now.tabs[2].windows)
      -- The same agent: its holder replays what was typed before the session was saved.
      support.wait_for(function()
        local window = shown(child).tabs[2].windows[1]
        return vim.tbl_contains(window.lines, 'you said: before saving')
      end, 'the replayed scrollback')
    end
  )

  it(
    'with terminal in sessionoptions, the restored terminal starts no agent, and <CR> attaches one '
      .. 'in its place',
    function()
      local repo = repo_with({ 'alpha' })
      local alpha = support.worktree_path(repo, 'alpha')
      press_on('<CR>', '○ alpha')
      vim.api.nvim_chan_send(wait_for_terminal(alpha), 'before saving\r')
      support.wait_for(function()
        return vim.tbl_contains(support.claude_input(repo), 'before saving')
      end, 'the agent to read what was typed')
      -- The fake claude writes this file each time it starts: an agent started again would too.
      local started = repo.log .. '.argv'
      assert(vim.uv.fs_unlink(started))

      local child = restore_in_child(repo, save_session(repo, DEFAULT))

      -- The restored `term://` buffer ran the bare binary, which printed its usage and ended.
      local restored
      support.wait_for(function()
        restored = vim.tbl_filter(function(window)
          return window.buftype == 'terminal'
        end, shown(child).tabs[2].windows)[1]
        return restored ~= nil and not restored.running and #restored.lines > 0
      end, 'the restored terminal to end')
      assert(vim.endswith(restored.name, ':' .. support.binary), restored.name)
      assert(not restored.marked, 'the restored terminal is no agent terminal of orca-term')
      assert(vim.tbl_contains(restored.lines, 'Usage: orca-term <COMMAND>'), restored.lines[1])
      assert(vim.uv.fs_stat(started) == nil, 'no agent started')
      assert(support.listed(repo, 'alpha').holder, 'the holder still runs')

      sidebar_after_restore(child, DEFAULT)
      press_in_child(child, '<CR>', '○ alpha')
      local attached
      support.wait_for(function()
        attached = vim.tbl_filter(function(window)
          return window.marked and window.running
        end, shown(child).tabs[2].windows)[1]
        return attached ~= nil and vim.tbl_contains(attached.lines, 'you said: before saving')
      end, "the agent's terminal, replaying its scrollback, in the restored tab")

      local now = shown(child)
      assert.are.equal(2, #now.tabs)
      assert.are.equal(2, now.current)
      assert(vim.uv.fs_stat(started) == nil, 'still no agent started')
    end
  )

  it(
    'with terminal in sessionoptions, a restored terminal of a stopped agent starts none',
    function()
      local repo = repo_with({ 'alpha' })
      local alpha = support.worktree_path(repo, 'alpha')
      press_on('<CR>', '○ alpha')
      wait_for_terminal(alpha)
      local session = save_session(repo, DEFAULT)
      support.run({ support.binary, 'agent', 'stop', 'alpha' }, repo.root)
      assert.is_false(support.listed(repo, 'alpha').holder)
      local started = repo.log .. '.argv'
      assert(vim.uv.fs_unlink(started))

      local child = restore_in_child(repo, session)

      support.wait_for(function()
        local restored = vim.tbl_filter(function(window)
          return window.buftype == 'terminal'
        end, shown(child).tabs[2].windows)[1]
        return restored ~= nil and not restored.running
      end, 'the restored terminal to end')
      assert(not support.listed(repo, 'alpha').holder, 'no holder started')
      assert(vim.uv.fs_stat(started) == nil, 'no agent started')
    end
  )
end)
