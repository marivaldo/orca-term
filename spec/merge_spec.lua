-- Merging a worktree's pull request (#136). Seam 2: the sidebar's menu and `:OrcaTerm pr-merge`
-- ask how to merge, then run the real binary's `pr merge`, which asks and merges through the fake
-- `gh` every fixture puts first on PATH (`support.lua`), never the real GitHub CLI. `vim.ui` and
-- `vim.notify` are stubbed; the core never is.

local support = require('support')
local actions = require('orca-term.actions')
local pr = require('orca-term.pr')
local sidebar = require('orca-term.sidebar')

---The menu of a worktree with no pull request open.
local MENU = actions.MENU

---The same menu with `Merge pull request` before `Remove worktree`.
local MENU_WITH_MERGE = {
  'Re-run setup',
  'Copy path',
  'Swap agent',
  'New session',
  'Create pull request',
  'Create draft pull request',
  'Merge pull request',
  'Remove worktree',
}

---What `gh pr view --json ...` prints for pull request 12 in `state`, a draft or not, with one
---passing check, whether GitHub can merge it and what its review decided.
---@param state string
---@param fields? table
---@return table
local function view(state, fields)
  return vim.tbl_extend('force', {
    number = 12,
    title = 'Fix the login',
    state = state,
    isDraft = false,
    url = 'https://github.com/o/r/pull/12',
    statusCheckRollup = { { status = 'COMPLETED', conclusion = 'SUCCESS' } },
    mergeable = 'MERGEABLE',
    reviewDecision = '',
  }, fields or {})
end

---The sidebar's line number of the worktree `name`, and the group `PR` is drawn in at the end of
---its second line (`nil` when there is no `PR`).
---@param name string
---@return integer? row 1-based
---@return string? group
local function row_of(name)
  local buf = assert(sidebar.buffer())
  local lines = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  for index, line in ipairs(lines) do
    if line:find(' ' .. name .. ' ', 1, true) or vim.endswith(line, ' ' .. name) then
      local second = lines[index + 1] or ''
      local marks = vim.api.nvim_buf_get_extmarks(
        buf,
        sidebar.NAMESPACE,
        { index, 0 },
        { index, -1 },
        { details = true }
      )
      for _, mark in ipairs(marks) do
        local details = mark[4] or {}
        if second:sub(mark[3] + 1, details.end_col) == 'PR' then
          return index, details.hl_group
        end
      end
      return index, nil
    end
  end
  return nil
end

---Waits until the sidebar draws `PR` on the row of `name` in `group`.
---@param name string
---@param group string
local function wait_for_pr(name, group)
  support.wait_for(function()
    local _, drawn = row_of(name)
    return drawn == group
  end, 'PR in ' .. group .. ' on ' .. name)
end

---Goes to the sidebar of the current tab, its cursor on the row of `name`.
---@param name string
local function cursor_on(name)
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_buf(win) == sidebar.buffer() then
      vim.api.nvim_set_current_win(win)
      vim.api.nvim_win_set_cursor(win, { assert(row_of(name), name), 0 })
      return
    end
  end
  error('no sidebar in the current tab')
end

---Presses `key` in the sidebar of the current tab, on the row of `name`.
---@param key string
---@param name string
local function press_on(key, name)
  cursor_on(name)
  vim.cmd('normal ' .. vim.keycode(key))
end

---Makes the fake `gh` of `repo` answer `found` for `demo`, then refreshes it with `R` and waits
---until the sidebar draws its `PR` in `group`.
---@param repo spec.Repo
---@param found table
---@param group string
local function refresh_to(repo, found, group)
  support.gh_answer(repo, 'demo', found)
  press_on('R', 'demo')
  wait_for_pr('demo', group)
end

---Waits until the notification after the first `seen` arrives, and returns it.
---@param ui spec.Ui
---@param seen integer
---@return { message: string, level: integer }
local function note_after(ui, seen)
  support.wait_for(function()
    return #ui.notes > seen
  end, 'a notification')
  return ui.notes[seen + 1]
end

---The runs of the fake `gh` of `repo` that merged, one string each.
---@param repo spec.Repo
---@return string[]
local function merges(repo)
  return vim.tbl_filter(function(call)
    return vim.startswith(call, 'pr merge')
  end, support.gh_calls(repo))
