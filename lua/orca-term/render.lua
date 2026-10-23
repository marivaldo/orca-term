-- orca-term's sidebar: what it shows, as lines of text and the colours on them.
--
-- Pure: it takes the fleet (or an error) and returns lines, highlight spans and which worktree
-- each line belongs to. It never touches a buffer or a window; `sidebar.lua` paints the result.
-- It also counts the fleet for the status strip (`M.strip`, `M.counts`), which `strip.lua` and the
-- lualine component draw, and lays out a worktree's detail (`M.detail`), which `detail.lua` shows in
-- a float.
-- The only Neovim calls measure text in screen cells (`vim.fn.strdisplaywidth`,
-- `vim.fn.strcharpart`, `vim.fn.strchars`) or are pure helpers (`vim.fs.basename`,
-- `vim.list_extend`, `vim.tbl_map`, `vim.split`).
--
-- The layout is prototype variant D (branch `prototype/fleet-sidebar`, spec issue #25). This is
-- spec/golden/sidebar.txt, with `pick-colour` selected; `•` marks the unseen worktrees, `≠` a git
-- operation in progress (a merge here), `⊘N` counts the tool calls a worktree's latest turn had
-- denied, and `✎N` its pending notes (`PR`, after them, marks a worktree whose branch has a pull
-- request, coloured by its state and checks). The agent (`cc`) and the age of the last activity
-- (`now`, `9m`, `3h`, `2d`) end the name line:
--
--   ▾ repo  main
--
--   NEEDS YOU 7
--   ╭────────────────────────────────╮
--   │ ? pick-colour     ⊘3 ✎1 cc now │
--   │   Which colour?                │
--   ╰────────────────────────────────╯
--  •● slow-job                   cc 9m
--     SLOW please
--  •● api-call                   cc 3h
--     API Error: 400 fake bad request
--   △ fix-login
--     setup failed (exit 1): npm ERR!…
--   ! old-spike
--     directory missing; run orca-ter…
--  •○ nudged                     cc 5m
--  •● greet                      cc 2d
--     ok
--
--   RUNNING 2
--   ◐ run-tests                 cc now
--     Bash: sleep 6
--   ◌ install-deps
--
--   QUIET 4
--   ● reviewed                   cc 1d
--     ok
--   ○ standby                       cc
--   · docs                        ≠ ✎2
--   · spike
--     ⎇ other-branch
--
-- Nothing reaches the last column: rows end one cell before `width`, where the card's right
-- border ends.

local fleet_order = require('orca-term.fleet')
local highlights = require('orca-term.highlights')

local M = {}

---The sidebar's width in screen cells, unless the caller says otherwise.
M.WIDTH = 36

---Each state's glyph, from the spec. A state this client does not know yet shows `·`.
---Working spins: see `M.SPINNER`.
---@type table<string, string>
M.GLYPHS = {
  working = '◐',
  waiting = '?',
  finished = '●',
  failed = '●',
  interrupted = '●',
  idle = '○',
  no_agent = '·',
  not_ready = '△',
  setting_up = '◌',
  broken = '!',
}

---The mark of an unseen worktree, in the first cell of its name line, before its glyph.
M.UNSEEN = '•'

---The mark of notes: before a worktree's count of pending notes at the right of its name line, and
---before each note `notes.lua` draws in a review.
M.NOTE = '✎'

---The mark of denials: before a worktree's count of denied tool calls at the right of its name
---line, and before each denied call in its detail.
M.DENIED = '⊘'

---The mark of a git operation in progress (a merge, a rebase, a cherry-pick or a revert) at the
---right of a worktree's name line.
M.OPERATION = '≠'
---The icon of a worktree's pull request, after its notes at the right of its name line, in the
---colour `M.pr_group` gives it.
M.PR = 'PR'

---The group an open pull request is drawn in, by its checks; with no checks it is
---`OrcaTermPrOpen`.
local PR_CHECK_GROUPS = {
  passing = 'OrcaTermPrPassing',
  failing = 'OrcaTermPrFailing',
  pending = 'OrcaTermPrPending',
}

---The group a pull request that is not open is drawn in, by its state.
local PR_STATE_GROUPS = {
  merged = 'OrcaTermPrMerged',
  closed = 'OrcaTermPrClosed',
  draft = 'OrcaTermPrDraft',
}

---The short label of each agent the core knows, at the right of a worktree's name line. Any other
---agent shows its own name.
---@type table<string, string>
M.AGENT_LABELS = {
  ['claude-code'] = 'cc',
  opencode = 'oc',
}

---The frames of the working glyph, one after the other: frame 0 is the first, as `M.GLYPHS`
---shows it. `sidebar.lua` turns them while a worktree is working.
M.SPINNER = { '◐', '◓', '◑', '◒' }

---A highlight on one line: `group` from byte `first` up to (not including) byte `last`.
---@class orca_term.Span
---@field line integer 1-based, an index into `lines`
---@field first integer 0-based byte column
---@field last integer 0-based byte column, exclusive
---@field group string

---What the sidebar shows.
---@class orca_term.Rendered
---@field lines string[]
---@field spans orca_term.Span[]
---@field paths table<integer, string> each line that belongs to a worktree, and that worktree's path
---@field firsts table<string, integer> each worktree's path, and the line holding its name
---@field order string[] the worktrees' paths, top to bottom
---@field selected? string the selected worktree's path; `nil` when there is no worktree

---A line being built: its text so far, its spans, and how many cells it may take.
---@class orca_term.Line
---@field text string
---@field spans { first: integer, last: integer, group: string }[]
---@field budget integer
---@field path? string
---@field first? boolean whether this line holds the worktree's name

---The width of `text` in screen cells.
---@param text string
---@return integer
local function cells(text)
  return vim.fn.strdisplaywidth(text)
end

---The glyph of `state` at spinner frame `frame`: only `working` changes with the frame.
---@param state string
---@param frame? integer 0 or more; `nil` is frame 0
---@return string
function M.glyph(state, frame)
  if state == 'working' then
    return M.SPINNER[(frame or 0) % #M.SPINNER + 1]
  end
  return M.GLYPHS[state] or '·'
end

---Whether any worktree of `fleet` is working, so its glyph spins.
---@param fleet orca_term.Fleet
---@return boolean
function M.spins(fleet)
  for _, worktree in ipairs(fleet.worktrees) do
    if worktree.state == 'working' then
      return true
    end
  end
  return false
end

---The longest start of `text` that fits in `width` cells, and what is left after it.
---@param text string
---@param width integer
---@return string head
---@return string rest
local function split_at(text, width)
  local head = ''
  local count = vim.fn.strchars(text)
  for index = 0, count - 1 do
    local char = vim.fn.strcharpart(text, index, 1)
    if cells(head .. char) > width then
      return head, vim.fn.strcharpart(text, index)
    end
    head = head .. char
  end
  return head, ''
end

---`text` cut to `width` cells, ending in `…` when it was cut.
---@param text string
---@param width integer
---@return string
local function truncate(text, width)
  if width <= 0 then
    return ''
  end
  if cells(text) <= width then
    return text
  end
  local head = split_at(text, width - 1)
  return head .. '…'
end

---`text` on one line: tabs, newlines and other control characters become spaces.
---@param text string
---@return string
local function one_line(text)
  return (text:gsub('%c', ' '))
end

---`text` broken into lines of at most `width` cells, at spaces where it can.
---@param text string
---@param width integer
---@return string[]
local function wrap(text, width)
  local lines = {}
  local current = ''
  for word in one_line(text):gmatch('%S+') do
    -- A word longer than a whole line (a path, say) is cut wherever the line ends.
    while cells(word) > width do
      if current ~= '' then
        table.insert(lines, current)
        current = ''
      end
      local head, rest = split_at(word, width)
      table.insert(lines, head)
      word = rest
    end
    if current == '' then
      current = word
    elseif cells(current) + 1 + cells(word) <= width then
      current = current .. ' ' .. word
    else
      table.insert(lines, current)
      current = word
    end
  end
  if current ~= '' then
    table.insert(lines, current)
  end
  return lines
end

---A new, empty line that may take `budget` cells.
---@param budget integer
---@param path? string
---@return orca_term.Line
local function new_line(budget, path)
  return { text = '', spans = {}, budget = budget, path = path }
end

---Adds `text` to the end of `line`, in `group` when given. Text that does not fit the line's
---budget is cut with `…`, so a line never grows past it.
---@param line orca_term.Line
---@param text string
---@param group? string
local function add(line, text, group)
  local room = line.budget - cells(line.text)
  local fitted = truncate(text, room)
  if fitted == '' then
    return
  end
  local first = #line.text
  line.text = line.text .. fitted
  if group then
    table.insert(line.spans, { first = first, last = #line.text, group = group })
  end
end

---The header: ` ▾ <repository>  <branch of the primary checkout>`. The primary checkout is only
---ever shown here, never as a row.
---@param repo string
---@param branch? string
---@param width integer
---@return orca_term.Line
local function header(repo, branch, width)
  local line = new_line(width - 1)
  add(line, ' ')
  add(line, '▾', 'OrcaTermMuted')
  add(line, ' ')
  add(line, repo, 'OrcaTermHeader')
  if branch then
    add(line, '  ')
    add(line, branch, 'OrcaTermMuted')
  end
  return line
end

---What a worktree's second line says: its detail or, when it has none, its agent's activity,
---after its branch when the branch is not named like the worktree. An empty string means the
---worktree has no second line.
---@param worktree orca_term.Worktree
---@return string
local function second_line_text(worktree)
  local said = worktree.detail or worktree.activity
  local text = said and one_line(said) or ''
  if worktree.branch ~= worktree.name then
    local branch = '⎇ ' .. (worktree.branch or 'detached')
    if text == '' then
      text = branch
    else
      text = branch .. ' · ' .. text
    end
  end
  return text
end

---The icons at the right of a worktree's name line, from left to right: `≠` for a git operation
---in progress, `⊘N` for the tool calls its latest turn had denied, then `✎N` for its pending
---notes. Empty when it has nothing to show.
---The group the pull request `pr` is drawn in: an open one in the colour of its checks (passing,
---failing, pending; `OrcaTermPrOpen` with none), a merged one `OrcaTermPrMerged`, a closed one
---`OrcaTermPrClosed`, a draft `OrcaTermPrDraft`.
---@param pr orca_term.Pr
---@return string
function M.pr_group(pr)
  if pr.state == 'open' then
    return PR_CHECK_GROUPS[pr.checks] or 'OrcaTermPrOpen'
  end
  return PR_STATE_GROUPS[pr.state] or 'OrcaTermMuted'
end

---What `pr refresh` found, in words: `PR #12 open, checks passing` (`no checks` when there are
---none), `No PR`, or `PR unavailable: <why>`.
---@param pr orca_term.Pr
---@return string
function M.pr_text(pr)
  if pr.number then
    local checks = (pr.checks and pr.checks ~= 'none') and ('checks ' .. pr.checks) or 'no checks'
    return ('PR #%d %s, %s'):format(pr.number, pr.state or '?', checks)
  end
  if pr.unavailable then
    return 'PR unavailable: ' .. pr.unavailable
  end
  return 'No PR'
end

---How long ago `then_ms` was at `now_ms` (both in milliseconds since the Unix epoch), in words:
---`just now` under a minute, then `2 min ago`, `3 h ago`, `4 d ago`.
---@param then_ms integer
---@param now_ms integer
---@return string
function M.ago(then_ms, now_ms)
  local seconds = math.max(0, math.floor((now_ms - then_ms) / 1000))
  if seconds < 60 then
    return 'just now'
  end
  local minutes = math.floor(seconds / 60)
  if minutes < 60 then
    return minutes .. ' min ago'
  end
  local hours = math.floor(minutes / 60)
  if hours < 24 then
    return hours .. ' h ago'
  end
  return math.floor(hours / 24) .. ' d ago'
end

---The label of `agent` at the right of a worktree's name line: `cc` for `claude-code`, `oc` for
---`opencode`, any other agent's own name.
---@param agent string
---@return string
function M.agent_label(agent)
  return M.AGENT_LABELS[agent] or agent
end

---How long ago `then_ms` was at `now_ms` (both in milliseconds since the Unix epoch), short, as a
---worktree's name line shows it: `now` under a minute, then `9m`, `3h`, `2d`.
---@param then_ms integer
---@param now_ms integer
---@return string
function M.age(then_ms, now_ms)
  local seconds = math.max(0, math.floor((now_ms - then_ms) / 1000))
  if seconds < 60 then
    return 'now'
  end
  local minutes = math.floor(seconds / 60)
  if minutes < 60 then
    return minutes .. 'm'
  end
  local hours = math.floor(minutes / 60)
  if hours < 24 then
    return hours .. 'h'
  end
  return math.floor(hours / 24) .. 'd'
end

---What ends a worktree's name line: its agent's label and the age of its last activity at
---`now_ms` (`cc 9m`), both muted. Only the label when its last activity is unknown, and nothing
---when it has no agent.
---@param worktree orca_term.Worktree
---@param now_ms integer
---@return { text: string, group: string }[]
local function agent_and_age(worktree, now_ms)
  if not worktree.agent then
    return {}
  end
  local found = { { text = M.agent_label(worktree.agent), group = 'OrcaTermAge' } }
  if worktree.last_activity then
    table.insert(found, { text = M.age(worktree.last_activity, now_ms), group = 'OrcaTermAge' })
  end
  return found
end

---The icons at the right of a worktree's name line, from left to right: `≠` for a git operation
---in progress, `⊘N` for the tool calls its latest turn had denied, `✎N` for its pending notes,
---then `PR` for its pull request. Empty when it has nothing to show.
---@param worktree orca_term.Worktree
---@return { text: string, group: string }[]
local function icons(worktree)
  local found = {}
  if worktree.operation then
    table.insert(found, { text = M.OPERATION, group = 'OrcaTermOperation' })
  end
  if (worktree.denials or 0) > 0 then
    table.insert(found, { text = M.DENIED .. worktree.denials, group = 'OrcaTermDenied' })
  end
  if (worktree.notes or 0) > 0 then
    table.insert(found, { text = M.NOTE .. worktree.notes, group = 'OrcaTermBadge' })
  end
  if worktree.pr and worktree.pr.number then
    table.insert(found, { text = M.PR, group = M.pr_group(worktree.pr) })
  end
  return found
end

---Adds `name` to `line`, in `group` when given, then `right` (the icons, then the agent and the
---age) flushed to the right end of the line's budget, a space between them. The name is cut with
---`…` to leave them room, and a space before them.
---@param line orca_term.Line
---@param name string
---@param group? string
---@param right { text: string, group: string }[]
local function add_name(line, name, group, right)
  if #right == 0 then
    add(line, name, group)
    return
  end
  local texts = vim.tbl_map(function(icon)
    return icon.text
  end, right)
  local right_cells = cells(table.concat(texts, ' '))
  add(line, truncate(name, line.budget - cells(line.text) - right_cells - 1), group)
  add(line, string.rep(' ', line.budget - cells(line.text) - right_cells))
  for index, icon in ipairs(right) do
    if index > 1 then
      add(line, ' ')
    end
    add(line, icon.text, icon.group)
  end
end

---The lines of one worktree: its name line and, when it has something to say, its second line.
---@param worktree orca_term.RankedWorktree
---@param needs_you boolean whether it is under "Needs you", which colours its second line
---@param budget integer the cells its lines may take
---@param frame integer the spinner frame
---@param now integer the time the age of its last activity is measured from, in milliseconds
---since the Unix epoch
---@return orca_term.Line[]
local function worktree_lines(worktree, needs_you, budget, frame, now)
  local state_group = highlights.state_group(worktree.state)

  local name_line = new_line(budget, worktree.path)
  name_line.first = true
  -- The unseen mark takes the first cell, before the glyph, and the name turns bold; a seen
  -- worktree leaves the cell blank.
  local unseen = worktree.unseen == true
  if unseen then
    add(name_line, M.UNSEEN, 'OrcaTermUnseen')
  else
    add(name_line, ' ')
  end
  add(name_line, M.glyph(worktree.state, frame), state_group)
  add(name_line, ' ')
  -- The icons sit at the right of this line, then the agent and the age of its last activity.
  local right = icons(worktree)
  vim.list_extend(right, agent_and_age(worktree, now))
  add_name(name_line, worktree.name, unseen and 'OrcaTermUnseenName' or nil, right)
  local lines = { name_line }

  local text = second_line_text(worktree)
  if text ~= '' then
    local detail_line = new_line(budget, worktree.path)
    add(detail_line, '   ')
    add(detail_line, text, needs_you and state_group or 'OrcaTermMuted')
    table.insert(lines, detail_line)
  end
  return lines
end

---`body` inside the rounded card that marks the selected worktree, added to `out`.
---@param out orca_term.Line[]
---@param body orca_term.Line[]
---@param path string
---@param width integer
local function add_card(out, body, path, width)
  local inner = width - 4

  local top = new_line(width - 1, path)
  add(top, ' ')
  add(top, '╭' .. string.rep('─', inner) .. '╮', 'OrcaTermBorder')
  table.insert(out, top)

  for _, line in ipairs(body) do
    local boxed = new_line(width - 1, path)
    boxed.first = line.first
    add(boxed, ' ')
    add(boxed, '│', 'OrcaTermBorder')
    local shift = #boxed.text
    boxed.text = boxed.text .. line.text .. string.rep(' ', inner - cells(line.text))
    for _, span in ipairs(line.spans) do
      table.insert(boxed.spans, {
        first = span.first + shift,
        last = span.last + shift,
        group = span.group,
      })
    end
    add(boxed, '│', 'OrcaTermBorder')
    table.insert(out, boxed)
  end

  local bottom = new_line(width - 1, path)
  add(bottom, ' ')
  add(bottom, '╰' .. string.rep('─', inner) .. '╯', 'OrcaTermBorder')
  table.insert(out, bottom)
end

---Turns the lines built into what the sidebar shows.
---@param built orca_term.Line[]
---@param order string[]
---@param selected? string
---@return orca_term.Rendered
local function finish(built, order, selected)
  local rendered = {
    lines = {},
    spans = {},
    paths = {},
    firsts = {},
    order = order,
    selected = selected,
  }
  for index, line in ipairs(built) do
    rendered.lines[index] = line.text
    for _, span in ipairs(line.spans) do
      table.insert(rendered.spans, {
        line = index,
        first = span.first,
        last = span.last,
        group = span.group,
      })
    end
    if line.path then
      rendered.paths[index] = line.path
      if line.first then
        rendered.firsts[line.path] = index
      end
    end
  end
  return rendered
end

---The time the sidebar measures ages from when it is given none, in milliseconds since the Unix
---epoch: the clock's. The golden spec replaces it, so the golden never depends on the clock.
---@return integer
function M.clock()
  return os.time() * 1000
end

---The sidebar for `fleet`, with the worktree at path `selected` in the card. When no worktree
---has that path (it is `nil`, or the worktree is gone), the first worktree shown is selected.
---A working worktree's glyph is the spinner's frame `frame`. The age of each worktree's last
---activity is measured from `now`, in milliseconds since the Unix epoch (the clock's when `nil`).
---@param fleet orca_term.Fleet
---@param selected? string
---@param width? integer
---@param frame? integer 0 or more; `nil` is frame 0
---@param now? integer
---@return orca_term.Rendered
function M.fleet(fleet, selected, width, frame, now)
  width = width or M.WIDTH
  frame = frame or 0
  now = now or M.clock()
  local sections = fleet_order.sections(fleet.worktrees)

  local order = {}
  local known = false
  for _, section in ipairs(sections) do
    for _, worktree in ipairs(section.worktrees) do
      table.insert(order, worktree.path)
      if worktree.path == selected then
        known = true
      end
    end
  end
  if not known then
    selected = order[1]
  end

  local repo = vim.fs.basename(fleet.primary.path)
  local built = { header(repo, fleet.primary.branch or 'detached', width) }

  if #sections == 0 then
    table.insert(built, new_line(width - 1))
    for _, text in ipairs({ 'no worktrees yet', 'orca-term worktree new <name>' }) do
      local line = new_line(width - 1)
      add(line, ' ')
      add(line, text, 'OrcaTermMuted')
      table.insert(built, line)
    end
    return finish(built, order, selected)
  end

  for _, section in ipairs(sections) do
    table.insert(built, new_line(width - 1))
    local heading = new_line(width - 1)
    add(heading, ' ')
    add(heading, section.title:upper() .. ' ' .. #section.worktrees, 'OrcaTermMuted')
    table.insert(built, heading)

    local needs_you = section.title == fleet_order.TITLES[1]
    for _, worktree in ipairs(section.worktrees) do
      if worktree.path == selected then
        -- Inside the card, a row has 4 cells fewer: ` │` on its left and `│` on its right.
        local body = worktree_lines(worktree, needs_you, width - 5, frame, now)
        add_card(built, body, worktree.path, width)
      else
        vim.list_extend(built, worktree_lines(worktree, needs_you, width - 1, frame, now))
      end
    end
  end
  return finish(built, order, selected)
end

---The smallest width `M.detail` lays out to: narrower, a wrapped line would hold almost nothing.
local DETAIL_SMALLEST = 12

---`text` broken at its own line breaks, then each line wrapped to `width` cells; blank lines are
---left out.
---@param text string
---@param width integer
---@return string[]
local function wrap_lines(text, width)
  local lines = {}
  for _, line in ipairs(vim.split(text, '\n', { plain = true })) do
    vim.list_extend(lines, wrap(line, width))
  end
  return lines
end

---Where a worktree's branch stands against the local default branch: `↑2 ↓1 main` (commits
---ahead, commits behind, each only when there are some), or `up to date with main`.
---@param drift orca_term.Drift
---@return string
local function drift_text(drift)
  if drift.ahead == 0 and drift.behind == 0 then
    return 'up to date with ' .. drift.against
  end
  local parts = {}
  if drift.ahead > 0 then
    table.insert(parts, '↑' .. drift.ahead)
  end
  if drift.behind > 0 then
    table.insert(parts, '↓' .. drift.behind)
  end
  table.insert(parts, drift.against)
  return table.concat(parts, ' ')
end

---How the detail float says a git operation in progress: `merge in progress`, or
---`rebase in progress (2/5)` with a rebase's step.
---@param operation string `merge`, `rebase`, `cherry-pick` or `revert`
---@param step? orca_term.OperationStep
---@return string
function M.operation_text(operation, step)
  if step then
    return string.format('%s in progress (%d/%d)', operation, step.current, step.total)
  end
  return operation .. ' in progress'
end

---A worktree's detail, as the float `detail.lua` opens from the sidebar shows it, laid out to
---`width` cells: one cell of margin on each side, and parts apart by a blank line, each only when
---it has something to say. Nothing is ever cut: the error, the denials and the setup's log wrap.
---
---   ● failed · claude-code
---   fix  ↑2 ↓1 main
---
---   rebase in progress (2/5)
---
---   PR #12 open, checks passing (2 min ago)
---   https://github.com/o/r/pull/12
---
---   API Error: 400 the whole message, however long, wrapped to the width
---
---   Denied
---   ⊘ Write: /path/to/a.txt
---
---   Setup (failed)
---   npm ERR! missing script: build
---
---The pull request's part shows the pull request `pr refresh` last found and how long ago (`now`,
---in milliseconds since the Unix epoch, the time it is measured from; the clock's when `nil`); a
---worktree with no pull request, or none known, has none.
---@param detail orca_term.Detail
---@param width integer
---@param now? integer
---@return orca_term.Rendered
function M.detail(detail, width, now)
  now = now or os.time() * 1000
  width = math.max(width, DETAIL_SMALLEST)
  -- Rows end one cell before `width`, as in the sidebar, after one cell on the left.
  local text_width = width - 2
  local worktree = detail.worktree
  local state_group = highlights.state_group(worktree.state)
  local built = {}

  ---Adds a line of `text` in `group`, after the left margin and `indent`.
  ---@param text string
  ---@param group? string
  ---@param indent? string
  local function add_line(text, group, indent)
    local line = new_line(width - 1)
    add(line, ' ' .. (indent or ''))
    add(line, text, group)
    table.insert(built, line)
  end

  -- The state and the agent, then the branch and its drift.
  local state_line = new_line(width - 1)
  add(state_line, ' ')
  add(state_line, M.glyph(worktree.state), state_group)
  add(state_line, ' ')
  add(state_line, (worktree.state:gsub('_', ' ')), state_group)
  if worktree.agent then
    add(state_line, ' · ' .. worktree.agent, 'OrcaTermMuted')
  end
  table.insert(built, state_line)

  local branch = worktree.branch or 'detached'
  local branch_line = new_line(width - 1)
  add(branch_line, ' ')
  add(branch_line, branch)
  if detail.drift then
    local drift = drift_text(detail.drift)
    if cells(branch) + 2 + cells(drift) > text_width then
      -- No room after the branch: the drift goes on a line of its own.
      table.insert(built, branch_line)
      branch_line = new_line(width - 1)
      add(branch_line, ' ')
    else
      add(branch_line, '  ')
    end
    add(branch_line, drift, 'OrcaTermMuted')
  end
  table.insert(built, branch_line)

  -- The git operation in progress, with a rebase's step when git records it.
  if worktree.operation then
    table.insert(built, new_line(width - 1))
    add_line(M.operation_text(worktree.operation, detail.operation_step), 'OrcaTermOperation')
  end

  -- The pull request: where it stands and how long ago that was found, then its address.
  local pr = worktree.pr
  if pr and pr.number then
    table.insert(built, new_line(width - 1))
    local said = M.pr_text(pr) .. ' (' .. M.ago(pr.fetched_at, now) .. ')'
    for _, text in ipairs(wrap(said, text_width)) do
      add_line(text, M.pr_group(pr))
    end
    for _, text in ipairs(wrap(pr.url or '', text_width)) do
      add_line(text, 'OrcaTermMuted')
    end
  end

  -- Why the worktree is in its state, whole, in the state's colour.
  local why = detail.detail and wrap_lines(detail.detail, text_width) or {}
  if #why > 0 then
    table.insert(built, new_line(width - 1))
    for _, text in ipairs(why) do
      add_line(text, state_group)
    end
  end

  -- The latest turn's denials, one per call, wrapped under their text.
  local denials = detail.denials or {}
  if #denials > 0 then
    table.insert(built, new_line(width - 1))
    add_line('Denied', 'OrcaTermMuted')
    local mark = M.DENIED .. ' '
    for _, denial in ipairs(denials) do
      for index, text in ipairs(wrap(denial.text, text_width - cells(mark))) do
        local line = new_line(width - 1)
        add(line, ' ')
        if index == 1 then
          add(line, M.DENIED, 'OrcaTermDenied')
          add(line, ' ')
        else
          add(line, string.rep(' ', cells(mark)))
        end
        add(line, text)
        table.insert(built, line)
      end
    end
  end

  -- The setup: how it went, then the end of its log.
  local setup = detail.setup
  if setup then
    table.insert(built, new_line(width - 1))
    -- The core names a status in snake case (`not_approved`); a person reads words.
    add_line('Setup (' .. (setup.status:gsub('_', ' ')) .. ')', 'OrcaTermMuted')
    for _, entry in ipairs(setup.tail or {}) do
      for _, text in ipairs(wrap(entry, text_width)) do
        add_line(text, 'OrcaTermMuted')
      end
    end
  end

  return finish(built, {}, nil)
end

---The states in the order the status strip counts them: the attention order of the sidebar
---(`fleet.lua`'s ranks), what needs the person first. A state this client does not know yet comes
---after them, sorted by name.
M.STRIP_ORDER = {
  'waiting',
  'failed',
  'interrupted',
  'broken',
  'not_ready',
  'working',
  'setting_up',
  'finished',
  'idle',
  'no_agent',
}

---How many worktrees are in one state.
---@class orca_term.StateCount
---@field state string
---@field count integer

---What the status strip counts: the worktrees in each state, and how many are unseen.
---@class orca_term.Counts
---@field states orca_term.StateCount[] in `STRIP_ORDER`, then unknown states by name; no zeros
---@field unseen integer

---One item of the status strip: its text (a glyph and a count, such as `?1`) and its colour.
---@class orca_term.StripItem
---@field text string
---@field group string

---How many worktrees of `fleet` are in each state, in the strip's order and leaving out the states
---no worktree is in, and how many worktrees are unseen. No fleet counts nothing.
---@param fleet? orca_term.Fleet
---@return orca_term.Counts
function M.counts(fleet)
  local by_state = {}
  local unseen = 0
  for _, worktree in ipairs(fleet and fleet.worktrees or {}) do
    by_state[worktree.state] = (by_state[worktree.state] or 0) + 1
    if worktree.unseen == true then
      unseen = unseen + 1
    end
  end

  local states = {}
  for _, state in ipairs(M.STRIP_ORDER) do
    if by_state[state] then
      table.insert(states, { state = state, count = by_state[state] })
      by_state[state] = nil
    end
  end
  -- What is left are the states this client does not know yet.
  local unknown = vim.tbl_keys(by_state)
  table.sort(unknown)
  for _, state in ipairs(unknown) do
    table.insert(states, { state = state, count = by_state[state] })
  end
  return { states = states, unseen = unseen }
end

---The status strip of `fleet`: one item per state a worktree is in (its glyph, which never spins
---here, then its count, in the state's colour), then the unseen count after `UNSEEN`, when any
---worktree is unseen. `?1 ◐2 ●3 •2` is four items. No fleet, or no worktree, is no item.
---@param fleet? orca_term.Fleet
---@return orca_term.StripItem[]
function M.strip(fleet)
  local counts = M.counts(fleet)
  local items = {}
  for _, counted in ipairs(counts.states) do
    table.insert(items, {
      text = M.glyph(counted.state) .. counted.count,
      group = highlights.state_group(counted.state),
    })
  end
  if counts.unseen > 0 then
    table.insert(items, { text = M.UNSEEN .. counts.unseen, group = 'OrcaTermUnseen' })
  end
  return items
end

---The sidebar when the fleet could not be read: why, wrapped to the width, so the person sees the
---message and its fix where the fleet would be.
---@param message string
---@param width? integer
---@return orca_term.Rendered
function M.error(message, width)
  width = width or M.WIDTH
  local built = { header('orca-term', nil, width), new_line(width - 1) }
  -- One cell on the left, as every row, and one on the right.
  for _, text in ipairs(wrap(message, width - 2)) do
    local line = new_line(width - 1)
    add(line, ' ')
    add(line, text, 'OrcaTermError')
    table.insert(built, line)
  end
  return finish(built, {}, nil)
end

return M
