-- lua/orca-term/fleet.lua, the attention order: pure, so these specs hand it plain tables. The
-- `unseen` mark and the agent states are not printed by the core yet; the order already knows
-- where they go.

local fleet = require('orca-term.fleet')

---A worktree as `worktree ls --json` prints it, plus the marks given.
---@param name string
---@param state string
---@param unseen? boolean
---@return orca_term.RankedWorktree
local function worktree(name, state, unseen)
  return { name = name, path = '/w/' .. name, branch = name, state = state, unseen = unseen }
end

---The names in each section, as `{ title, name, name, ... }`.
---@param sections orca_term.Section[]
---@return string[][]
local function names(sections)
  local out = {}
  for _, section in ipairs(sections) do
    local row = { section.title }
    for _, item in ipairs(section.worktrees) do
      table.insert(row, item.name)
    end
    table.insert(out, row)
  end
  return out
end

describe('fleet', function()
  it('ranks every state as the spec orders attention', function()
    local cases = {
      { worktree('a', 'waiting'), 1 },
      { worktree('b', 'failed', true), 2 },
      { worktree('c', 'interrupted', true), 2 },
      { worktree('d', 'broken'), 3 },
      { worktree('e', 'not_ready'), 3 },
      { worktree('f', 'finished', true), 4 },
      { worktree('g', 'idle', true), 4 },
      { worktree('h', 'working'), 5 },
      { worktree('i', 'setting_up'), 5 },
      { worktree('j', 'finished'), 6 },
      { worktree('k', 'failed'), 6 },
      { worktree('l', 'interrupted'), 6 },
      { worktree('m', 'idle'), 7 },
      { worktree('n', 'no_agent'), 8 },
      { worktree('o', 'a_state_from_a_newer_core'), 8 },
    }
    for _, case in ipairs(cases) do
      local item, expected = case[1], case[2]
      assert.are.equal(expected, fleet.rank(item), item.state)
    end
  end)

  it('puts ranks 1 to 4 under Needs you, 5 under Running and the rest under Quiet', function()
    local sections = fleet.sections({
      worktree('quiet-idle', 'idle'),
      worktree('needs-waiting', 'waiting'),
      worktree('running', 'working'),
      worktree('quiet-none', 'no_agent'),
      worktree('needs-unseen', 'finished', true),
      worktree('quiet-seen', 'finished'),
      worktree('needs-broken', 'broken'),
    })
    assert.are.same({
      { 'Needs you', 'needs-waiting', 'needs-broken', 'needs-unseen' },
      { 'Running', 'running' },
      { 'Quiet', 'quiet-seen', 'quiet-idle', 'quiet-none' },
    }, names(sections))
  end)

  it('orders worktrees of the same rank by name', function()
    local sections = fleet.sections({
      worktree('zeta', 'no_agent'),
      worktree('alpha', 'no_agent'),
      worktree('mid', 'no_agent'),
    })
    assert.are.same({ { 'Quiet', 'alpha', 'mid', 'zeta' } }, names(sections))
  end)

  it('leaves out empty sections and returns none for an empty fleet', function()
    assert.are.same({ { 'Running', 'x' } }, names(fleet.sections({ worktree('x', 'setting_up') })))
    assert.are.same({}, fleet.sections({}))
  end)
end)
