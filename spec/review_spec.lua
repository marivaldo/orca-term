-- Seam 2: the review in diffview.nvim, in headless Neovim against the real binary. `d` on a
-- worktree in the sidebar (and `:OrcaTerm review` in its tab) runs the core's `worktree review`,
-- which marks the worktree seen and prints the review's base, then opens diffview from the
-- worktree's tab, comparing that base with the worktree's files, in a review tab of its own where
-- the sidebar never shows and which `tabs.lua` never takes for the worktree's tab. `gf` there edits
-- the file in the worktree's tab.
--
-- diffview.nvim comes from .rocks/diffview.nvim, pinned by `just lua-rocks` (`just lua-test`
-- refuses to run without it), added to the runtimepath for this file. It stays loaded for the
-- specs after this one, as a plugin would, but every spec closes every diffview view and review
-- tab it opened and deletes the autocommand groups of the review's `gf` and of its notes.

local support = require('support')
local render = require('orca-term.render')
local review = require('orca-term.review')
local sidebar = require('orca-term.sidebar')
local tabs = require('orca-term.tabs')

---Where `just lua-rocks` puts diffview.nvim.
local DIFFVIEW = support.root .. '/.rocks/diffview.nvim'

---diffview's global state, `DiffviewGlobal`, which holds its logger.
---@return table
local function diffview_global()
  -- selene: allow(global_usage)
  return rawget(_G, 'DiffviewGlobal')
end

---The sidebar buffer's lines.
---@return string[]
local function lines()
  return vim.api.nvim_buf_get_lines(assert(sidebar.buffer()), 0, -1, false)
end

---The index of the first sidebar line holding `text`, or `nil`.
---@param text string
---@return integer?
local function line_with(text)
  for index, line in ipairs(lines()) do
    if line:find(text, 1, true) then
      return index
    end
  end
  return nil
end

---The sidebar's window in `tab` (the current tab when `nil`), or `nil` when it is not shown there.
---@param tab? integer
---@return integer?
local function sidebar_window(tab)
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(tab or 0)) do
    if vim.api.nvim_win_get_buf(win) == sidebar.buffer() then
      return win
    end
  end
  return nil
end

---Presses `key` in the sidebar of the current tab, on the name line of the worktree `name`.
---@param key string
---@param name string
local function press_on(key, name)
  local win = assert(sidebar_window(), 'no sidebar in the current tab')
  vim.api.nvim_set_current_win(win)
  vim.api.nvim_win_set_cursor(win, { assert(line_with(' ' .. name), 'no line for ' .. name), 0 })
  vim.cmd('normal ' .. vim.keycode(key))
end

---Whether the sidebar shows the worktree `name` with the unseen mark before its glyph.
---@param name string
---@return boolean
local function shows_unseen(name)
  for _, line in ipairs(lines()) do
    local bare = line:gsub('^ │', '')
    if vim.startswith(bare, render.UNSEEN) and bare:find(' ' .. name, 1, true) then
      return true
    end
  end
  return false
end

---diffview's view in the current tab, or `nil`.
---@return table?
local function current_view()
  return require('diffview.lib').get_current_view()
end

---The paths of the files the view lists, sorted.
---@param view table
---@return string[]
local function listed_files(view)
  local paths = {}
  for _, file in view.files:iter() do
    table.insert(paths, file.path)
  end
  table.sort(paths)
  return paths
end

---Every review tab: tabs holding a diffview view or marked by orca-term.
---@return integer[]
local function review_tabs()
  return vim.tbl_filter(review.is_review, vim.api.nvim_list_tabpages())
end

---Waits until the current tab is a review tab holding a diffview view that lists `count` files.
---@param count integer
---@return table view
local function wait_for_review(count)
  support.wait_for(function()
    local view = current_view()
    return view ~= nil and #listed_files(view) == count and view.cur_entry ~= nil
  end, 'the review to open and list its files')
  return assert(current_view())
end

---The tab-local directory of `tab`.
---@param tab integer
---@return string
local function directory_of(tab)
  return vim.fn.getcwd(-1, vim.api.nvim_tabpage_get_number(tab))
end

---The fixture directory of the spec's repository, whose files `after_each` wipes from the buffers.
---@type string?
local fixture_dir = nil

