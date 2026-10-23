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

---The index in `rendered.spans` of the span in `group` on line `line`, or `nil`.
---@param rendered orca_term.Rendered
---@param line integer
---@param group string
---@return integer?
local function span_index(rendered, line, group)
  for index, span in ipairs(rendered.spans) do
    if span.line == line and span.group == group then
      return index
    end
  end
  return nil
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

  it(
    "shows the agent's activity on the second line in every agent state, cut to the width",
    function()
      local activities = {
        working = 'Bash: cargo test',
        waiting = 'Which colour?',
        finished = 'All done. Second line here.',
        failed = 'API Error: 400 fake bad request',
        interrupted = 'SLOW please',
      }
      local worktrees = {}
      for state, activity in pairs(activities) do
        table.insert(worktrees, worktree(state, { state = state, activity = activity }))
      end
      table.insert(worktrees, worktree('quiet', { state = 'idle' }))
      local rendered = render.fleet(fleet(worktrees), '/src/wt/quiet')
      for state, activity in pairs(activities) do
        local name = assert(line_with(rendered, render.glyph(state) .. ' ' .. state), state)
        assert.are.equal('   ' .. activity, rendered.lines[name + 1])
      end
      local quiet = assert(rendered.firsts['/src/wt/quiet'])
      assert.are.equal(
        ' ╰────────────────────────────────╯',
        rendered.lines[quiet + 1]
      )

      local long = 'Bash: ' .. string.rep('cargo test --workspace ', 5) .. '\nsecond line'
      local cut = render.fleet(fleet({ worktree('busy', { state = 'working', activity = long }) }))
      assert_fits(cut, render.WIDTH)
      local busy = assert(cut.firsts['/src/wt/busy'])
      assert.are.equal(' │   Bash: cargo test --workspac… │', cut.lines[busy + 1])
    end
  )

  it('shows the detail rather than the activity, after the branch when it differs', function()
    local rendered = render.fleet(
      fleet({
        worktree('both', { state = 'failed', detail = 'the detail', activity = 'the activity' }),
        worktree('moved', { branch = 'other', state = 'finished', activity = 'ok' }),
      }),
      '/src/wt/moved'
    )
    local both = assert(line_with(rendered, 'both'))
    assert.are.equal('   the detail', rendered.lines[both + 1])
    assert.is_nil(line_with(rendered, 'the activity'))
    local moved = assert(rendered.firsts['/src/wt/moved'])
    assert.are.equal(' │   ⎇ other · ok                 │', rendered.lines[moved + 1])
  end)

  it(
    'says how many files changed on the second line when the state has nothing more urgent',
    function()
      local quiet = {
        finished = 'All done.',
        interrupted = 'SLOW please',
        idle = false,
        no_agent = false,
      }
      local worktrees = {}
      for state, activity in pairs(quiet) do
        local fields = { state = state, changed_files = 4, activity = activity or nil }
        table.insert(worktrees, worktree(state, fields))
      end
      table.insert(worktrees, worktree('single', { state = 'finished', changed_files = 1 }))
      table.insert(
        worktrees,
        worktree('clean', { state = 'finished', changed_files = 0, activity = 'ok' })
      )
      table.insert(worktrees, worktree('unknown', { state = 'idle' }))
      table.insert(worktrees, worktree('moved', { branch = 'other', changed_files = 2 }))
      local rendered = render.fleet(fleet(worktrees), '/src/wt/unknown')
      for state in pairs(quiet) do
        local name = assert(line_with(rendered, render.glyph(state) .. ' ' .. state), state)
        assert.are.equal('   4 files changed', rendered.lines[name + 1], state)
      end
      assert.are.equal(
        '   1 file changed',
        rendered.lines[assert(line_with(rendered, 'single')) + 1]
      )
      assert.are.equal('   ok', rendered.lines[assert(line_with(rendered, 'clean')) + 1])
      -- A count the core could not give leaves the line as it was: none, for an idle worktree.
      local unknown = assert(rendered.firsts['/src/wt/unknown'])
      assert.are.equal(
        ' ╰────────────────────────────────╯',
        rendered.lines[unknown + 1]
      )
      local moved = assert(line_with(rendered, 'moved'))
      assert.are.equal('   ⎇ other · 2 files changed', rendered.lines[moved + 1])
      assert.are.same({ 'OrcaTermMuted' }, groups_on(rendered, moved + 1))
      assert_fits(rendered, render.WIDTH)
    end
  )

  it(
    'keeps the question, the error, the setup failure, the reason and the activity first',
    function()
      local urgent = {
        waiting = { activity = 'Which colour?' },
        failed = { activity = 'API Error: 400' },
        not_ready = { detail = 'setup failed (exit 1)' },
        broken = { detail = 'directory missing' },
        working = { activity = 'Bash: sleep 6' },
      }
      local worktrees = {}
      for state, fields in pairs(urgent) do
        local all = vim.tbl_extend('force', { state = state, changed_files = 4 }, fields)
        table.insert(worktrees, worktree(state, all))
      end
      -- A quiet worktree holds the card, so that every urgent row reads plain.
      table.insert(worktrees, worktree('chosen'))
      local rendered = render.fleet(fleet(worktrees), '/src/wt/chosen')
      for state, fields in pairs(urgent) do
        local name = assert(line_with(rendered, render.glyph(state) .. ' ' .. state), state)
        assert.are.equal('   ' .. (fields.detail or fields.activity), rendered.lines[name + 1])
      end
      assert.is_nil(line_with(rendered, 'changed'))
      -- Under Needs you, the count takes the state's colour, as the line always does there.
      local unseen = render.fleet(
        fleet({
          worktree('done', { state = 'finished', unseen = true, changed_files = 2 }),
          worktree('chosen'),
        }),
        '/src/wt/chosen'
      )
      local done = assert(line_with(unseen, '   2 files changed'))
      assert.are.same({ 'OrcaTermStateFinished' }, groups_on(unseen, done))
    end
  )

  it('turns only the working glyph with the spinner frame', function()
    local worktrees = {}
    for state in pairs(render.GLYPHS) do
      table.insert(worktrees, worktree(state, { state = state, activity = 'doing ' .. state }))
    end
    local first = render.fleet(fleet(worktrees), '/src/wt/idle')
    assert.are.same(first, render.fleet(fleet(worktrees), '/src/wt/idle', nil, 0))
    for frame, glyph in ipairs({ '◐', '◓', '◑', '◒', '◐' }) do
      local turned = render.fleet(fleet(worktrees), '/src/wt/idle', nil, frame - 1)
      assert.are.same(first.spans, turned.spans)
      for index, line in ipairs(first.lines) do
        if line == ' ◐ working' then
          assert.are.equal(' ' .. glyph .. ' working', turned.lines[index])
        else
          assert.are.equal(line, turned.lines[index])
        end
      end
    end
    assert.is_true(render.spins(fleet(worktrees)))
    assert.is_false(render.spins(fleet({ worktree('a', { state = 'finished' }) })))
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

  it(
    'puts the unseen mark before the glyph and makes the name bold, seen ones unchanged',
    function()
      local long = string.rep('very-long-', 6)
      local rendered = render.fleet(
        fleet({
          worktree('done', { state = 'finished', unseen = true, activity = 'ok' }),
          worktree('broke', { state = 'failed', unseen = true }),
          worktree('quiet', { state = 'idle', unseen = true }),
          worktree(long, { state = 'interrupted', unseen = true }),
          worktree('looked', { state = 'finished', unseen = false }),
        }),
        '/src/wt/broke'
      )
      assert_fits(rendered, render.WIDTH)

      local done = assert(rendered.firsts['/src/wt/done'])
      assert.are.equal('•● done', rendered.lines[done])
      assert.are.equal('   ok', rendered.lines[done + 1])
      assert.are.same(
        { 'OrcaTermUnseen', 'OrcaTermStateFinished', 'OrcaTermUnseenName' },
        groups_on(rendered, done)
      )
      -- The mark is the first cell of the line, where a seen worktree has a blank.
      assert.are.same(
        { line = done, first = 0, last = #render.UNSEEN, group = 'OrcaTermUnseen' },
        rendered.spans[assert(span_index(rendered, done, 'OrcaTermUnseen'))]
      )

      -- Inside the card too, the mark keeps its cell and the line its width.
      local broke = assert(rendered.firsts['/src/wt/broke'])
      assert.are.equal(' │•● broke                        │', rendered.lines[broke])
      assert.are.same({
        'OrcaTermBorder',
        'OrcaTermUnseen',
        'OrcaTermStateFailed',
        'OrcaTermUnseenName',
        'OrcaTermBorder',
      }, groups_on(rendered, broke))

      local quiet = assert(rendered.firsts['/src/wt/quiet'])
      assert.are.equal('•○ quiet', rendered.lines[quiet])
      local cut = assert(rendered.firsts['/src/wt/' .. long])
      assert.are.equal('•● very-long-very-long-very-long-v…', rendered.lines[cut])

      -- A seen worktree keeps a blank first cell and a plain name.
      local looked = assert(rendered.firsts['/src/wt/looked'])
      assert.are.equal(' ● looked', rendered.lines[looked])
      assert.are.same({ 'OrcaTermStateFinished' }, groups_on(rendered, looked))
    end
  )

  it(
    "flushes a worktree's pending notes ✎N to the right of its second line, and shows none for zero",
    function()
      local rendered = render.fleet(
        fleet({
          worktree('docs', { notes = 2 }),
          worktree('quiet', { notes = 0 }),
          worktree('long', { notes = 12, activity = 'a question long enough to reach the notes' }),
          worktree('picked', { notes = 1 }),
        }),
        '/src/wt/picked'
      )
      assert_fits(rendered, render.WIDTH)

      -- With nothing to say, the second line holds only the notes, flushed right.
      local docs = assert(rendered.firsts['/src/wt/docs'])
      assert.are.equal(' · docs', rendered.lines[docs])
      assert.are.equal('                                 ✎2', rendered.lines[docs + 1])
      assert.are.equal('/src/wt/docs', rendered.paths[docs + 1])
      assert.are.equal(render.WIDTH - 1, vim.fn.strdisplaywidth(rendered.lines[docs + 1]))
      local badge = rendered.spans[assert(span_index(rendered, docs + 1, 'OrcaTermBadge'))]
      assert.are.equal('✎2', rendered.lines[docs + 1]:sub(badge.first + 1, badge.last))

      -- No notes and nothing to say: a single line.
      local quiet = assert(rendered.firsts['/src/wt/quiet'])
      assert.are.equal(' · quiet', rendered.lines[quiet])
      assert.are_not.equal('/src/wt/quiet', rendered.paths[quiet + 1])

      -- What the second line says gives way to the notes, cut with `…`, a space before them.
      local long = assert(rendered.firsts['/src/wt/long'])
      assert.are.equal(' · long', rendered.lines[long])
      assert.are.equal('   a question long enough to r… ✎12', rendered.lines[long + 1])

      -- Inside the card, the notes end before its right border.
      local picked = assert(rendered.firsts['/src/wt/picked'])
      assert.are.equal(' │ · picked                       │', rendered.lines[picked])
      assert.are.equal(' │                             ✎1 │', rendered.lines[picked + 1])
    end
  )

  it("shows a worktree's denials ⊘N before its notes ✎N, and none for zero", function()
    local rendered = render.fleet(
      fleet({
        worktree('both', { denials = 3, notes = 2 }),
        worktree('denied', { denials = 1, notes = 0 }),
        worktree('none', { denials = 0, notes = 0 }),
        worktree('picked', { denials = 12, notes = 1 }),
      }),
      '/src/wt/picked'
    )
    assert_fits(rendered, render.WIDTH)

    local both = assert(rendered.firsts['/src/wt/both'])
    assert.are.equal('                              ⊘3 ✎2', rendered.lines[both + 1])
    assert.are.same({ 'OrcaTermDenied', 'OrcaTermBadge' }, groups_on(rendered, both + 1))
    local denied_span = rendered.spans[assert(span_index(rendered, both + 1, 'OrcaTermDenied'))]
    assert.are.equal('⊘3', rendered.lines[both + 1]:sub(denied_span.first + 1, denied_span.last))

    local denied = assert(rendered.firsts['/src/wt/denied'])
    assert.are.equal('                                 ⊘1', rendered.lines[denied + 1])

    local none = assert(rendered.firsts['/src/wt/none'])
    assert.are.equal(' · none', rendered.lines[none])
    assert.are_not.equal('/src/wt/none', rendered.paths[none + 1])

    local picked = assert(rendered.firsts['/src/wt/picked'])
    assert.are.equal(' │                         ⊘12 ✎1 │', rendered.lines[picked + 1])
  end)

  it(
    'shows ≠ for a git operation in progress, before ⊘N and ✎N, and none without one',
    function()
      local rendered = render.fleet(
        fleet({
          worktree('all', { operation = 'rebase', denials = 3, notes = 2 }),
          worktree('merging', { operation = 'merge' }),
          worktree('none', { denials = 0, notes = 0 }),
        }),
        '/src/wt/none'
      )
      assert_fits(rendered, render.WIDTH)

      local all = assert(rendered.firsts['/src/wt/all'])
      assert.are.equal('                            ≠ ⊘3 ✎2', rendered.lines[all + 1])
      assert.are.same(
        { 'OrcaTermOperation', 'OrcaTermDenied', 'OrcaTermBadge' },
        groups_on(rendered, all + 1)
      )
      local mark = rendered.spans[assert(span_index(rendered, all + 1, 'OrcaTermOperation'))]
      assert.are.equal('≠', rendered.lines[all + 1]:sub(mark.first + 1, mark.last))

      local merging = assert(rendered.firsts['/src/wt/merging'])
      assert.are.equal('                                  ≠', rendered.lines[merging + 1])

      local none = assert(rendered.firsts['/src/wt/none'])
      assert.are.equal(' │ · none                         │', rendered.lines[none])
    end
  )

  it('labels claude-code cc, opencode oc and any other agent by its own name', function()
    assert.are.equal('cc', render.agent_label('claude-code'))
    assert.are.equal('oc', render.agent_label('opencode'))
    assert.are.equal('aider', render.agent_label('aider'))
  end)

  it('says an age as now under a minute, then minutes, hours and days', function()
    local minute = 60 * 1000
    local cases = {
      { 0, 0, 'now' },
      { 59 * 1000, 0, 'now' },
      { 0, 5000, 'now' }, -- a clock behind is now
      { minute, 0, '1m' },
      { 59 * minute, 0, '59m' },
      { 60 * minute, 0, '1h' },
      { 23 * 60 * minute + 59 * minute, 0, '23h' },
      { 24 * 60 * minute, 0, '1d' },
      { 9 * 24 * 60 * minute, 0, '9d' },
    }
    for _, case in ipairs(cases) do
      assert.are.equal(case[3], render.age(case[2], case[1]))
    end
  end)

  it(
    'ends the name line with the agent and the age of its last activity, muted, the icons below',
    function()
      local now = 1700000000000
      local rendered = render.fleet(
        fleet({
          worktree('aaa'),
          worktree('docs-adr', { agent = 'claude-code', last_activity = now - 9 * 60 * 1000 }),
          worktree('ts', { agent = 'opencode', last_activity = now - 3 * 3600 * 1000, notes = 2 }),
          worktree('fresh', { agent = 'claude-code' }),
          worktree('mine', { agent = 'aider', last_activity = now }),
        }),
        '/src/wt/aaa',
        nil,
        0,
        now
      )
      local docs = assert(rendered.firsts['/src/wt/docs-adr'])
      assert.are.equal(' · docs-adr                   cc 9m', rendered.lines[docs])
      assert.are.same(
        { 'OrcaTermStateNoAgent', 'OrcaTermAge', 'OrcaTermAge' },
        groups_on(rendered, docs)
      )
      local ts = assert(rendered.firsts['/src/wt/ts'])
      assert.are.equal(' · ts                         oc 3h', rendered.lines[ts])
      assert.are.equal('                                 ✎2', rendered.lines[ts + 1])
      local fresh = assert(rendered.firsts['/src/wt/fresh'])
      assert.are.equal(' · fresh                         cc', rendered.lines[fresh], 'no age')
      local mine = assert(rendered.firsts['/src/wt/mine'])
      assert.are.equal(' · mine                   aider now', rendered.lines[mine])
      local none = assert(rendered.firsts['/src/wt/aaa'])
      assert.are.equal(' │ · aaa                          │', rendered.lines[none], 'no agent')
    end
  )

  it(
    'cuts the name and the second line first, keeping the agent, the age and the icons in place',
    function()
      local now = 1700000000000
      local long = string.rep('very-long-', 6)
      for _, width in ipairs({ 16, 24, 30, 36, 50 }) do
        -- In the card (the long worktree selected) and out of it (`aaa` selected).
        for _, selected in ipairs({ long, 'aaa' }) do
          local border = selected == long and ' │' or ''
          local rendered = render.fleet(
            fleet({
              worktree('aaa'),
              worktree(long, {
                agent = 'claude-code',
                last_activity = now - 2 * 86400 * 1000,
                activity = string.rep('busy ', 12),
                operation = 'merge',
                denials = 3,
                notes = 1,
              }),
            }),
            '/src/wt/' .. selected,
            width,
            0,
            now
          )
          assert_fits(rendered, width)
          -- At 16 cells there is no room even for the icons, the agent and the age: the row is
          -- cut, and never grows wider.
          if width >= 24 then
            local first = assert(rendered.firsts['/src/wt/' .. long])
            local line = rendered.lines[first]
            assert(line:find('…', 1, true), line)
            assert(vim.endswith(line, 'cc 2d' .. border), line)
            local second = rendered.lines[first + 1]
            assert(second:find('busy', 1, true) and second:find('…', 1, true), second)
            assert(vim.endswith(second, '… ≠ ⊘3 ✎1' .. border), second)
          end
        end
      end
    end
  )

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

describe("render.detail, a worktree's detail", function()
  ---A detail document shaped like `worktree detail --json`, for the worktree `fix`; a worktree
  ---field set to `vim.NIL` is left out.
  ---@param fields? table
  ---@param worktree_fields? table
  ---@return orca_term.Detail
  local function document(fields, worktree_fields)
    local item = worktree(
      'fix',
      vim.tbl_extend('force', {
        state = 'finished',
        agent = 'claude-code',
        holder = false,
        unseen = false,
        notes = 0,
        denials = 0,
      }, worktree_fields or {})
    )
    -- `vim.NIL` leaves a field out, as the core's `null` reads in the client.
    for key, value in pairs(item) do
      if value == vim.NIL then
        item[key] = nil
      end
    end
    return vim.tbl_extend('force', {
      contract = 1,
      version = '0.0.0',
      worktree = item,
      denials = {},
    }, fields or {})
  end

  it(
    'shows the state, the agent, the branch and its drift, and only that when there is nothing else',
    function()
      local rendered =
        render.detail(document({ drift = { against = 'main', ahead = 2, behind = 1 } }), 50)
      assert.are.same({ ' ● finished · claude-code', ' fix  ↑2 ↓1 main' }, rendered.lines)
      assert.are.same(
        { 'OrcaTermStateFinished', 'OrcaTermStateFinished', 'OrcaTermMuted' },
        groups_on(rendered, 1)
      )
      assert.are.same({ 'OrcaTermMuted' }, groups_on(rendered, 2))

      local function branch_line(drift)
        return render.detail(document({ drift = drift }), 50).lines[2]
      end
      assert.are.equal(' fix  ↑2 main', branch_line({ against = 'main', ahead = 2, behind = 0 }))
      assert.are.equal(' fix  ↓4 main', branch_line({ against = 'main', ahead = 0, behind = 4 }))
      assert.are.equal(
        ' fix  up to date with main',
        branch_line({ against = 'main', ahead = 0, behind = 0 })
      )
      -- No local default branch: no drift at all.
      assert.are.equal(' fix', branch_line(nil))

      -- No agent, no branch.
      local bare =
        render.detail(document({}, { state = 'no_agent', agent = vim.NIL, branch = vim.NIL }), 50)
      assert.are.same({ ' · no agent', ' detached' }, bare.lines)
    end
  )

  it('lists how many files the worktree changed under its branch, when it changed some', function()
    local rendered = render.detail(
      document({ drift = { against = 'main', ahead = 1, behind = 0 } }, { changed_files = 3 }),
      50
    )
    assert.are.same(
      { ' ● finished · claude-code', ' fix  ↑1 main', ' 3 files changed' },
      rendered.lines
    )
    assert.are.same({ 'OrcaTermMuted' }, groups_on(rendered, 3))
    assert.are.equal(
      ' 1 file changed',
      render.detail(document({}, { changed_files = 1 }), 50).lines[3]
    )
    assert.are.equal(2, #render.detail(document({}, { changed_files = 0 }), 50).lines)
  end)

  it('names opencode as the agent the same way (#35)', function()
    local rendered = render.detail(document({}, { agent = 'opencode' }), 50)
    assert.are.equal(' ● finished · opencode', rendered.lines[1])
  end)

  it('wraps the whole error to the width in the state colour, never cutting it', function()
    local error = 'API Error: 400 '
      .. string.rep('a long message that goes on and on ', 8)
      .. '/a/path/far/too/long/to/fit/on/one/line/of/the/float/at/all/whatever/its/width/is'
    local rendered = render.detail(document({ detail = error }, { state = 'failed' }), 40)
    assert_fits(rendered, 39)
    assert.are.equal('', rendered.lines[3])
    local words = {}
    for index = 4, #rendered.lines do
      assert.are.same({ 'OrcaTermStateFailed' }, groups_on(rendered, index))
      table.insert(words, vim.trim(rendered.lines[index]))
    end
    assert.is_nil(table.concat(rendered.lines):find('…', 1, true))
    assert.are.equal((error:gsub('%s', '')), (table.concat(words):gsub('%s', '')))
  end)

  it('lists the denials after a muted heading, each after ⊘, wrapped under its text', function()
    local long = 'Write: /a/very/long/path/to/a/denied/directory/and/a/file/inside/it.txt'
    local rendered = render.detail(
      document({
        denials = {
          { tool = 'Bash', text = 'Bash: FOO=bar ls' },
          { tool = 'Write', text = long },
        },
      }),
      40
    )
    assert_fits(rendered, 39)
    assert.are.same({
      ' ● finished · claude-code',
      ' fix',
      '',
      ' Denied',
      ' ⊘ Bash: FOO=bar ls',
      ' ⊘ Write:',
      '   /a/very/long/path/to/a/denied/direct',
      '   ory/and/a/file/inside/it.txt',
    }, rendered.lines)
    assert.are.same({ 'OrcaTermMuted' }, groups_on(rendered, 4))
    assert.are.same({ 'OrcaTermDenied' }, groups_on(rendered, 5))
    local mark = rendered.spans[assert(span_index(rendered, 5, 'OrcaTermDenied'))]
    assert.are.equal('⊘', rendered.lines[5]:sub(mark.first + 1, mark.last))
    assert.are.same({}, groups_on(rendered, 7))
  end)

  it("shows the setup's status and the end of its log, muted, after the error", function()
    local rendered = render.detail(
      document({
        detail = 'setup failed (exit 1): npm ERR! missing script: build',
        setup = {
          status = 'failed',
          log = '/r/setup.log',
          tail = { 'installing', '', 'npm ERR! missing script: build' },
        },
      }, { state = 'not_ready', agent = vim.NIL }),
      60
    )
    assert.are.same({
      ' △ not ready',
      ' fix',
      '',
      ' setup failed (exit 1): npm ERR! missing script: build',
      '',
      ' Setup (failed)',
      ' installing',
      ' npm ERR! missing script: build',
    }, rendered.lines)
    assert.are.same({ 'OrcaTermStateNotReady' }, groups_on(rendered, 4))
    for index = 6, 8 do
      assert.are.same({ 'OrcaTermMuted' }, groups_on(rendered, index))
    end
  end)

  it('names a git operation in progress after the branch, with a rebase step', function()
    local rendered = render.detail(
      document({
        drift = { against = 'main', ahead = 1, behind = 1 },
        operation_step = { current = 2, total = 5 },
        detail = 'API Error: 400',
      }, { state = 'failed', operation = 'rebase' }),
      50
    )
    assert.are.same({
      ' ● failed · claude-code',
      ' fix  ↑1 ↓1 main',
      '',
      ' rebase in progress (2/5)',
      '',
      ' API Error: 400',
    }, rendered.lines)
    assert.are.same({ 'OrcaTermOperation' }, groups_on(rendered, 4))

    local function operation_line(operation)
      return render.detail(document({}, { operation = operation }), 50).lines[4]
    end
    assert.are.equal(' merge in progress', operation_line('merge'))
    assert.are.equal(' cherry-pick in progress', operation_line('cherry-pick'))
    assert.are.equal(' revert in progress', operation_line('revert'))
    assert.are.equal(' rebase in progress', operation_line('rebase'))
    assert.are.equal(2, #render.detail(document({}), 50).lines, 'no operation, no line')
  end)

  it('reads a setup not approved in words', function()
    local rendered = render.detail(
      document({
        detail = 'setup not approved; run orca-term worktree setup fix',
        setup = { status = 'not_approved', log = '/r/setup.log', tail = {} },
      }, { state = 'not_ready', agent = vim.NIL }),
      60
    )
    assert.are.equal(' Setup (not approved)', rendered.lines[#rendered.lines])
  end)
end)
