-- Seam 2: switching between worktrees, in headless Neovim against the real binary. Each worktree
-- gets a tab whose tab-local directory is the worktree: `<CR>` in the sidebar goes to it (making it
-- when missing), `:OrcaTerm jump`, `:OrcaTerm worktree N` and their `<Plug>` maps do too, the
-- sidebar's selection follows the current tab, and closing a tab never touches the worktree.

local support = require('support')
local sidebar = require('orca-term.sidebar')

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

---The sidebar's window in the current tab.
---@return integer
local function sidebar_window()
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_buf(win) == sidebar.buffer() then
      return win
    end
  end
  error('no sidebar in the current tab')
end

---Presses `key` in the sidebar of the current tab, on line `line`.
---@param key string
---@param line integer
local function press(key, line)
  local win = sidebar_window()
  vim.api.nvim_set_current_win(win)
  vim.api.nvim_win_set_cursor(win, { line, 0 })
  vim.cmd('normal ' .. vim.keycode(key))
end

---The tab-local directory of the current tab.
---@return string
local function tab_directory()
  return vim.fn.getcwd(-1, vim.fn.tabpagenr())
end

---A repository with worktrees named `names`, made by the core, and Neovim in its primary checkout.
---@param names string[]
---@return spec.Repo
local function repo_with(names)
  support.load_client()
  local repo = support.repo()
  for _, name in ipairs(names) do
    support.run({ support.binary, 'worktree', 'new', name }, repo.root)
  end
  support.cd(repo.root)
  return repo
end

