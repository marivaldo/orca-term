-- Seam 2: the sidebar's worktree actions, in headless Neovim against the real binary: `a` creates a
-- worktree with no agent without blocking Neovim and lands in its tab (with an agent, see
-- spec/agent_spec.lua), and `.` offers running the setup again, copying the path and removing the
-- worktree, which always asks first, asks again before discarding changes, and closes the
-- worktree's tabs. `vim.ui` and `vim.notify` are stubbed; the core never is.

local support = require('support')
local sidebar = require('orca-term.sidebar')

-- A clipboard of the spec's own, so copying a path never reaches the developer's clipboard. It
-- must be set before Neovim first looks for a clipboard.
local clipboard = {}
vim.g.clipboard = {
  name = 'spec',
  copy = {
    ['+'] = function(copied)
      clipboard['+'] = copied
    end,
    ['*'] = function(copied)
      clipboard['*'] = copied
    end,
  },
  paste = {
    ['+'] = function()
      return clipboard['+'] or {}
    end,
    ['*'] = function()
      return clipboard['*'] or {}
    end,
  },
}

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

---Presses `key` in the sidebar of the current tab, on the line holding `text`.
---@param key string
---@param text string
local function press_on(key, text)
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_buf(win) == sidebar.buffer() then
      vim.api.nvim_set_current_win(win)
      vim.api.nvim_win_set_cursor(win, { assert(line_with(text), text), 0 })
      vim.cmd('normal ' .. vim.keycode(key))
      return
    end
  end
  error('no sidebar in the current tab')
end

---The tab-local directory of the current tab.
---@return string
local function tab_directory()
  return vim.fn.getcwd(-1, vim.fn.tabpagenr())
end

---Whether the fleet lists a worktree named `name`.
---@param repo spec.Repo
---@param name string
---@return boolean
local function listed(repo, name)
  return (pcall(support.worktree_path, repo, name))
end

---Waits until a notification arrives, and returns it.
---@param ui spec.Ui
---@return { message: string, level: integer }
local function next_note(ui)
  local seen = #ui.notes
  support.wait_for(function()
    return #ui.notes > seen
  end, 'a notification')
  return ui.notes[seen + 1]
end

---A setup script that waits until the file `gate` exists, so a test decides when it ends.
---@param gate string
---@return string[]
local function gated_setup(gate)
  return { 'while [ ! -e "' .. gate .. '" ]; do sleep 0.05; done' }
end

---A repository with worktrees named `names`, made by the core, Neovim in its primary checkout and
---the sidebar open.
---@param names string[]
---@return spec.Repo
local function repo_with(names)
  support.load_client()
  local repo = support.repo()
  for _, name in ipairs(names) do
    support.run({ support.binary, 'worktree', 'new', name }, repo.root)
  end
  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  return repo
end

