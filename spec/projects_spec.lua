-- Seam 2: the sidebar grouped by project (#154), in headless Neovim against the real binary. The
-- project list lives in the fixture's XDG_STATE_HOME (`support.isolate()`), never in the
-- developer's: `support.add_project` adds a repository to it through `orca-term project add`, and
-- the sidebar reads it through `orca-term fleets --json`. One header per project, an action run in
-- the second project's repository while Neovim sits in the first, a project with no worktrees, a
-- gone project, and the sidebar as before when the list is empty. `vim.ui` and `vim.notify` are
-- stubbed; the core never is.

local support = require('support')
local core = require('orca-term.core')
local detail = require('orca-term.detail')
local render = require('orca-term.render')
local sidebar = require('orca-term.sidebar')
local terminal = require('orca-term.terminal')

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

---The highlight groups on the sidebar's line `line`, in order.
---@param line integer
---@return string[]
local function groups_on(line)
  local buf = assert(sidebar.buffer())
  local range = { { line - 1, 0 }, { line - 1, -1 } }
  local marks =
    vim.api.nvim_buf_get_extmarks(buf, sidebar.NAMESPACE, range[1], range[2], { details = true })
  return vim.tbl_map(function(mark)
    return mark[4].hl_group
  end, marks)
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

---Makes a worktree in `repo` through the core.
---@param repo spec.Repo
---@param name string
local function new_worktree(repo, name)
  support.run({ support.binary, 'worktree', 'new', name }, repo.root)
end

---Two projects on the list, in this order: `repo`, with the worktree `one`, then `api`, with the
---worktrees `two` and `three`. Neovim sits in the first project's primary checkout.
---@return spec.Repo repo the first project
---@return spec.Repo api the second project
local function two_projects()
  support.load_client()
  local repo = support.repo()
  local api = support.sibling_repo(repo, 'api')
  new_worktree(repo, 'one')
  new_worktree(api, 'two')
  new_worktree(api, 'three')
  support.add_project(repo)
  support.add_project(api)
  support.cd(repo.root)
  return repo, api
end

describe('the sidebar, grouped by project', function()
  local columns = vim.o.columns

  before_each(function()
    vim.o.columns = 160
  end)

  after_each(function()
    support.reset_editor()
    support.cleanup()
    vim.o.columns = columns
  end)

  it("shows one header per project, in the list's order, above its own worktrees", function()
    two_projects()

    vim.cmd('OrcaTerm sidebar')

    assert.are.same({
      ' ▾ repo  main',
      '',
      ' QUIET 1',
      ' ╭────────────────────────────────╮',
      ' │ · one                          │',
      ' ╰────────────────────────────────╯',
      '',
      ' ▾ api  main',
      '',
      ' QUIET 2',
      ' · three',
      ' · two',
    }, lines())
    assert.are.same({ 'OrcaTermMuted', 'OrcaTermHeader', 'OrcaTermMuted' }, groups_on(8))
  end)

  it("follows the list's order, not the names, and not Neovim's directory", function()
    support.load_client()
    local repo = support.repo()
    local api = support.sibling_repo(repo, 'api')
    new_worktree(repo, 'one')
    new_worktree(api, 'two')
    support.add_project(repo)
    support.add_project(api)
    -- Neovim in the second project, and then outside every repository: the sidebar is the same.
    support.cd(api.root)
    vim.cmd('OrcaTerm sidebar')
    local shown = lines()
    assert.are.equal(' ▾ repo  main', shown[1])
    assert.is_true(line_with('▾ repo') < line_with('▾ api'))

    support.cd(repo.tmp)
    sidebar.refresh()
    assert.are.same(shown, lines())
  end)

  it('moves the card with j and k from one project to the next', function()
    two_projects()
    vim.cmd('OrcaTerm sidebar')
    vim.api.nvim_set_current_win(sidebar_window())
    assert.truthy(line_with('│ · one'))

    vim.cmd('normal j')
    assert.truthy(line_with('│ · three'))
    assert.is_nil(line_with('│ · one'))
    assert.are.equal(line_with('│ · three'), vim.api.nvim_win_get_cursor(0)[1])

    vim.cmd('normal jj')
    assert(line_with('│ · two'), 'j stops at the last worktree of the last project')

    vim.cmd('normal kk')
    assert.truthy(line_with('│ · one'))
  end)

  it('reads every project again when Neovim gains focus', function()
    local _, api = two_projects()
    vim.cmd('OrcaTerm sidebar')
    assert.is_nil(line_with('later'))

    new_worktree(api, 'later')
    vim.cmd('doautocmd FocusGained')

    assert.is_true(line_with('▾ api') < line_with('· later'))
  end)

  it("marks a worktree of the second project unseen in that project's repository", function()
    local _, api = two_projects()
    local ui = support.stub_ui({})
    vim.cmd('OrcaTerm sidebar')
    assert.is_false(support.listed(api, 'two').unseen)

    -- Neovim sits in the first project, where the core knows no worktree `two`.
    press_on('u', '· two')

    support.wait_for(function()
      return line_with('•· two') ~= nil
    end, 'the unseen mark on two')
    assert.is_true(support.listed(api, 'two').unseen)
    assert.are.same({}, ui.notes)
  end)

  it("shows the detail and the sessions of the second project's worktree", function()
    two_projects()
    local ui = support.stub_ui({})
    vim.cmd('OrcaTerm sidebar')

    press_on('K', '· two')
    support.wait_for(function()
      return detail.window() ~= nil
    end, 'the detail float of two')
    local float = assert(detail.window())
    local shown = vim.api.nvim_buf_get_lines(vim.api.nvim_win_get_buf(float), 0, -1, false)
    assert.are.equal(' · no agent', shown[1])
    press_on('K', '· two')

    press_on('s', '· two')
    support.wait_for(function()
      return told(ui, 'no sessions in worktree two')
    end, 'the sessions of two')
    for _, note in ipairs(ui.notes) do
      assert.are.equal(vim.log.levels.INFO, note.level, note.message)
    end
  end)

  it("refreshes the pull request of the second project's worktree with R", function()
    local repo, api = two_projects()
    local ui = support.stub_ui({})
    vim.cmd('OrcaTerm sidebar')
    -- Opening refreshes the worktrees of both projects (#156), before `two` has a pull request:
    -- the core has kept that it has none once this ends.
    support.wait_for(function()
      local found = support.listed(api, 'two').pr
      return #support.gh_calls(repo) == 3 and type(found) == 'table' and found.none == true
    end, 'the refresh on opening')
    support.gh_answer(repo, 'two', {
      number = 12,
      title = 'Two',
      state = 'OPEN',
      isDraft = false,
      url = 'https://github.com/o/r/pull/12',
      statusCheckRollup = {},
    })

    press_on('R', '· two')

    support.wait_for(function()
      local row = line_with('· two')
      return row ~= nil and (lines()[row + 1] or ''):find('PR', 1, true) ~= nil
    end, 'the pull request of two in its row')
    assert.is_true(vim.startswith(support.gh_calls(repo)[4], 'pr view two '))
    assert.are.same({}, ui.notes)
  end)

  it("removes a worktree of the second project from that project's repository", function()
    local _, api = two_projects()
    local ui = support.stub_ui({ select = { 'Remove worktree', 'Remove' } })
    vim.cmd('OrcaTerm sidebar')
    local path = support.worktree_path(api, 'two')

    press_on('.', '· two')

    support.wait_for(function()
      return told(ui, 'removed worktree two')
    end, 'the removal of two')
    assert.are.equal('Worktree two', ui.prompts[1])
    assert.is_nil(vim.uv.fs_stat(path))
    assert.is_nil(line_with('two'))
    assert.truthy(line_with('· three'))
  end)

  it("asks for the review of the second project's worktree in that project's repository", function()
    local _, api = two_projects()
    -- The review itself needs diffview.nvim and is spec/review_spec.lua's: here, only where `d`
    -- says the core runs.
    local actions = require('orca-term.actions')
    local real = actions.review
    local asked = nil
    ---@diagnostic disable-next-line: duplicate-set-field
    actions.review = function(path, primary)
      asked = { path = path, primary = primary }
    end
    finally(function()
      actions.review = real
    end)
    vim.cmd('OrcaTerm sidebar')

    press_on('d', '· two')

    assert.are.same({ path = support.worktree_path(api, 'two'), primary = api.root }, asked)
  end)

  it("offers the second project's own menu, and pushes, composes and merges there", function()
    local repo, api = two_projects()
    -- Only the second project has `origin`, where Neovim does not sit.
    local origin = repo.tmp .. '/origin.git'
    support.run({ 'git', 'init', '--quiet', '--bare', origin })
    support.git(api, { 'remote', 'add', 'origin', origin })
    support.git(api, { 'push', '--quiet', 'origin', 'main' })
    local path = support.worktree_path(api, 'two')
    support.run({ 'git', 'commit', '--quiet', '--allow-empty', '-m', 'Add two' }, path)
    -- The menu is cancelled (it offers no such entry); the merge is then answered.
    local ui = support.stub_ui({ select = { 'no such entry', 'Squash and merge' } })
    vim.cmd('OrcaTerm sidebar')
    -- The card goes to `two`: the cursor then stays on it each time the sidebar is drawn again.
    vim.api.nvim_set_current_win(sidebar_window())
    vim.cmd('normal jj')
    assert.truthy(line_with('│ · two'))

    -- The menu of a worktree offers what its own project's fleet allows: a rebase and a push.
    press_on('.', '· two')
    assert.is_true(vim.tbl_contains(ui.shown[1], 'Rebase onto origin/main'))
    assert.is_true(vim.tbl_contains(ui.shown[1], 'Push'))

    -- The commands act on the worktree under the cursor, in its own project.
    vim.cmd('OrcaTerm push')
    support.wait_for(function()
      return told(ui, 'two: pushed to origin/two')
    end, 'the push of two')

    vim.cmd('OrcaTerm pr-create')
    local composer = vim.tbl_filter(function(buf)
      return vim.api.nvim_buf_get_name(buf) == 'orca-term://pr/two'
    end, vim.api.nvim_list_bufs())[1]
    assert(composer, 'no composer for two')
    assert.are.equal('Add two', vim.api.nvim_buf_get_lines(composer, 0, 1, false)[1])
    vim.api.nvim_buf_delete(composer, { force = true })

    support.gh_answer(repo, 'two', {
      number = 12,
      title = 'Add two',
      state = 'OPEN',
      isDraft = false,
      url = 'https://github.com/o/r/pull/12',
      statusCheckRollup = { { status = 'COMPLETED', conclusion = 'SUCCESS' } },
      mergeable = 'MERGEABLE',
      reviewDecision = '',
    })
    press_on('R', '· two')
    support.wait_for(function()
      local row = line_with('· two')
      return row ~= nil and (lines()[row + 1] or ''):find('PR', 1, true) ~= nil
    end, 'the pull request of two in its row')
    vim.cmd('OrcaTerm pr-merge')
    support.wait_for(function()
      return told(ui, 'merged PR #12')
    end, 'the merge of the pull request of two')

    for _, note in ipairs(ui.notes) do
      assert.are.equal(vim.log.levels.INFO, note.level, note.message)
    end
  end)

  it(
    "opens a worktree of the second project, starting its agent from that project's repository",
    function()
      support.load_client()
      local repo = support.agent_repo()
      local api = support.sibling_repo(repo, 'api')
      new_worktree(repo, 'one')
      new_worktree(api, 'two')
      support.run({ support.binary, 'agent', 'start', 'two' }, api.root)
      support.run({ support.binary, 'agent', 'stop', 'two' }, api.root)
      support.add_project(repo)
      support.add_project(api)
      support.cd(repo.root)
      local ui = support.stub_ui({})
      vim.cmd('OrcaTerm sidebar')
      local path = support.worktree_path(api, 'two')

      press_on('<CR>', ' two')

      -- The holder was stopped: `<CR>` starts the agent again, which only the core run in the second
      -- project can do, then attaches to it.
      support.wait_for(function()
        local buf = terminal.find(path)
        return buf ~= nil and terminal.running_job(buf) ~= nil
      end, 'the agent terminal of two')
      assert.are.equal(path, vim.fn.getcwd(-1, vim.fn.tabpagenr()))
      assert.is_true(support.listed(api, 'two').holder)
      assert.are.same({}, ui.notes)
      -- The selection follows the tab, into the second project: its card is around `two`.
      support.wait_for(function()
        local row = line_with(' two')
        return row ~= nil and vim.startswith(lines()[row], ' │')
      end, 'the card around two')
    end
  )

  it('shows a project with no worktrees as its header and that it has none yet', function()
    support.load_client()
    local repo = support.repo()
    local empty = support.sibling_repo(repo, 'empty')
    new_worktree(repo, 'one')
    support.add_project(empty)
    support.add_project(repo)
    support.cd(repo.root)

    vim.cmd('OrcaTerm sidebar')

    assert.are.same({
      ' ▾ empty  main',
      '   no worktrees yet',
      '',
      ' ▾ repo  main',
      '',
      ' QUIET 1',
      ' ╭────────────────────────────────╮',
      ' │ · one                          │',
      ' ╰────────────────────────────────╯',
    }, lines())
    assert.are.same({ 'OrcaTermMuted' }, groups_on(2))
  end)

  it(
    'shows a gone project as its header and the error, muted, and still shows the others',
    function()
      support.load_client()
      local repo = support.repo()
      local gone = support.sibling_repo(repo, 'gone')
      new_worktree(repo, 'one')
      support.add_project(gone)
      support.add_project(repo)
      vim.fn.delete(gone.root, 'rf')
      support.cd(repo.root)

      vim.cmd('OrcaTerm sidebar')

      local shown = lines()
      assert.are.equal(' ▾ gone', shown[1])
      local next_header = assert(line_with('▾ repo  main'))
      local words = {}
      for index = 2, next_header - 2 do
        assert.are.same({ 'OrcaTermMuted' }, groups_on(index))
        assert(vim.fn.strdisplaywidth(shown[index]) <= 36, shown[index])
        table.insert(words, vim.trim(shown[index]))
      end
      -- The core's words, whatever the wrapping did to their spaces.
      local said = table.concat(words):gsub('%s', '')
      assert(said:find('isgone', 1, true), said)
      assert(said:find('orca-termprojectrm', 1, true), said)
      assert.are.equal('', shown[next_header - 1])
      assert.truthy(line_with('│ · one'))
    end
  )

  it('shows why the project list cannot be read', function()
    support.load_client()
    local repo = support.repo()
    new_worktree(repo, 'one')
    local dir = vim.env.XDG_STATE_HOME .. '/orca-term'
    vim.fn.mkdir(dir, 'p')
    vim.fn.writefile({ 'not json' }, dir .. '/projects.json')
    support.cd(repo.root)

    vim.cmd('OrcaTerm sidebar')

    local shown = lines()
    assert.are.equal(' ▾ orca-term', shown[1])
    local text = table.concat(vim.tbl_map(vim.trim, vim.list_slice(shown, 3)), ' ')
    assert(text:find('projects.json', 1, true), text)
    assert.same({ 'OrcaTermError' }, groups_on(3))
  end)

  it("with an empty project list, shows the repository of Neovim's directory as before", function()
    support.load_client()
    local repo = support.repo()
    local api = support.sibling_repo(repo, 'api')
    new_worktree(repo, 'one')
    new_worktree(api, 'two')
    support.cd(repo.root)

    vim.cmd('OrcaTerm sidebar')

    local fleet = assert(core.worktree_ls(repo.root))
    local alone = render.fleet(fleet).lines
    assert.are.equal(' ▾ repo  main', alone[1])
    assert.are.same(alone, lines())
    assert.is_nil(line_with('api'))

    -- A list emptied again is an empty list: a project added, then taken off.
    support.add_project(api)
    sidebar.refresh()
    assert.are.equal(' ▾ api  main', lines()[1])
    support.run({ support.binary, 'project', 'rm', api.root }, repo.root)
    sidebar.refresh()
    assert.are.same(alone, lines())
  end)
end)
