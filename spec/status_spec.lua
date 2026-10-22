-- Seam 2: the live status in the sidebar, in headless Neovim against the real binary and the fake
-- claude of `support.agent_repo()`. The core starts the agent; the spec then acts as claude does,
-- playing a recording of claude's hooks through the handlers the agent's settings register
-- (`support.play_hooks`, as tests/hooks.rs does), reads the fleet again with `sidebar.refresh()` and
-- checks the glyph, the activity and their colours. While a worktree is working its glyph turns,
-- without the fleet being read again, and the timer stops once nothing is working or the sidebar
-- closes.

local support = require('support')
local core = require('orca-term.core')
local render = require('orca-term.render')
local sidebar = require('orca-term.sidebar')

---The sidebar buffer's lines.
---@return string[]
local function lines()
  return vim.api.nvim_buf_get_lines(assert(sidebar.buffer()), 0, -1, false)
end

---Line `index` of the sidebar without the card's borders, the spaces around and the unseen mark
---(spec/unseen_spec.lua covers the mark).
---@param index integer
---@return string
local function bare(index)
  local text = vim.trim((lines()[index]:gsub('│', '')))
  if vim.startswith(text, render.UNSEEN) then
    text = text:sub(#render.UNSEEN + 1)
  end
  return text
end

---The index of the line holding the name of the worktree `name`: its glyph, a space, its name,
---then what sits at its right (the agent and the age).
---@param name string
---@return integer
local function name_line(name)
  for index = 1, #lines() do
    if vim.fn.strcharpart(bare(index), 2):match('^%S+') == name then
      return index
    end
  end
  error('no line for ' .. name)
end

---The glyph shown before the worktree named `name`.
---@param name string
---@return string
local function glyph_of(name)
  return vim.fn.strcharpart(bare(name_line(name)), 0, 1)
end

---The text of the worktree's second line, without the card's borders and the indent.
---@param name string
---@return string
local function second_line(name)
  return bare(name_line(name) + 1)
end

---The highlight groups on line `line` (1-based), in order.
---@param line integer
---@return string[]
local function groups_on(line)
  local groups = {}
  local marks = vim.api.nvim_buf_get_extmarks(
    assert(sidebar.buffer()),
    sidebar.NAMESPACE,
    { line - 1, 0 },
    { line - 1, -1 },
    { details = true }
  )
  for _, mark in ipairs(marks) do
    table.insert(groups, mark[4].hl_group)
  end
  return groups
end

---A repository whose agents are the fake claude, with the worktree `fix` whose agent the core
---started, and the sidebar open in its primary checkout.
---@return spec.AgentRepo repo
---@return string worktree the path of `fix`
local function agent_on_fix()
  support.load_client()
  local repo = support.agent_repo()
  support.run({ support.binary, 'worktree', 'new', 'fix' }, repo.root)
  support.run({ support.binary, 'agent', 'start', 'fix' }, repo.root)
  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  return repo, support.worktree_path(repo, 'fix')
end

describe('the live status in the sidebar', function()
  after_each(function()
    support.reset_editor()
    support.cleanup()
  end)

  it('shows a turn in progress as working, with the command the agent runs', function()
    local repo, fix = agent_on_fix()
    assert.are.equal(render.GLYPHS.idle, glyph_of('fix'))

    support.play_hooks(repo, fix, 'tui-queuetool-2.1.300.log', 1, 3)
    sidebar.refresh()

    assert.truthy(vim.tbl_contains(render.SPINNER, glyph_of('fix')))
    assert.are.equal('Bash: sleep 6', second_line('fix'))
    assert.truthy(vim.tbl_contains(groups_on(name_line('fix')), 'OrcaTermStateWorking'))
  end)

  it('shows a question as waiting, with the question, under Needs you', function()
    local repo, fix = agent_on_fix()

    support.play_hooks(repo, fix, 'tui-ask-2.1.300.log', 1, 4)
    sidebar.refresh()

    assert.are.equal('?', glyph_of('fix'))
    assert.are.equal('Which colour?', second_line('fix'))
    assert.truthy(lines()[name_line('fix') - 2]:find('NEEDS YOU 1', 1, true))
    assert.truthy(vim.tbl_contains(groups_on(name_line('fix') + 1), 'OrcaTermStateWaiting'))
    assert.is_false(sidebar.spinning())
  end)

  it("shows a finished turn with the agent's last words", function()
    local repo, fix = agent_on_fix()

    support.play_hooks(repo, fix, 'p-hello-2.1.300.log')
    sidebar.refresh()

    assert.are.equal('●', glyph_of('fix'))
    assert.are.equal('ok', second_line('fix'))
    assert.truthy(vim.tbl_contains(groups_on(name_line('fix')), 'OrcaTermStateFinished'))
  end)

  it('shows a turn ended on an API error as failed, with the error', function()
    local repo, fix = agent_on_fix()

    support.play_hooks(repo, fix, 'p-stopfailure-2.1.300.log')
    sidebar.refresh()

    assert.are.equal('●', glyph_of('fix'))
    assert.are.equal('API Error: 400 fake bad req…', second_line('fix'))
    assert.truthy(vim.tbl_contains(groups_on(name_line('fix')), 'OrcaTermStateFailed'))
  end)

  it(
    'turns the working glyph without reading the fleet, until nothing works or it closes',
    function()
      local repo, fix = agent_on_fix()
      support.play_hooks(repo, fix, 'tui-queuetool-2.1.300.log', 1, 3)
      sidebar.refresh()
      assert.is_true(sidebar.spinning())

      -- The person's window stays current, and the sidebar's cursor stays where they put it.
      local current = vim.api.nvim_get_current_win()
      local buf = assert(sidebar.buffer())
      local sidebar_win = assert(vim.fn.win_findbuf(buf)[1])
      vim.api.nvim_win_set_cursor(sidebar_win, { 1, 0 })

      local real_ls = core.worktree_ls
      local reads = 0
      ---@diagnostic disable-next-line: duplicate-set-field
      core.worktree_ls = function(...)
        reads = reads + 1
        return real_ls(...)
      end
      local seen = {}
      local ok = pcall(function()
        support.wait_for(function()
          seen[glyph_of('fix')] = true
          return vim.tbl_count(seen) == #render.SPINNER
        end, 'every frame of the spinner')
      end)
      core.worktree_ls = real_ls
      assert(ok, 'the glyph showed only ' .. table.concat(vim.tbl_keys(seen), ' '))
      assert.are.equal(0, reads, 'the spinner read the fleet again')
      assert.are.equal(current, vim.api.nvim_get_current_win())
      assert.are.same({ 1, 0 }, vim.api.nvim_win_get_cursor(sidebar_win))
      assert.are.equal('Bash: sleep 6', second_line('fix'))

      -- The turn finishes: nothing works, the timer stops.
      support.play_hooks(repo, fix, 'tui-queuetool-2.1.300.log', 4)
      sidebar.refresh()
      assert.is_false(sidebar.spinning())
      assert.are.equal('●', glyph_of('fix'))

      -- A new turn spins again, until the sidebar closes.
      support.play_hooks(repo, fix, 'p-hello-2.1.300.log', 1, 2)
      sidebar.refresh()
      assert.is_true(sidebar.spinning())
      sidebar.close()
      assert.is_false(sidebar.spinning())
    end
  )
end)
