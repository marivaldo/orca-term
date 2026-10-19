-- orca-term's sidebar: what it shows, as lines of text and the colours on them.
--
-- Pure: it takes the fleet (or an error) and returns lines, highlight spans and which worktree
-- each line belongs to. It never touches a buffer or a window; `sidebar.lua` paints the result.
-- It also counts the fleet for the status strip (`M.strip`, `M.counts`), which `strip.lua` and the
-- lualine component draw.
-- The only Neovim calls measure text in screen cells (`vim.fn.strdisplaywidth`,
-- `vim.fn.strcharpart`, `vim.fn.strchars`) or are pure helpers (`vim.fs.basename`,
-- `vim.list_extend`, `vim.tbl_map`).
--
-- The layout is prototype variant D (branch `prototype/fleet-sidebar`, spec issue #25). This is
-- spec/golden/sidebar.txt, with `pick-colour` selected; `•` marks the unseen worktrees, and `✎N`
-- counts a worktree's pending notes:
--
--   ▾ repo  main
--
--   NEEDS YOU 7
--   ╭────────────────────────────────╮
--   │ ? pick-colour               ✎1 │
--   │   Which colour?                │
--   ╰────────────────────────────────╯
--  •● api-call
--     API Error: 400 fake bad request
--  •● slow-job
--     SLOW please
--   △ fix-login
--     setup failed (exit 1): npm ERR!…
--   ! old-spike
--     directory missing; run orca-ter…
--  •● greet
--     ok
--  •○ nudged
--
--   RUNNING 2
--   ◌ install-deps
--   ◐ run-tests
--     Bash: sleep 6
--
--   QUIET 4
--   ● reviewed
--     ok
--   ○ standby
--   · docs                          ✎2
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

---The icons at the right of a worktree's name line, from left to right: `✎N` for its pending
---notes. Empty when it has nothing to show.
---@param worktree orca_term.Worktree
---@return { text: string, group: string }[]
local function icons(worktree)
  local found = {}
  if (worktree.notes or 0) > 0 then
    table.insert(found, { text = M.NOTE .. worktree.notes, group = 'OrcaTermBadge' })
  end
  return found
end

---Adds `name` to `line`, in `group` when given, then `icons` flushed to the right end of the line's
---budget, a space between them. The name is cut with `…` to leave the icons room, and a space
---before them.
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
---@return orca_term.Line[]
local function worktree_lines(worktree, needs_you, budget, frame)
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
  -- The icons sit at the right of this line; the agent and the age of the last activity will join
  -- them.
  add_name(name_line, worktree.name, unseen and 'OrcaTermUnseenName' or nil, icons(worktree))
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

---The sidebar for `fleet`, with the worktree at path `selected` in the card. When no worktree
---has that path (it is `nil`, or the worktree is gone), the first worktree shown is selected.
---A working worktree's glyph is the spinner's frame `frame`.
---@param fleet orca_term.Fleet
---@param selected? string
---@param width? integer
---@param frame? integer 0 or more; `nil` is frame 0
---@return orca_term.Rendered
function M.fleet(fleet, selected, width, frame)
  width = width or M.WIDTH
  frame = frame or 0
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
        local body = worktree_lines(worktree, needs_you, width - 5, frame)
        add_card(built, body, worktree.path, width)
      else
        vim.list_extend(built, worktree_lines(worktree, needs_you, width - 1, frame))
      end
    end
  end
  return finish(built, order, selected)
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
