-- Seam 2: the sidebar in headless Neovim, against the real binary: where it opens, what toggling
-- does, the selection keys, refreshing on focus, its width and its colours. The rendered look
-- itself is checked by spec/sidebar_golden_spec.lua.

local support = require('support')
local sidebar = require('orca-term.sidebar')

---The windows showing the sidebar in `tab`.
---@param tab integer
---@return integer[]
local function sidebar_windows(tab)
  local found = {}
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(tab)) do
    if vim.api.nvim_win_get_buf(win) == sidebar.buffer() then
      table.insert(found, win)
    end
  end
  return found
end

---The sidebar buffer's lines.
---@return string[]
local function lines()
  return vim.api.nvim_buf_get_lines(assert(sidebar.buffer()), 0, -1, false)
end

---The index of the first line holding `text`, or `nil`.
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

describe('the sidebar', function()
  local columns = vim.o.columns

  before_each(function()
    vim.o.columns = 160
  end)

  after_each(function()
    support.reset_editor()
    support.cleanup()
    vim.o.columns = columns
  end)

  it('opens in every tab without leaving the current window, and in tabs made later', function()
    repo_with({ 'fix' })
    vim.cmd.tabnew()
    vim.cmd.tabnew()
    local current = vim.api.nvim_get_current_win()

    vim.cmd('OrcaTerm sidebar')

    assert.are.equal(current, vim.api.nvim_get_current_win())
    for _, tab in ipairs(vim.api.nvim_list_tabpages()) do
      local windows = sidebar_windows(tab)
      assert.are.equal(1, #windows)
      assert.are.equal(36, vim.api.nvim_win_get_width(windows[1]))
      -- At the far left of the tab.
      assert.are.equal(windows[1], vim.api.nvim_tabpage_list_wins(tab)[1])
    end

    vim.cmd.tabnew()
    assert.are.equal(1, #sidebar_windows(vim.api.nvim_get_current_tabpage()))
  end)

  it('closes in every tab when toggled again, leaving the other windows as they were', function()
    repo_with({ 'fix' })
    -- Two windows of unequal widths in the current tab, where 'equalalways' would even them out.
    vim.cmd.tabnew()
    vim.cmd.vsplit()
    vim.api.nvim_win_set_width(0, 100)
    local function layout()
      local widths = {}
      for _, tab in ipairs(vim.api.nvim_list_tabpages()) do
        for _, win in ipairs(vim.api.nvim_tabpage_list_wins(tab)) do
          table.insert(widths, win .. ':' .. vim.api.nvim_win_get_width(win))
        end
      end
      return widths
    end
    local before = layout()

    vim.cmd('OrcaTerm sidebar')
    vim.cmd('OrcaTerm sidebar')

    assert.is_false(sidebar.is_open())
    for _, tab in ipairs(vim.api.nvim_list_tabpages()) do
      assert.are.same({}, sidebar_windows(tab))
    end
    assert.are.same(before, layout())
    vim.cmd.tabnew()
    assert.are.same({}, sidebar_windows(vim.api.nvim_get_current_tabpage()))
  end)

  it(
    ':q on the last other window of a tab closes the tab, and the sidebar stays elsewhere',
    function()
      repo_with({ 'fix' })
      vim.cmd.tabnew()
      vim.cmd('OrcaTerm sidebar')
      local first = vim.api.nvim_list_tabpages()[1]

      vim.cmd('quit')

      assert.are.same({ first }, vim.api.nvim_list_tabpages())
      assert.are.equal(1, #sidebar_windows(first))
      assert.is_true(sidebar.is_open())
    end
  )

  it(':close on the last other window of a tab closes the tab too', function()
    repo_with({ 'fix' })
    vim.cmd.tabnew()
    vim.cmd('OrcaTerm sidebar')
    local first = vim.api.nvim_list_tabpages()[1]

    vim.cmd('close')
    -- The tab is closed once the window is gone, on the next turn of the event loop.
    vim.wait(1000, function()
      return #vim.api.nvim_list_tabpages() == 1
    end)

    assert.are.same({ first }, vim.api.nvim_list_tabpages())
    assert.are.equal(1, #sidebar_windows(first))
  end)

  it('lets :q quit Neovim from the last tab, as it would without the sidebar', function()
    local repo = repo_with({ 'fix' })
    -- A child Neovim with only this client loaded; it must exit on its own.
    local child = vim
      .system({
        'nvim',
        '--clean',
        '--headless',
        '--cmd',
        'set runtimepath^=' .. vim.fn.fnameescape(support.root),
        '-c',
        "lua require('orca-term').setup({ binary = '" .. support.binary .. "' })",
        '-c',
        'OrcaTerm sidebar',
        '-c',
        "lua assert(require('orca-term.sidebar').is_open())",
        '-c',
        'quit',
      }, { cwd = repo.root, text = true, timeout = 10000 })
      :wait()
    assert.are.equal(0, child.code, child.stderr)
  end)

  it('moves the card with j and k without changing the tab', function()
    repo_with({ 'alpha', 'beta', 'gamma' })
    vim.cmd.tabnew()
    vim.cmd('OrcaTerm sidebar')
    local tab = vim.api.nvim_get_current_tabpage()
    local win = sidebar_windows(tab)[1]
    vim.api.nvim_set_current_win(win)

    -- The first worktree starts selected: its name is inside the card.
    assert.are.equal(line_with('│ · alpha'), vim.api.nvim_win_get_cursor(win)[1])

    vim.cmd('normal j')
    assert.truthy(line_with('│ · beta'))
    assert.truthy(line_with(' · alpha'))
    assert.is_nil(line_with('│ · alpha'))
    assert.are.equal(line_with('│ · beta'), vim.api.nvim_win_get_cursor(win)[1])
    assert.are.equal(tab, vim.api.nvim_get_current_tabpage())

    vim.cmd('normal jj')
    assert(line_with('│ · gamma'), 'j stops at the last worktree')

    vim.cmd('normal kkk')
    assert(line_with('│ · alpha'), 'k stops at the first worktree')
    assert.are.equal(tab, vim.api.nvim_get_current_tabpage())
  end)

  it('reads the fleet again when Neovim gains focus', function()
    local repo = repo_with({ 'fix' })
    vim.cmd('OrcaTerm sidebar')
    assert.is_nil(line_with('later'))

    support.run({ support.binary, 'worktree', 'new', 'later' }, repo.root)
    vim.cmd('doautocmd FocusGained')

    assert.truthy(line_with('· later'))
  end)

  it('shows the primary checkout only in the header, never as a row', function()
    repo_with({ 'fix' })
    vim.cmd('OrcaTerm sidebar')
    local shown = lines()
    assert.are.equal(' ▾ repo  main', shown[1])
    for index = 2, #shown do
      assert(not shown[index]:find('repo', 1, true), shown[index])
      assert(not shown[index]:find('main', 1, true), shown[index])
    end
  end)

  it('draws no line wider than 36 cells', function()
    repo_with({ 'a-very-long-worktree-name-that-never-fits-in-the-sidebar', 'short' })
    vim.cmd('OrcaTerm sidebar')
    vim.cmd('normal! G')
    for _, line in ipairs(lines()) do
      assert(vim.fn.strdisplaywidth(line) <= 36, line)
    end
    assert.truthy(line_with('…'))
  end)

  it('shows why the fleet cannot be read, outside a git repository', function()
    support.load_client()
    local outside = support.isolate().tmp
    support.cd(outside)

    vim.cmd('OrcaTerm sidebar')

    local shown = lines()
    assert.are.equal(' ▾ orca-term', shown[1])
    local text = table.concat(vim.tbl_map(vim.trim, vim.list_slice(shown, 3)), ' ')
    assert(text:find('not inside a git repository', 1, true), text)
    for _, line in ipairs(shown) do
      assert(vim.fn.strdisplaywidth(line) <= 36, line)
    end
  end)

  it('links its colours to the colorscheme, keeping a group the person defined', function()
    repo_with({ 'fix' })
    vim.api.nvim_set_hl(0, 'OrcaTermStateBroken', { fg = '#ff0000' })

    vim.cmd('OrcaTerm sidebar')

    assert.are.equal(
      '#ff0000',
      ('#%06x'):format(vim.api.nvim_get_hl(0, { name = 'OrcaTermStateBroken' }).fg)
    )
    local links = {
      OrcaTermStateNoAgent = 'DiagnosticUnnecessary',
      OrcaTermStateSettingUp = 'DiagnosticInfo',
      OrcaTermStateNotReady = 'DiagnosticWarn',
      OrcaTermHeader = 'Title',
      OrcaTermBorder = 'FloatBorder',
      OrcaTermDenied = 'DiagnosticWarn',
    }
    for group, link in pairs(links) do
      assert.are.equal(link, vim.api.nvim_get_hl(0, { name = group }).link, group)
    end
    vim.api.nvim_set_hl(0, 'OrcaTermStateBroken', {})
  end)

  it('defines no global keymap when opened and closed', function()
    repo_with({ 'fix' })
    local before = support.global_keymaps()

    vim.cmd('OrcaTerm sidebar')
    assert.are.same(before, support.global_keymaps())
    vim.cmd('OrcaTerm sidebar')
    assert.are.same(before, support.global_keymaps())

    local keys = {}
    for _, keymap in ipairs(vim.api.nvim_buf_get_keymap(assert(sidebar.buffer()), 'n')) do
      table.insert(keys, keymap.lhs)
    end
    table.sort(keys)
    assert.are.same({ '.', '<CR>', 'K', 'a', 'd', 'j', 'k', 'u' }, keys)
  end)
end)