---@class spec.ReviewRepo: spec.AgentRepo
---@field path string the worktree `fix`
---@field base string the commit `fix` forked from, the head of `main`

---A repository whose worktree `fix` (made by the core) has a commit of its own (`added.txt`, five
---lines), an uncommitted change (`tracked.txt`), a file it deleted without committing (`gone.txt`)
---and a file git does not track (`untracked.txt`); with `agent`, its agent started and a turn
---ended through the hooks, so it is unseen. Neovim is in the primary checkout, sidebar open.
---@param agent boolean
---@return spec.ReviewRepo
local function review_repo(agent)
  support.load_client()
  local repo = support.agent_repo() --[[@as spec.ReviewRepo]]
  fixture_dir = repo.tmp
  vim.fn.writefile({ 'one' }, repo.root .. '/tracked.txt')
  vim.fn.writefile({ 'gone' }, repo.root .. '/gone.txt')
  support.git(repo, { 'add', '.' })
  support.git(repo, { 'commit', '--quiet', '-m', 'base' })
  repo.base = vim.trim(support.git(repo, { 'rev-parse', 'main' }))

  support.run({ support.binary, 'worktree', 'new', 'fix' }, repo.root)
  repo.path = support.worktree_path(repo, 'fix')
  if agent then
    support.run({ support.binary, 'agent', 'start', 'fix' }, repo.root)
    support.play_hooks(repo, repo.path, 'p-hello-2.1.300.log')
  end
  vim.fn.writefile({ 'a1', 'a2', 'a3', 'a4', 'a5' }, repo.path .. '/added.txt')
  support.run({ 'git', 'add', 'added.txt' }, repo.path)
  support.run({ 'git', 'commit', '--quiet', '-m', 'own' }, repo.path)
  vim.fn.writefile({ 'one', 'two' }, repo.path .. '/tracked.txt')
  vim.fn.delete(repo.path .. '/gone.txt')
  vim.fn.writefile({ 'new' }, repo.path .. '/untracked.txt')

  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  return repo
end

---The files the review lists: the commit of its own, the uncommitted change and the uncommitted
---deletion. diffview lists files git does not track only against the index, so `untracked.txt`,
---compared here with a commit, is not among them.
local LISTED = { 'added.txt', 'gone.txt', 'tracked.txt' }