describe('the worktree actions', function()
  local columns = vim.o.columns

  before_each(function()
    vim.o.columns = 160
  end)

  after_each(function()
    support.reset_editor()
    support.cleanup()
    vim.o.columns = columns
  end)

  describe('a', function()
    it('creates the worktree through the core and lands in its tab', function()
      local repo = repo_with({ 'alpha' })
      local ui = support.stub_ui({ input = 'gamma', select = { 'No agent' } })

      press_on('a', '· alpha')

      assert.are.same({ 'New worktree name: ', 'Agent for gamma' }, ui.prompts)
      assert.are.same({ { 'claude-code', 'No agent' } }, ui.shown)
      assert.are.same({
        message = 'orca-term: created worktree gamma',
        level = vim.log.levels.INFO,
      }, next_note(ui))
      assert.are.equal(support.worktree_path(repo, 'gamma'), tab_directory())
      assert.are.equal(2, #vim.api.nvim_list_tabpages())
      assert(line_with('│ · gamma'), 'the new worktree is selected')
    end)

    it('shows the worktree as setting up while its setup runs, without blocking', function()
      local repo = repo_with({})
      local gate = repo.tmp .. '/gate'
      support.set_setup(repo, gated_setup(gate))
      local ui = support.stub_ui({ input = 'slow', select = { 'No agent' } })

      press_on('a', 'no worktrees yet')

      -- Neovim keeps running while the core waits on the setup.
      support.wait_for(function()
        return line_with('◌ slow') ~= nil
      end, 'the sidebar to show slow setting up')
      assert.are.same({}, ui.notes)
      vim.fn.writefile({}, gate)
      assert.are.equal('orca-term: created worktree slow', next_note(ui).message)
      assert.truthy(line_with('│ · slow'))
    end)

    it('warns with the detail when the setup fails, and still lands in the tab', function()
      local repo = repo_with({})
      support.set_setup(repo, { 'echo boom', 'exit 3' })
      local ui = support.stub_ui({ input = 'broken-setup', select = { 'No agent' } })

      press_on('a', 'no worktrees yet')

      local note = next_note(ui)
      assert.are.equal(vim.log.levels.WARN, note.level)
      assert.are.equal(
        'orca-term: created worktree broken-setup, but its setup failed: setup failed (exit 3): boom',
        note.message
      )
      assert.are.equal(support.worktree_path(repo, 'broken-setup'), tab_directory())
    end)

    it("shows the core's error for an invalid name, and does nothing for an empty one", function()
      repo_with({ 'alpha' })
      local ui = support.stub_ui({ input = 'bad/name', select = { 'No agent' } })

      press_on('a', '· alpha')

      local note = next_note(ui)
      assert.are.equal(vim.log.levels.ERROR, note.level)
      assert(note.message:find('cannot contain `/`', 1, true), note.message)
      assert(vim.startswith(note.message, 'orca-term: '), note.message)
      assert.are.equal(1, #vim.api.nvim_list_tabpages())

      support.stub_ui({ input = '' })
      press_on('a', '· alpha')
      vim.wait(200)
      assert.are.equal(1, #vim.api.nvim_list_tabpages())
    end)
  end)

  describe('.', function()
    it('offers the actions of the worktree under the cursor', function()
      repo_with({ 'alpha', 'beta' })
      local ui = support.stub_ui({})

      press_on('.', '· beta')

      assert.are.same({ 'Worktree beta' }, ui.prompts)
      assert.are.same(
        { { 'Re-run setup', 'Copy path', 'Swap agent', 'Remove worktree' } },
        ui.shown
      )
    end)

    it('runs the setup again', function()
      local repo = repo_with({ 'alpha' })
      support.set_setup(repo, { 'exit 3' })
      local ui = support.stub_ui({ select = { 'Re-run setup' } })
      press_on('.', '· alpha')
      assert.are.equal(vim.log.levels.WARN, next_note(ui).level)
      assert.truthy(line_with('△ alpha'))

      support.set_setup(repo, { 'true' })
      ui = support.stub_ui({ select = { 'Re-run setup' } })
      press_on('.', '△ alpha')

      assert.are.same({
        message = 'orca-term: worktree alpha is ready',
        level = vim.log.levels.INFO,
      }, next_note(ui))
      assert.truthy(line_with('· alpha'))
    end)

    it('copies the path to the unnamed register and the clipboard', function()
      local repo = repo_with({ 'alpha' })
      local path = support.worktree_path(repo, 'alpha')
      local ui = support.stub_ui({ select = { 'Copy path' } })

      press_on('.', '· alpha')

      assert.are.equal(path, vim.fn.getreg('"'))
      assert.are.same({ path }, clipboard['+'])
      assert.are.same(
        { { message = 'orca-term: copied ' .. path, level = vim.log.levels.INFO } },
        ui.notes
      )
    end)

    it('asks before removing, and Cancel leaves the worktree', function()
      local repo = repo_with({ 'alpha' })
      local ui = support.stub_ui({ select = { 'Remove worktree', 'Cancel' } })

      press_on('.', '· alpha')
      vim.wait(200)

      assert.are.same({
        'Worktree alpha',
        'Remove worktree alpha? Its branch is deleted only if merged.',
      }, ui.prompts)
      assert.are.same({ 'Remove', 'Cancel' }, ui.shown[2])
      assert.is_true(listed(repo, 'alpha'))
      assert.are.same({}, ui.notes)
    end)

    it('removes the worktree on Remove, closing its tabs', function()
      local repo = repo_with({ 'alpha', 'beta' })
      local alpha = support.worktree_path(repo, 'alpha')
      press_on('<CR>', '· alpha')
      vim.cmd.tabnew()
      vim.cmd.tcd(vim.fn.fnameescape(alpha))
      vim.cmd.tabfirst()
      assert.are.equal(3, #vim.api.nvim_list_tabpages())
      local ui = support.stub_ui({ select = { 'Remove worktree', 'Remove' } })

      press_on('.', '· alpha')

      assert.are.same({
        message = 'orca-term: removed worktree alpha and its branch alpha (merged into main)',
        level = vim.log.levels.INFO,
      }, next_note(ui))
      assert.are.equal(1, #vim.api.nvim_list_tabpages())
      assert.is_false(listed(repo, 'alpha'))
      assert.are.equal(0, vim.fn.isdirectory(alpha))
      assert.is_nil(line_with('alpha'))
      assert.truthy(line_with('│ · beta'))
    end)

    it('points the last tab at the primary checkout instead of closing it', function()
      local repo = repo_with({ 'alpha' })
      press_on('<CR>', '· alpha')
      vim.cmd('tabonly')
      local ui = support.stub_ui({ select = { 'Remove worktree', 'Remove' } })

      press_on('.', '· alpha')

      assert.are.equal(vim.log.levels.INFO, next_note(ui).level)
      assert.are.equal(1, #vim.api.nvim_list_tabpages())
      assert.are.equal(repo.root, tab_directory())
      assert.truthy(line_with('no worktrees yet'))
    end)

    it('asks again before discarding changes, and Remove anyway forces it', function()
      local repo = repo_with({ 'alpha' })
      local alpha = support.worktree_path(repo, 'alpha')
      vim.fn.writefile({ 'unsaved' }, alpha .. '/scratch.txt')

      local ui = support.stub_ui({ select = { 'Remove worktree', 'Remove', 'Cancel' } })
      press_on('.', '· alpha')
      support.wait_for(function()
        return #ui.prompts == 3
      end, 'the second confirmation')
      assert.are.equal(
        'alpha has uncommitted changes. Remove anyway, discarding them and deleting its branch '
          .. 'even if unmerged?',
        ui.prompts[3]
      )
      assert.are.same({ 'Remove anyway', 'Cancel' }, ui.shown[3])
      assert.is_true(listed(repo, 'alpha'))

      ui = support.stub_ui({ select = { 'Remove worktree', 'Remove', 'Remove anyway' } })
      press_on('.', '· alpha')

      assert.are.equal(
        'orca-term: removed worktree alpha and its branch alpha (--force)',
        next_note(ui).message
      )
      assert.is_false(listed(repo, 'alpha'))
    end)

    it('shows why a busy worktree cannot be removed or set up, offering no force', function()
      local repo = repo_with({ 'alpha' })
      local gate = repo.tmp .. '/gate'
      support.set_setup(repo, gated_setup(gate))
      -- A setup running in the worktree holds its lock until the gate opens.
      local running = vim.system(
        { support.binary, 'worktree', 'setup', 'alpha' },
        { cwd = repo.root, text = true }
      )
      vim.cmd('doautocmd FocusGained')
      support.wait_for(function()
        vim.cmd('doautocmd FocusGained')
        return line_with('◌ alpha') ~= nil
      end, 'the setup to start')

      local ui = support.stub_ui({ select = { 'Remove worktree', 'Remove' } })
      press_on('.', '◌ alpha')
      local note = next_note(ui)
      assert.are.equal(vim.log.levels.ERROR, note.level)
      assert(note.message:find('is busy', 1, true), note.message)
      assert.are.equal(2, #ui.prompts, 'no second confirmation')

      ui = support.stub_ui({ select = { 'Re-run setup' } })
      press_on('.', '◌ alpha')
      note = next_note(ui)
      assert.are.equal(vim.log.levels.ERROR, note.level)
      assert(note.message:find('is busy', 1, true), note.message)

      vim.fn.writefile({}, gate)
      running:wait()
      assert.is_true(listed(repo, 'alpha'))
    end)
  end)
end)
