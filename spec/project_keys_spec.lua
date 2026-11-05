-- Seam 2: the sidebar's project keys (#155), in headless Neovim against the real binary: `a`
-- creates a worktree in the project the cursor is in, `P` adds a project to the project list and
-- `.` on a project's header offers `Remove from list`, which asks first and touches nothing on
-- disk. The project list lives in the fixture's XDG_STATE_HOME (`support.isolate()`), never in
-- the developer's. `vim.ui` and `vim.notify` are stubbed; the core never is.

local support = require('support')
local core = require('orca-term.core')
local render = require('orca-term.render')
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

---The header the sidebar's line `line` is under: the nearest line above it that starts a project.
---@param line integer
---@return string?
local function header_above(line)
  local shown = lines()
  for index = line, 1, -1 do
    if vim.startswith(shown[index], ' ▾ ') then
      return shown[index]
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

---Presses `key` in the sidebar of the current tab, on the line holding `text`.
---@param key string
---@param text string
local function press_on(key, text)
  local win = sidebar_window()
  vim.api.nvim_set_current_win(win)
  vim.api.nvim_win_set_cursor(win, { assert(line_with(text), text), 0 })
  vim.cmd('normal ' .. vim.keycode(key))
end

---Whether a notification holding `text` arrived.
---@param ui spec.Ui
---@param text string
---@return boolean
local function told(ui, text)
  for _, note in ipairs(ui.notes) do
    if note.message:find(text, 1, true) then
      return true
    end
  end
  return false
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

---Makes a worktree in `repo` through the core.
---@param repo spec.Repo
---@param name string
local function new_worktree(repo, name)
  support.run({ support.binary, 'worktree', 'new', name }, repo.root)
end

---Whether the fleet of `repo` lists a worktree named `name`.
---@param repo spec.Repo
---@param name string
---@return boolean
local function listed(repo, name)
  return (pcall(support.worktree_path, repo, name))
end

---The paths on the project list, in its order, as `project ls --json` prints them.
---@param repo spec.Repo any repository of the fixture, whose project list it is
---@return string[]
local function project_paths(repo)
  local out = support.run({ support.binary, 'project', 'ls', '--json' }, repo.tmp)
  return vim.tbl_map(function(project)
    return project.path
  end, vim.json.decode(out).projects)
end

---Two repositories of one fixture: `repo`, with the worktree `one`, and `api`, with the worktree
---`two`. Neither is on the project list yet.
---@return spec.Repo repo
---@return spec.Repo api
local function two_repositories()
  support.load_client()
  local repo = support.repo()
  local api = support.sibling_repo(repo, 'api')
  new_worktree(repo, 'one')
  new_worktree(api, 'two')
  return repo, api
end

---Two projects on the list, in this order: `repo`, with the worktree `one`, then `api`, with the
---worktree `two`.
---@return spec.Repo repo the first project
---@return spec.Repo api the second project
local function two_projects()
  local repo, api = two_repositories()
  support.add_project(repo)
  support.add_project(api)
  return repo, api
end

---Presses `a` on the sidebar line holding `text`, with Neovim sitting in `other`, names the
---worktree `fresh` with no agent, and checks it was created in `project` and nowhere else: its
---fleet lists it, its tab is current and the sidebar shows it under that project's header.
---@param text string
---@param project spec.Repo the project the cursor is in
---@param other spec.Repo the other project, where Neovim sits
local function creates_in(text, project, other)
  support.cd(other.root)
  local ui = support.stub_ui({ input = 'fresh', select = { 'No agent' } })
  vim.cmd('OrcaTerm sidebar')

  press_on('a', text)

  support.wait_for(function()
    return told(ui, 'created worktree fresh')
  end, 'the creation of fresh')
  assert.are.same({ 'New worktree name: ', 'Agent for fresh' }, ui.prompts)
  assert.is_true(listed(project, 'fresh'))
  assert.is_false(listed(other, 'fresh'))
  assert.are.equal(support.worktree_path(project, 'fresh'), vim.fn.getcwd(-1, vim.fn.tabpagenr()))
  local row = assert(line_with('│ · fresh'), 'the new worktree is selected')
  local name = vim.fs.basename(project.root)
  assert.are.equal((' ▾ %s  main'):format(name), header_above(row))