end

---A repository with the worktree `demo` (its worktrees kept under the fixture's own directory),
---the fake `gh` answering `found` for it (none when `nil`), Neovim in the primary checkout and the
---sidebar open, showing what the core found.
---@param found? table
---@return spec.Repo repo
---@return string path the worktree `demo`
local function merge_repo(found)
  support.load_client()
  local repo = support.repo()
  vim.fn.writefile({ 'base: ' .. repo.tmp .. '/worktrees' }, repo.root .. '/.git/orca-term.yaml')
  support.run({ support.binary, 'worktree', 'new', 'demo' }, repo.root)
  if found then
    support.gh_answer(repo, 'demo', found)
  end
  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  support.wait_for(function()
    return #support.gh_calls(repo) == 1 and support.listed(repo, 'demo').pr ~= nil
  end, 'the first refresh')
  if found then
    wait_for_pr('demo', 'OrcaTermPrPassing')
  end
  return repo, support.worktree_path(repo, 'demo')
end

describe('merging a pull request', function()
  after_each(function()
    pr._reset()
    support.reset_editor()
    support.cleanup()
  end)

  it('is offered in the menu only while the pull request is open or a draft', function()
    local repo = merge_repo(nil)
    local ui = support.stub_ui({})

    press_on('.', 'demo')
    assert.are.same(MENU, ui.shown[1])

    -- `:OrcaTerm pr-merge` on a worktree with no open pull request only says so.
    cursor_on('demo')
    vim.cmd('OrcaTerm pr-merge')
    assert.are.same({
      message = 'orca-term: worktree demo has no open pull request to merge; R in the sidebar '
        .. 'reads it again from GitHub',
      level = vim.log.levels.ERROR,
    }, ui.notes[#ui.notes])

    refresh_to(repo, view('OPEN'), 'OrcaTermPrPassing')
    press_on('.', 'demo')
    assert.are.same(MENU_WITH_MERGE, ui.shown[2])

    refresh_to(repo, view('OPEN', { isDraft = true }), 'OrcaTermPrDraft')
    press_on('.', 'demo')
    assert.are.same(MENU_WITH_MERGE, ui.shown[3])

    refresh_to(repo, view('MERGED'), 'OrcaTermPrMerged')
    press_on('.', 'demo')
    assert.are.same(MENU, ui.shown[4])
    assert.are.same({}, merges(repo))
  end)

  it('asks how, squash first, merges through the core and shows the row merged', function()
    local repo, path = merge_repo(view('OPEN', { reviewDecision = 'APPROVED' }))
    local ui = support.stub_ui({ select = { 'Merge pull request', 'Squash and merge' } })
    local seen = #ui.notes

    press_on('.', 'demo')

    assert.are.equal('Merge PR #12 of demo?', ui.prompts[2])
    assert.are.same(
      { 'Squash and merge', 'Create a merge commit', 'Rebase and merge', 'Cancel' },
      ui.shown[2]
    )
    assert.are.same({
      message = 'orca-term: merging PR #12 of demo',
      level = vim.log.levels.INFO,
    }, note_after(ui, seen))
    assert.are.same({
      message = 'orca-term: merged PR #12: https://github.com/o/r/pull/12',
      level = vim.log.levels.INFO,
    }, note_after(ui, seen + 1))
    assert.are.same({ 'pr merge 12 --squash' }, merges(repo))
    for _, call in ipairs(support.gh_calls(repo)) do
      assert(not call:find('delete-branch', 1, true), call)
    end
    assert.are.equal('merged', support.listed(repo, 'demo').pr.state)
    wait_for_pr('demo', 'OrcaTermPrMerged')
    assert.are.equal(1, vim.fn.isdirectory(path), 'the worktree stays')
  end)

  it('passes the method chosen, and Cancel merges nothing', function()
    local repo = merge_repo(view('OPEN'))
    support.stub_ui({ select = { 'Merge pull request', 'Cancel' } })
    press_on('.', 'demo')
    vim.wait(300)
    assert.are.same({}, merges(repo))

    support.stub_ui({ select = { 'Merge pull request', 'Rebase and merge' } })
    press_on('.', 'demo')
    wait_for_pr('demo', 'OrcaTermPrMerged')
    assert.are.same({ 'pr merge 12 --rebase' }, merges(repo))

    refresh_to(repo, view('OPEN'), 'OrcaTermPrPassing')
    support.stub_ui({ select = { 'Merge pull request', 'Create a merge commit' } })
    press_on('.', 'demo')
    wait_for_pr('demo', 'OrcaTermPrMerged')
    assert.are.same({ 'pr merge 12 --rebase', 'pr merge 12 --merge' }, merges(repo))
  end)

  it('warns, and keeps the row open, when GitHub still shows it open after gh merged', function()
    local repo = merge_repo(view('OPEN', { reviewDecision = 'APPROVED' }))
    vim.fn.writefile({}, repo.gh .. '/merge-held')
    local ui = support.stub_ui({ select = { 'Merge pull request', 'Squash and merge' } })
    local seen = #ui.notes

    press_on('.', 'demo')

    assert.are.equal('orca-term: merging PR #12 of demo', note_after(ui, seen).message)
    assert.are.same({
      message = 'orca-term: gh accepted the merge of PR #12 (squash), but GitHub still shows it '
        .. 'open: a merge queue or a required check may be holding it; run orca-term pr refresh '
        .. 'demo later',
      level = vim.log.levels.WARN,
    }, note_after(ui, seen + 1))
    assert.are.same({ 'pr merge 12 --squash' }, merges(repo))
    assert.are.equal('open', support.listed(repo, 'demo').pr.state)
    vim.wait(300)
    local _, group = row_of('demo')
    assert.are.equal('OrcaTermPrPassing', group)
  end)

  it("shows the core's reason on a refusal, and gh's words for anything else", function()
    local repo = merge_repo(view('OPEN', { reviewDecision = 'REVIEW_REQUIRED' }))
    local ui = support.stub_ui({ select = { 'Squash and merge' } })
    local seen = #ui.notes
    -- `:OrcaTerm pr-merge` in the sidebar merges the worktree under the cursor.
    cursor_on('demo')

    vim.cmd('OrcaTerm pr-merge')

    assert.are.equal('Merge PR #12 of demo?', ui.prompts[1])
    assert.are.same({
      message = 'orca-term: PR #12 of worktree demo needs an approving review first (nothing '
        .. 'was merged)',
      level = vim.log.levels.ERROR,
    }, note_after(ui, seen + 1))
    assert.are.same({}, merges(repo))

    support.gh_answer(repo, 'demo', view('OPEN', { reviewDecision = 'APPROVED' }))
    vim.fn.writefile(
      { 'X Pull request o/r#12 is not mergeable: the base branch policy prohibits the merge.' },
      repo.gh .. '/merge.err'
    )
    ui = support.stub_ui({ select = { 'Squash and merge' } })
    cursor_on('demo')
    vim.cmd('OrcaTerm pr-merge')

    local failed = note_after(ui, 1)
    assert.are.equal(vim.log.levels.ERROR, failed.level)
    assert.are.equal(
      'orca-term: gh pr merge failed for PR #12 of worktree demo: X Pull request o/r#12 is not '
        .. 'mergeable: the base branch policy prohibits the merge.',
      failed.message
    )
    assert.are.same({ 'pr merge 12 --squash' }, merges(repo))
    local _, group = row_of('demo')
    assert.are.equal('OrcaTermPrPassing', group)
  end)

  it(":OrcaTerm pr-merge merges the current tab's worktree", function()
    local repo, path = merge_repo(view('OPEN'))
    local ui = support.stub_ui({})
    press_on('<CR>', 'demo')
    assert.are.equal(path, vim.fn.getcwd(-1, vim.fn.tabpagenr()))
    ui = support.stub_ui({ select = { 'Squash and merge' } })

    vim.cmd('OrcaTerm pr-merge')

    assert.are.equal('Merge PR #12 of demo?', ui.prompts[1])
    support.wait_for(function()
      return #merges(repo) == 1 and support.listed(repo, 'demo').pr.state == 'merged'
    end, 'the merge')
    assert.are.equal(
      'orca-term: merged PR #12: https://github.com/o/r/pull/12',
      note_after(ui, 1).message
    )
  end)
end)