describe('switching worktrees', function()
  local columns = vim.o.columns

  before_each(function()
    vim.o.columns = 160
  end)

  after_each(function()
    support.reset_editor()
    support.cleanup()
    vim.o.columns = columns
  end)

  it(
    "<CR> makes the worktree's tab with its directory and focuses its main window, then reuses it",
    function()
      local repo = repo_with({ 'alpha', 'beta' })
      local beta = support.worktree_path(repo, 'beta')
      vim.cmd('OrcaTerm sidebar')
      local first = vim.api.nvim_get_current_tabpage()

      press('<CR>', assert(line_with('· beta')))

      local tabs = vim.api.nvim_list_tabpages()
      assert.are.equal(2, #tabs)
      assert.are.equal(tabs[2], vim.api.nvim_get_current_tabpage())
      assert.are.equal(beta, tab_directory())
      assert.are.equal(1, vim.fn.haslocaldir(-1, 2), 'the directory is local to the tab')
      assert.are_not.equal(sidebar.buffer(), vim.api.nvim_get_current_buf())
      assert(sidebar_window(), 'the sidebar shows in the new tab')

      vim.api.nvim_set_current_tabpage(first)
      press('<CR>', assert(line_with('· beta')))

      assert.are.equal(2, #vim.api.nvim_list_tabpages())
      assert.are.equal(tabs[2], vim.api.nvim_get_current_tabpage())
      assert.are_not.equal(sidebar.buffer(), vim.api.nvim_get_current_buf())
    end
  )

  it('<CR> on the header or a blank line does nothing', function()
    repo_with({ 'alpha' })
    vim.cmd('OrcaTerm sidebar')

    press('<CR>', 1)
    press('<CR>', 2)

    assert.are.equal(1, #vim.api.nvim_list_tabpages())
    assert.are.equal(sidebar.buffer(), vim.api.nvim_get_current_buf())
  end)

  it("the selection follows the current tab when it is a worktree's", function()
    local repo = repo_with({ 'alpha', 'beta' })
    vim.cmd('OrcaTerm sidebar')
    local primary_tab = vim.api.nvim_get_current_tabpage()
    assert.truthy(line_with('│ · alpha'))

    press('<CR>', assert(line_with('· beta')))
    local beta_tab = vim.api.nvim_get_current_tabpage()
    assert.truthy(line_with('│ · beta'))

    -- The primary checkout's tab keeps the selection where it was.
    vim.api.nvim_set_current_tabpage(primary_tab)
    assert.truthy(line_with('│ · beta'))

    -- A tab pointed at a worktree by hand selects it too.
    vim.cmd.tabnew()
    vim.cmd.tcd(vim.fn.fnameescape(support.worktree_path(repo, 'alpha')))
    assert.truthy(line_with('│ · alpha'))

    vim.api.nvim_set_current_tabpage(beta_tab)
    assert.truthy(line_with('│ · beta'))
    local cursor = vim.api.nvim_win_get_cursor(sidebar_window())[1]
    assert.are.equal(line_with('│ · beta'), cursor, "the sidebar's cursor follows the card")
  end)

  it("closing a worktree's tab leaves the worktree as it was", function()
    local repo = repo_with({ 'alpha' })
    local alpha = support.worktree_path(repo, 'alpha')
    vim.cmd('OrcaTerm sidebar')
    press('<CR>', assert(line_with('· alpha')))
    assert.are.equal(2, #vim.api.nvim_list_tabpages())

    vim.cmd.tabclose()

    assert.are.equal(1, #vim.api.nvim_list_tabpages())
    assert.are.equal(alpha, support.worktree_path(repo, 'alpha'))
    assert.are.equal(1, vim.fn.isdirectory(alpha))
    assert.truthy(line_with('· alpha'))
  end)

  it(':OrcaTerm jump picks a worktree in sidebar order and goes to its tab', function()
    local repo = repo_with({ 'beta', 'alpha' })
    local before = support.global_keymaps()
    local ui = support.stub_ui({ select = { 'beta  no agent' } })

    -- The sidebar is closed: jumping reads the fleet itself.
    vim.cmd('OrcaTerm jump')

    assert.are.same({ { 'alpha  no agent', 'beta  no agent' } }, ui.shown)
    assert.are.same({ 'Go to worktree' }, ui.prompts)
    assert.are.equal(support.worktree_path(repo, 'beta'), tab_directory())
    assert.are.equal(2, #vim.api.nvim_list_tabpages())
    assert.are.same(before, support.global_keymaps())
  end)

  it(':OrcaTerm jump does nothing when the pick is cancelled', function()
    repo_with({ 'alpha' })
    support.stub_ui({})

    vim.cmd('OrcaTerm jump')

    assert.are.equal(1, #vim.api.nvim_list_tabpages())
  end)

  it(':OrcaTerm worktree N goes to the Nth worktree, and says when there is none', function()
    local repo = repo_with({ 'alpha', 'beta' })
    local ui = support.stub_ui({})

    vim.cmd('OrcaTerm worktree 2')
    assert.are.equal(support.worktree_path(repo, 'beta'), tab_directory())

    vim.cmd('OrcaTerm worktree 1')
    assert.are.equal(support.worktree_path(repo, 'alpha'), tab_directory())
    assert.are.equal(3, #vim.api.nvim_list_tabpages())

    vim.cmd('OrcaTerm worktree 3')
    vim.cmd('OrcaTerm worktree x')
    assert.are.equal(3, #vim.api.nvim_list_tabpages())
    assert.are.same({
      {
        message = 'orca-term: there is no worktree 3; this repository has 2',
        level = vim.log.levels.ERROR,
      },
      {
        message = "orca-term: `:OrcaTerm worktree` takes the worktree's number in the sidebar, "
          .. 'from 1, such as `:OrcaTerm worktree 2`',
        level = vim.log.levels.ERROR,
      },
    }, ui.notes)
  end)

  it('the <Plug> maps go to the Nth worktree and to the jump list', function()
    local repo = repo_with({ 'alpha', 'beta' })
    local ui = support.stub_ui({ select = { 'alpha  no agent' } })

    vim.cmd('normal ' .. vim.keycode('<Plug>(OrcaTermWorktree2)'))
    assert.are.equal(support.worktree_path(repo, 'beta'), tab_directory())

    vim.cmd('normal ' .. vim.keycode('<Plug>(OrcaTermJump)'))
    assert.are.same({ 'Go to worktree' }, ui.prompts)
    assert.are.equal(support.worktree_path(repo, 'alpha'), tab_directory())
  end)
end)
