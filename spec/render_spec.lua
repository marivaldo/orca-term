-- lua/orca-term/render.lua, the sidebar's lines and colours: pure, so these specs hand it plain
-- tables shaped like `worktree ls --json` and read the lines it returns.

local render = require('orca-term.render')

---A fleet whose primary checkout is `/src/orca-term` on `main`, with `worktrees`.
---@param worktrees orca_term.Worktree[]
---@return orca_term.Fleet
local function fleet(worktrees)
  return {
    contract = 1,
    version = '0.0.0',
    primary = { path = '/src/orca-term', branch = 'main' },
    worktrees = worktrees,
  }
end

---A worktree on a branch of its own name, unless `fields` says otherwise.
---@param name string
---@param fields? table
---@return orca_term.Worktree
local function worktree(name, fields)
  local item = { name = name, path = '/src/wt/' .. name, branch = name, state = 'no_agent' }
  return vim.tbl_extend('force', item, fields or {})
end

---Fails unless every line fits in `width` screen cells.
---@param rendered orca_term.Rendered
---@param width integer
local function assert_fits(rendered, width)
  for index, line in ipairs(rendered.lines) do
    local cells = vim.fn.strdisplaywidth(line)
    assert(cells <= width, ('line %d is %d cells wide: %s'):format(index, cells, line))
  end
end

---The groups of the spans on line `line`, in order.
---@param rendered orca_term.Rendered
---@param line integer
---@return string[]
local function groups_on(rendered, line)
  local groups = {}
  for _, span in ipairs(rendered.spans) do
    if span.line == line then
      table.insert(groups, span.group)
    end
  end
  return groups
end

---The line holding `text`, or `nil`.
---@param rendered orca_term.Rendered
---@param text string
---@return integer?
local function line_with(rendered, text)
  for index, line in ipairs(rendered.lines) do
    if line:find(text, 1, true) then
      return index
    end
  end
  return nil
end

describe('render', function()
  it('shows the repository and the primary branch in the header, never as a row', function()
    local rendered = render.fleet(fleet({ worktree('fix') }))
    assert.are.equal(' ▾ orca-term  main', rendered.lines[1])
    assert.are.same({ 'OrcaTermMuted', 'OrcaTermHeader', 'OrcaTermMuted' }, groups_on(rendered, 1))
    assert.are.same({ '/src/wt/fix' }, rendered.order)
    for _, path in pairs(rendered.paths) do
      assert.are_not.equal('/src/orca-term', path)
    end
  end)

  it('never draws a line wider than the width, however long the names and details', function()
    local long = string.rep('very-long-', 10)
    local rendered = render.fleet(
      fleet({
        worktree(long .. 'a', { state = 'not_ready', detail = string.rep('detail ', 20) }),
        worktree(long .. 'b', { branch = long .. 'branch', detail = 'x' }),
        worktree(long .. 'c', { branch = 'wide-日本語-' .. long }),
      }),
      '/src/wt/' .. long .. 'b'
    )
    assert_fits(rendered, render.WIDTH)
    assert.truthy(line_with(rendered, '…'))
    for _, width in ipairs({ 20, 30, 50 }) do
      assert_fits(render.fleet(fleet({ worktree(long) }), nil, width), width)
    end
  end)

  it('shows the branch only when it differs from the name, and a detached one', function()
    local loose = worktree('loose')
    loose.branch = nil
    local rendered = render.fleet(
      fleet({
        worktree('same'),
        worktree('moved', { branch = 'other' }),
        loose,
        worktree('broken', { state = 'broken', detail = 'directory missing' }),
        worktree('away', { branch = 'elsewhere', state = 'broken', detail = 'gone' }),
      }),
      '/src/wt/same'
    )
    assert.truthy(line_with(rendered, '· same'))
    assert.is_nil(line_with(rendered, '⎇ same'))
    assert.are.equal('   ⎇ other', rendered.lines[assert(line_with(rendered, '· moved')) + 1])
    assert.are.equal('   ⎇ detached', rendered.lines[assert(line_with(rendered, '· loose')) + 1])
    assert.are.equal(
      '   directory missing',
      rendered.lines[assert(line_with(rendered, '! broken')) + 1]
    )
    assert.are.equal(
      '   ⎇ elsewhere · gone',
      rendered.lines[assert(line_with(rendered, '! away')) + 1]
    )
  end)

  it('colours the second line by state under Needs you and mutes it elsewhere', function()
    local rendered = render.fleet(
      fleet({
        worktree('fail', { state = 'not_ready', detail = 'setup failed' }),
        worktree('moved', { branch = 'other' }),
        worktree('selected'),
      }),
      '/src/wt/selected'
    )
    local fail = assert(line_with(rendered, 'fail'))
    assert.are.same({ 'OrcaTermStateNotReady' }, groups_on(rendered, fail))
    assert.are.same({ 'OrcaTermStateNotReady' }, groups_on(rendered, fail + 1))
    local moved = assert(line_with(rendered, 'moved'))
    assert.are.same({ 'OrcaTermStateNoAgent' }, groups_on(rendered, moved))
    assert.are.same({ 'OrcaTermMuted' }, groups_on(rendered, moved + 1))
  end)

  it('draws the card around the selected worktree only', function()
    local rendered = render.fleet(
      fleet({ worktree('a'), worktree('b', { branch = 'other' }), worktree('c') }),
      '/src/wt/b'
    )
    assert.are.same({
      ' ▾ orca-term  main',
      '',
      ' QUIET 3',
      ' · a',
      ' ╭────────────────────────────────╮',
      ' │ · b                            │',
      ' │   ⎇ other                      │',
      ' ╰────────────────────────────────╯',
      ' · c',
    }, rendered.lines)
    assert.are.equal('/src/wt/b', rendered.selected)
    assert.are.equal(6, rendered.firsts['/src/wt/b'])
    assert.are.same(
      { 'OrcaTermBorder', 'OrcaTermStateNoAgent', 'OrcaTermBorder' },
      groups_on(rendered, 6)
    )
  end)

  it('selects the first worktree shown when the selected one is gone', function()
    local rendered = render.fleet(
      fleet({ worktree('quiet'), worktree('urgent', { state = 'broken' }) }),
      '/src/wt/gone'
    )
    assert.are.equal('/src/wt/urgent', rendered.selected)
    assert.are.same({ '/src/wt/urgent', '/src/wt/quiet' }, rendered.order)
    assert.are.equal(
      ' ╭────────────────────────────────╮',
      rendered.lines[4]
    )
  end)

  it('says how to make a worktree when the fleet is empty', function()
    local rendered = render.fleet(fleet({}))
    assert.are.same({
      ' ▾ orca-term  main',
      '',
      ' no worktrees yet',
      ' orca-term worktree new <name>',
    }, rendered.lines)
    assert.is_nil(rendered.selected)
  end)

  it('wraps an error to the width, in the error colour', function()
    local message = 'orca-term: no core binary found; run `cargo build` in '
      .. '/a/very/long/path/that/does/not/fit/on/one/line/at/all or set `binary` in setup()'
    local rendered = render.error(message)
    assert.are.equal(' ▾ orca-term', rendered.lines[1])
    assert.are.equal('', rendered.lines[2])
    assert_fits(rendered, render.WIDTH)
    local words = {}
    for index = 3, #rendered.lines do
      assert.are.same({ 'OrcaTermError' }, groups_on(rendered, index))
      table.insert(words, vim.trim(rendered.lines[index]))
    end
    -- Nothing is lost: joined back, the lines hold the whole message.
    assert.are.equal((message:gsub('%s', '')), (table.concat(words):gsub('%s', '')))
  end)
end)
