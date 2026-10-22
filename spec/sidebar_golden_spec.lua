-- Seam 2, golden: the sidebar as Neovim shows it, for a real repository holding a worktree in
-- every state the core reports today, read by the real binary (`support.every_state_repo()`: the
-- agents are the fake claude, their states come from recordings of claude's hooks, and unseen and
-- seen worktrees show side by side). The buffer's text and its highlights are compared with
-- spec/golden/sidebar.txt.
--
-- The golden file holds the buffer's lines, then one highlight per line as
-- `<line>:<first>-<last> <group>` (1-based line, 0-based byte columns, the last one excluded).
-- After a deliberate change to the sidebar's look, rewrite it and review the diff:
--
--     UPDATE_GOLDEN=1 just lua-test
--
-- The repository's directory is named `repo` and no path is shown, so the file is the same on
-- every machine. The ages and the order by recency never depend on the clock either: the spec
-- fixes the time the sidebar measures ages from (`render.clock`) and each worktree's
-- `last_activity` (`LAST_ACTIVITY`), in the fleet the core returns. Each worktree's
-- `changed_files` is the real one: the fixture writes its untracked files, so it never varies.

local support = require('support')
local core = require('orca-term.core')
local render = require('orca-term.render')
local sidebar = require('orca-term.sidebar')

local GOLDEN = support.root .. '/spec/golden/sidebar.txt'

---The time the golden measures ages from, in milliseconds since the Unix epoch.
local NOW = 1700000000000

local MINUTE = 60 * 1000
local HOUR = 60 * MINUTE
local DAY = 24 * HOUR

---Each worktree's fixed last activity, by name; a worktree not named has none. They show every
---age format (`now`, minutes, hours, days), and order *Needs you* against the alphabet: `slow-job`
---before `api-call`, `nudged` before `greet`. `standby` has an agent and no last activity, so it
---shows the label alone.
local LAST_ACTIVITY = {
  ['pick-colour'] = NOW - 20 * 1000,
  ['slow-job'] = NOW - 9 * MINUTE,
  ['api-call'] = NOW - 3 * HOUR,
  nudged = NOW - 5 * MINUTE,
  greet = NOW - 2 * DAY,
  ['run-tests'] = NOW,
  reviewed = NOW - DAY,
}

---`fleet` with each worktree's last activity replaced by its fixed one.
---@param fleet? orca_term.Fleet
---@return orca_term.Fleet?
local function fixed(fleet)
  for _, worktree in ipairs(fleet and fleet.worktrees or {}) do
    worktree.last_activity = LAST_ACTIVITY[worktree.name]
  end
  return fleet
end

---Fixes the clock and the last activity of every fleet the core returns, and returns what puts
---them back.
---@return fun()
local function fix_time()
  local real_clock, real_ls, real_ls_async = render.clock, core.worktree_ls, core.worktree_ls_async
  ---@diagnostic disable-next-line: duplicate-set-field
  render.clock = function()
    return NOW
  end
  ---@diagnostic disable-next-line: duplicate-set-field
  core.worktree_ls = function(...)
    local fleet, err = real_ls(...)
    return fixed(fleet), err
  end
  ---@diagnostic disable-next-line: duplicate-set-field
  core.worktree_ls_async = function(cwd, on_done)
    real_ls_async(cwd, function(fleet, err)
      on_done(fixed(fleet), err)
    end)
  end
  return function()
    render.clock, core.worktree_ls, core.worktree_ls_async = real_clock, real_ls, real_ls_async
  end
end

---The sidebar buffer's text and highlights, in the golden file's format.
---@return string[]
local function snapshot()
  local buf = assert(sidebar.buffer())
  local out = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  table.insert(out, '')
  table.insert(out, '# highlights: <line>:<first>-<last> <group>')
  local marks = vim.api.nvim_buf_get_extmarks(buf, sidebar.NAMESPACE, 0, -1, { details = true })
  for _, mark in ipairs(marks) do
    local row, col, details = mark[2], mark[3], mark[4] or {}
    table.insert(out, ('%d:%d-%d %s'):format(row + 1, col, details.end_col, details.hl_group))
  end
  return out
end

describe('the sidebar, golden', function()
  after_each(function()
    support.reset_editor()
    support.cleanup()
  end)

  it('renders every state the core reports today, unseen and seen', function()
    support.load_client()
    local repo, release = support.every_state_repo()
    finally(release)

    finally(fix_time())

    support.cd(repo.root)
    vim.cmd('OrcaTerm sidebar')
    local actual = snapshot()

    if os.getenv('UPDATE_GOLDEN') == '1' then
      vim.fn.mkdir(vim.fs.dirname(GOLDEN), 'p')
      vim.fn.writefile(actual, GOLDEN)
    end
    assert(
      vim.uv.fs_stat(GOLDEN),
      'no ' .. GOLDEN .. '; write it with UPDATE_GOLDEN=1 just lua-test'
    )
    assert.are.same(vim.fn.readfile(GOLDEN), actual)
  end)
end)