end

describe("the sidebar's project keys", function()
  local columns = vim.o.columns

  before_each(function()
    vim.o.columns = 160
  end)

  after_each(function()
    support.reset_editor()
    support.cleanup()
    vim.o.columns = columns
  end)

  it("knows each project's header line from what render.lua drew", function()
    local repo = two_projects()
    local fleets = assert(core.fleets(repo.tmp))

    local rendered = render.projects(fleets.projects)

    assert.are.equal(' ▾ repo  main', rendered.lines[1])
    assert.are.equal(' ▾ api  main', rendered.lines[8])
    assert.are.same({ [1] = 1, [8] = 2 }, rendered.headers)
    -- One repository alone, the sidebar of an empty project list, has no project header.
    assert.are.same({}, render.fleet(fleets.projects[1].fleet).headers)
    assert.are.same({}, render.error('no fleet').headers)
  end)

  describe('a', function()
    it("on the first project's header creates the worktree in the first project", function()
      local repo, api = two_projects()
      creates_in('▾ repo', repo, api)
    end)

    it('on a worktree of the first project creates the worktree in the first project', function()
      local repo, api = two_projects()
      creates_in('· one', repo, api)
    end)

    it("on the second project's header creates the worktree in the second project", function()
      local repo, api = two_projects()
      creates_in('▾ api', api, repo)
    end)

    it('on a worktree of the second project creates the worktree in the second project', function()
      local repo, api = two_projects()
      creates_in('· two', api, repo)
    end)

    it('under the header of a project with no worktrees creates its first one', function()
      support.load_client()
      local repo = support.repo()
      local empty = support.sibling_repo(repo, 'empty')
      new_worktree(repo, 'one')
      support.add_project(repo)
      support.add_project(empty)

      creates_in('no worktrees yet', empty, repo)
    end)

    it("on a gone project tells the core's reason and asks nothing", function()
      support.load_client()
      local repo = support.repo()
      local gone = support.sibling_repo(repo, 'gone')
      new_worktree(repo, 'one')
      support.add_project(repo)
      support.add_project(gone)
      vim.fn.delete(gone.root, 'rf')
      support.cd(repo.root)
      local ui = support.stub_ui({ input = 'fresh', select = { 'No agent' } })
      vim.cmd('OrcaTerm sidebar')

      press_on('a', '▾ gone')

      assert.are.same({}, ui.prompts)
      assert.are.equal(1, #ui.notes)
      assert.are.equal(vim.log.levels.ERROR, ui.notes[1].level)
      assert(vim.startswith(ui.notes[1].message, 'orca-term: '), ui.notes[1].message)
      assert(ui.notes[1].message:find('is gone', 1, true), ui.notes[1].message)
      assert.is_false(listed(repo, 'fresh'))
    end)
  end)

  describe('P', function()
    it('asks `Add project: `, completing directories, and adds it through the core', function()
      local repo, api = two_repositories()
      support.add_project(repo)
      support.cd(repo.root)
      local ui = support.stub_ui({ input = api.root })
      -- What the question completes, which the stub does not record.
      local stubbed = vim.ui.input
      local completion = nil
      ---@diagnostic disable-next-line: duplicate-set-field
      vim.ui.input = function(opts, on_confirm)
        completion = opts.completion
        stubbed(opts, on_confirm)
      end
      vim.cmd('OrcaTerm sidebar')
      assert.is_nil(line_with('▾ api'))

      press_on('P', '· one')

      assert.are.same({
        message = 'orca-term: added project api at ' .. api.root,
        level = vim.log.levels.INFO,
      }, next_note(ui))
      assert.are.same({ 'Add project: ' }, ui.prompts)
      assert.are.equal('dir', completion)
      assert.are.same({ repo.root, api.root }, project_paths(repo))
      -- The sidebar was read again: the project is under the first, with its worktree.
      local header = assert(line_with('▾ api  main'), 'the header of api')
      assert.is_true(line_with('▾ repo  main') < header)
      assert.is_true(header < line_with('· two'))
    end)

    it('takes a path as the completion writes it, with `~` and a trailing slash', function()
      local repo, api = two_repositories()
      support.add_project(repo)
      support.cd(repo.root)
      -- The fixture's HOME is `<tmp>/home`, beside the repositories.
      local ui = support.stub_ui({ input = '~/../api/' })
      vim.cmd('OrcaTerm sidebar')

      press_on('P', '▾ repo')

      assert.are.equal('orca-term: added project api at ' .. api.root, next_note(ui).message)
      assert.are.same({ repo.root, api.root }, project_paths(repo))
    end)

    it('adds the project of a path inside it, and says when it is already on the list', function()
      local repo, api = two_repositories()
      support.add_project(repo)
      support.cd(repo.root)
      local ui = support.stub_ui({ input = support.worktree_path(api, 'two') })
      vim.cmd('OrcaTerm sidebar')

      press_on('P', '▾ repo')
      assert.are.equal('orca-term: added project api at ' .. api.root, next_note(ui).message)

      press_on('P', '▾ repo')
      assert.are.same({
        message = ('orca-term: project api at %s is already on the list'):format(api.root),
        level = vim.log.levels.INFO,
      }, next_note(ui))
      assert.are.same({ repo.root, api.root }, project_paths(repo))
    end)

    it("shows the core's error for a path it refuses, and adds nothing", function()
      local repo = two_projects()
      support.cd(repo.root)
      -- The fixture's own directory holds the repositories and is inside none.
      local ui = support.stub_ui({ input = repo.tmp })
      vim.cmd('OrcaTerm sidebar')
      local before = lines()

      press_on('P', '▾ repo')

      local note = next_note(ui)
      assert.are.equal(vim.log.levels.ERROR, note.level)
      assert.are.equal(
        ('orca-term: %s is not inside a git repository, so it cannot be added as a project (nothing was added)'):format(
          repo.tmp
        ),
        note.message
      )

      -- A path that is no directory is refused too, in the core's words.
      ui = support.stub_ui({ input = repo.tmp .. '/nowhere' })
      press_on('P', '▾ repo')
      note = next_note(ui)
      assert.are.equal(vim.log.levels.ERROR, note.level)
      assert.are.equal(('orca-term: %s/nowhere is not a directory'):format(repo.tmp), note.message)

      assert.are.equal(2, #project_paths(repo))
      assert.are.same(before, lines())
    end)

    it('adds nothing when the question is cancelled or left empty', function()
      local repo = two_projects()
      support.cd(repo.root)
      local ui = support.stub_ui({})
      vim.cmd('OrcaTerm sidebar')

      press_on('P', '▾ repo')
      support.stub_ui({ input = '  ' })
      press_on('P', '▾ repo')

      vim.wait(200)
      assert.are.same({ 'Add project: ' }, ui.prompts)
      assert.are.same({}, ui.notes)
      assert.are.equal(2, #project_paths(repo))
    end)

    it(
      'is offered with an empty project list too, where the sidebar then shows the list',
      function()
        local repo, api = two_repositories()
        support.cd(repo.root)
        local ui = support.stub_ui({ input = api.root })
        vim.cmd('OrcaTerm sidebar')
        -- The empty list: the repository of Neovim's directory, as before the list existed.
        assert.are.equal(' ▾ repo  main', lines()[1])
        assert.is_nil(line_with('api'))

        press_on('P', '▾ repo')

        assert.are.equal('orca-term: added project api at ' .. api.root, next_note(ui).message)
        assert.are.same({ api.root }, project_paths(repo))
        assert.are.equal(' ▾ api  main', lines()[1])
        assert.truthy(line_with('· two'))
      end
    )
  end)

  describe('. on a project header', function()
    it('offers `Remove from list`, which asks first and touches nothing on disk', function()
      local repo, api = two_projects()
      support.cd(repo.root)
      local two = support.worktree_path(api, 'two')
      local ui = support.stub_ui({ select = { 'Remove from list', 'Remove' } })
      vim.cmd('OrcaTerm sidebar')

      press_on('.', '▾ api')

      assert.are.same({
        message = 'orca-term: removed project api from the list (nothing on disk was touched)',
        level = vim.log.levels.INFO,
      }, next_note(ui))
      assert.are.same({
        'Project api',
        'Remove project api from the list? Its directory, worktrees and branches are not touched.',
      }, ui.prompts)
      assert.are.same({ { 'Remove from list' }, { 'Remove', 'Cancel' } }, ui.shown)
      assert.are.same({ repo.root }, project_paths(repo))
      -- The sidebar was read again: the project and its worktree are gone from it.
      assert.is_nil(line_with('api'))
      assert.is_nil(line_with('two'))
      assert.truthy(line_with('│ · one'))
      -- Nothing on disk was touched: the directory, the worktree and its branch are there.
      assert.are.equal(1, vim.fn.isdirectory(api.root))
      assert.are.equal(1, vim.fn.isdirectory(two))
      assert.are.equal(two, support.worktree_path(api, 'two'))
      assert.are.equal(
        'two',
        vim.trim(support.git(api, { 'branch', '--list', 'two', '--format=%(refname:short)' }))
      )
    end)

    it('removes nothing when the question, or the menu, is cancelled', function()
      local repo, api = two_projects()
      support.cd(repo.root)
      local ui = support.stub_ui({ select = { 'Remove from list', 'Cancel' } })
      vim.cmd('OrcaTerm sidebar')
      local before = lines()

      press_on('.', '▾ api')
      -- No answer is left: the menu itself is cancelled.
      press_on('.', '▾ api')

      vim.wait(200)
      assert.are.equal(3, #ui.prompts)
      assert.are.same({}, ui.notes)
      assert.are.same({ repo.root, api.root }, project_paths(repo))
      assert.are.same(before, lines())
    end)

    it('takes a gone project off the list', function()
      support.load_client()
      local repo = support.repo()
      local gone = support.sibling_repo(repo, 'gone')
      new_worktree(repo, 'one')
      support.add_project(repo)
      support.add_project(gone)
      vim.fn.delete(gone.root, 'rf')
      support.cd(repo.root)
      local ui = support.stub_ui({ select = { 'Remove from list', 'Remove' } })
      vim.cmd('OrcaTerm sidebar')

      press_on('.', '▾ gone')

      assert(next_note(ui).message:find('removed project gone from the list', 1, true))
      assert.are.same({ repo.root }, project_paths(repo))
      assert.is_nil(line_with('gone'))
    end)

    it('offers nothing on a line that is neither a header nor a worktree', function()
      local repo = two_projects()
      support.cd(repo.root)
      local ui = support.stub_ui({ select = { 'Remove from list', 'Remove' } })
      vim.cmd('OrcaTerm sidebar')

      press_on('.', 'QUIET 1')

      vim.wait(200)
      assert.are.same({}, ui.prompts)
      assert.are.equal(2, #project_paths(repo))
    end)

    it('still offers the menu of a worktree on a worktree', function()
      local repo = two_projects()
      support.cd(repo.root)
      local ui = support.stub_ui({})
      vim.cmd('OrcaTerm sidebar')

      press_on('.', '· two')

      assert.are.same({ 'Worktree two' }, ui.prompts)
    end)
  end)
end)