describe('the review', function()
  ---diffview's module while a spec makes `require('diffview')` fail; put back after it.
  ---@type table?
  local hidden_diffview = nil

  -- Unlike the other specs, these leave 'columns' as it is. diffview runs `:redraw` as it opens a
  -- view, and in this Neovim, which has no UI, a redraw after 'columns' changed crashes Neovim now
  -- and then (seen with 160 columns on Neovim 0.12).
  setup(function()
    vim.opt.runtimepath:append(DIFFVIEW)
    require('diffview').setup({ use_icons = false })
    -- diffview logs to a file under stdpath('cache'), which follows the fixture's HOME: Neovim's
    -- own temporary directory outlives every fixture.
    diffview_global().logger.outfile = vim.fn.tempname()
  end)

  teardown(function()
    -- busted forgets the modules and globals this file loaded once it ends, diffview's included:
    -- what diffview left in Neovim must not run after that. Its log is not written at exit, its
    -- autocommands go, and so does it from the runtimepath.
    local logger = diffview_global().logger
    if logger.batch_handle then
      logger.batch_handle.close()
      logger.batch_handle = nil
    end
    pcall(vim.api.nvim_del_augroup_by_name, 'diffview_nvim')
    pcall(vim.api.nvim_del_augroup_by_name, 'diffview_panels')
    vim.opt.runtimepath:remove(DIFFVIEW)
  end)

  after_each(function()
    if hidden_diffview then
      package.preload['diffview'] = nil
      package.loaded['diffview'] = hidden_diffview
      hidden_diffview = nil
    end
    local lib = require('diffview.lib')
    for _, view in ipairs(vim.list_slice(lib.views)) do
      pcall(view.close, view)
      lib.dispose_view(view)
    end
    pcall(vim.api.nvim_del_augroup_by_name, 'orca-term-review')
    require('orca-term.notes')._reset()
    support.reset_editor()
    -- The worktree's files, opened by diffview or by `gf`, go with the fixture.
    for _, buf in ipairs(vim.api.nvim_list_bufs()) do
      local name = vim.api.nvim_buf_get_name(buf)
      if fixture_dir and vim.startswith(name, fixture_dir .. '/') then
        pcall(vim.api.nvim_buf_delete, buf, { force = true })
      end
    end
    fixture_dir = nil
    support.cleanup()
  end)

  it(
    'd opens the review from the worktree in a review tab, without the sidebar, and clears unseen',
    function()
      local repo = review_repo(true)
      assert.is_true(support.listed(repo, 'fix').unseen)
      assert.is_true(shows_unseen('fix'))
      local primary_tab = vim.api.nvim_get_current_tabpage()

      press_on('d', 'fix')
      local view = wait_for_review(#LISTED)

      local review_tab = vim.api.nvim_get_current_tabpage()
      assert.are.equal(repo.path, view.adapter.ctx.toplevel)
      assert.are.equal(repo.base, view.left.commit)
      assert.are.same(LISTED, listed_files(view))
      assert.are.equal(repo.path, review.worktree_of(review_tab))
      -- diffview made it from the worktree's tab, whose directory it inherited.
      assert.are.equal(repo.path, directory_of(review_tab))

      assert(sidebar_window(review_tab) == nil, 'no sidebar in the review tab')
      local worktree_tab = assert(tabs.find_all(repo.path)[1])
      assert.are_not.equal(review_tab, worktree_tab)
      assert.are_not.equal(primary_tab, worktree_tab)
      assert(sidebar_window(worktree_tab), "the sidebar shows in the worktree's tab")
      assert(sidebar_window(primary_tab), "the sidebar shows in the primary checkout's tab")

      assert.is_false(support.listed(repo, 'fix').unseen)
      assert.is_false(shows_unseen('fix'))
    end
  )

  it('d again goes back to the same review tab instead of opening another', function()
    review_repo(false)
    local primary_tab = vim.api.nvim_get_current_tabpage()
    press_on('d', 'fix')
    wait_for_review(#LISTED)
    local review_tab = vim.api.nvim_get_current_tabpage()
    local count = #vim.api.nvim_list_tabpages()

    vim.api.nvim_set_current_tabpage(primary_tab)
    press_on('d', 'fix')
    support.wait_for(function()
      return vim.api.nvim_get_current_tabpage() == review_tab
    end, 'the review tab to become current again')

    assert.are.equal(count, #vim.api.nvim_list_tabpages())
    assert.are.same({ review_tab }, review_tabs())
  end)

  it("is never taken for the worktree's tab", function()
    local repo = review_repo(false)
    press_on('d', 'fix')
    wait_for_review(#LISTED)
    local review_tab = vim.api.nvim_get_current_tabpage()
    local worktree_tab = assert(tabs.find_all(repo.path)[1])

    assert.are.same({ worktree_tab }, tabs.find_all(repo.path))
    assert.is_false(tabs.is_current(repo.path))

    -- Going to the worktree from the review tab lands in its own tab.
    vim.cmd('OrcaTerm worktree 1')
    assert.are.equal(worktree_tab, vim.api.nvim_get_current_tabpage())
    assert.is_true(tabs.is_current(repo.path))

    -- With its own tab closed, going to the worktree makes a new tab, never the review tab.
    vim.cmd.tabclose(vim.api.nvim_tabpage_get_number(worktree_tab))
    vim.api.nvim_set_current_tabpage(review_tab)
    vim.cmd('OrcaTerm worktree 1')
    local new_tab = vim.api.nvim_get_current_tabpage()
    assert.are_not.equal(review_tab, new_tab)
    assert.are.same({ new_tab }, tabs.find_all(repo.path))
  end)

  it(
    'keeps the sidebar out when the sidebar is toggled, and out of a diffview opened by hand',
    function()
      local repo = review_repo(false)
      press_on('d', 'fix')
      wait_for_review(#LISTED)
      local review_tab = vim.api.nvim_get_current_tabpage()

      vim.cmd('OrcaTerm sidebar')
      vim.cmd('OrcaTerm sidebar')
      assert.is_nil(sidebar_window(review_tab))
      assert(sidebar_window(tabs.find_all(repo.path)[1]), "the sidebar shows in the worktree's tab")

      -- A diffview the person opens, from their window in the primary checkout's tab.
      vim.cmd('tabfirst')
      for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
        if win ~= sidebar_window() then
          vim.api.nvim_set_current_win(win)
        end
      end
      require('diffview').open({ '-C' .. vim.fn.fnameescape(repo.path), repo.base })
      local by_hand = vim.api.nvim_get_current_tabpage()
      assert.are_not.equal(review_tab, by_hand)
      assert(current_view(), 'diffview opened a view')
      -- diffview splits its diff windows from the tab's first window, the sidebar's, and fills them
      -- a moment later: then no window shows the sidebar, and diffview's three are all there is.
      support.wait_for(function()
        return sidebar_window(by_hand) == nil
      end, 'no sidebar in a diffview opened by hand')
      assert.are.equal(3, #vim.api.nvim_tabpage_list_wins(by_hand))
      for _, win in ipairs(vim.api.nvim_tabpage_list_wins(by_hand)) do
        if vim.wo[win].diff then
          assert.are.equal(vim.go.signcolumn, vim.wo[win].signcolumn, 'a text window again')
        end
      end
      assert(review.worktree_of(by_hand) == nil, 'orca-term did not open it')
      -- diffview's own gf stays on its file panel there.
      local panel = assert(current_view()).panel.bufid
      local gf = vim.fn.maparg('gf', 'n', false, true)
      vim.api.nvim_buf_call(panel, function()
        gf = vim.fn.maparg('gf', 'n', false, true)
      end)
      assert.is_falsy(tostring(gf.desc):find('orca%-term'))
      -- Nor are the notes' keys there.
      for _, map in ipairs(vim.api.nvim_buf_get_keymap(panel, 'n')) do
        assert.are_not.equal('gn', map.lhs)
      end
    end
  )

  it(
    "gf on the right-hand window edits the file in the worktree's tab, on the same line",
    function()
      local repo = review_repo(false)
      press_on('d', 'fix')
      local view = wait_for_review(#LISTED)
      local added = repo.path .. '/added.txt'
      local main = view.cur_layout:get_main_win()
      support.wait_for(function()
        local buf = vim.api.nvim_win_get_buf(main.id)
        local gf = vim.fn.maparg('gf', 'n', false, true)
        vim.api.nvim_buf_call(buf, function()
          gf = vim.fn.maparg('gf', 'n', false, true)
        end)
        return vim.api.nvim_buf_get_name(buf) == added
          and tostring(gf.desc):find('^orca%-term') ~= nil
      end, "the right-hand window to show added.txt with orca-term's gf")

      vim.api.nvim_set_current_win(main.id)
      vim.api.nvim_win_set_cursor(main.id, { 3, 0 })
      vim.cmd('normal gf')

      assert.are.equal(tabs.find_all(repo.path)[1], vim.api.nvim_get_current_tabpage())
      assert.are.equal(added, vim.api.nvim_buf_get_name(0))
      assert.are.equal(3, vim.api.nvim_win_get_cursor(0)[1])
      assert.are_not.equal(sidebar.buffer(), vim.api.nvim_get_current_buf())
    end
  )

  it('gf on the file panel edits the file, and says so when the change deleted it', function()
    local repo = review_repo(false)
    press_on('d', 'fix')
    local view = wait_for_review(#LISTED)
    local review_tab = vim.api.nvim_get_current_tabpage()
    local ui = support.stub_ui({})

    ---Presses gf on the panel's line holding `name`.
    ---@param name string
    local function gf_on_panel(name)
      vim.api.nvim_set_current_tabpage(review_tab)
      local panel = view.panel
      vim.api.nvim_set_current_win(panel.winid)
      local panel_lines = vim.api.nvim_buf_get_lines(panel.bufid, 0, -1, false)
      for index, line in ipairs(panel_lines) do
        if line:find(name, 1, true) then
          vim.api.nvim_win_set_cursor(panel.winid, { index, 0 })
          vim.cmd('normal gf')
          return
        end
      end
      error('no panel line for ' .. name)
    end

    gf_on_panel('gone.txt')
    assert.are.equal(review_tab, vim.api.nvim_get_current_tabpage())
    assert.are.same({
      {
        message = 'orca-term: gone.txt is not on disk: the change deleted it',
        level = vim.log.levels.WARN,
      },
    }, ui.notes)

    gf_on_panel('tracked.txt')
    assert.are.equal(tabs.find_all(repo.path)[1], vim.api.nvim_get_current_tabpage())
    assert.are.equal(repo.path .. '/tracked.txt', vim.api.nvim_buf_get_name(0))
    assert.are_not.equal(sidebar.buffer(), vim.api.nvim_get_current_buf())
  end)

  it(":OrcaTerm review opens the review of the current tab's worktree, and only there", function()
    local repo = review_repo(true)
    local ui = support.stub_ui({})

    vim.cmd('OrcaTerm review')
    assert.are.same({
      {
        message = "orca-term: the review needs a worktree's tab; go to one, "
          .. 'or press d on a worktree in the sidebar',
        level = vim.log.levels.ERROR,
      },
    }, ui.notes)
    assert.is_true(support.listed(repo, 'fix').unseen)

    vim.cmd('OrcaTerm worktree 1')
    vim.cmd('OrcaTerm review')
    local view = wait_for_review(#LISTED)
    assert.are.equal(repo.path, view.adapter.ctx.toplevel)
    assert.is_false(support.listed(repo, 'fix').unseen)

    -- From the review tab itself, it stays there.
    local review_tab = vim.api.nvim_get_current_tabpage()
    vim.cmd('normal ' .. vim.keycode('<Plug>(OrcaTermReview)'))
    vim.wait(300)
    assert.are.equal(review_tab, vim.api.nvim_get_current_tabpage())
    assert.are.same({ review_tab }, review_tabs())
    assert.are.equal(1, #ui.notes)
  end)

  it('without diffview.nvim says it is needed and leaves the worktree unseen', function()
    local repo = review_repo(true)
    local ui = support.stub_ui({})
    -- As if it were not installed: `require` finds no module, so it runs the preload, which fails.
    hidden_diffview = package.loaded['diffview']
    package.loaded['diffview'] = nil
    package.preload['diffview'] = function()
      error('diffview.nvim is not installed')
    end

    press_on('d', 'fix')
    vim.wait(500)

    assert.are.same({
      {
        message = 'orca-term: the review needs diffview.nvim '
          .. '(https://github.com/sindrets/diffview.nvim)',
        level = vim.log.levels.ERROR,
      },
    }, ui.notes)
    assert.are.equal(1, #vim.api.nvim_list_tabpages())
    assert.is_true(support.listed(repo, 'fix').unseen)
    assert.is_true(shows_unseen('fix'))
  end)

  it("tells the core's refusal and opens nothing, on a broken worktree", function()
    local repo = review_repo(false)
    vim.fn.delete(repo.path, 'rf')
    sidebar.refresh()
    local ui = support.stub_ui({})

    press_on('d', 'fix')
    support.wait_for(function()
      return #ui.notes > 0
    end, 'the refusal')

    assert.are.equal(vim.log.levels.ERROR, ui.notes[1].level)
    assert.truthy(ui.notes[1].message:find('^orca%-term: '))
    assert.truthy(ui.notes[1].message:find('fix', 1, true))
    assert.are.equal(1, #vim.api.nvim_list_tabpages())
    assert.are.same({}, review_tabs())
  end)

  it('closes with the worktree removed from the . menu', function()
    local repo = review_repo(false)
    local primary_tab = vim.api.nvim_get_current_tabpage()
    press_on('d', 'fix')
    wait_for_review(#LISTED)
    vim.api.nvim_set_current_tabpage(primary_tab)
    -- The worktree has changes: removing it asks once more before discarding them.
    local ui = support.stub_ui({ select = { 'Remove worktree', 'Remove', 'Remove anyway' } })

    press_on('.', 'fix')
    support.wait_for(function()
      return #vim.api.nvim_list_tabpages() == 1
    end, "the worktree's tabs to close")

    assert.are.same({}, review_tabs())
    assert.are.same({}, require('diffview.lib').views)
    assert.are.equal(0, vim.fn.isdirectory(repo.path))
    assert.truthy(ui.notes[1].message:find('removed worktree fix', 1, true))
  end)
end)
