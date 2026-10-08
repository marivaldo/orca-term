-- PROTOTYPE: throwaway, never production.
-- Ticket: "Prototype the fleet view: order, layout and glyphs"
-- https://github.com/marivaldo/orca-term/issues/17
--
-- Question: how should the fleet view look in nvim so "which lane needs me now" reads at a glance?
-- Plan: three structurally different fleet views over twelve mock lanes, in a new tab of your own
-- nvim (your colorscheme, your font), switchable with ]v / [v or <Left> / <Right>.
--   A  Sidebar         a narrow split beside the code, sorted by attention, detail of the lane under the cursor
--   B  Status buffer   a fugitive-style full buffer, grouped into sections, every column visible
--   C  Strip + picker  counts always visible in the tabline, a floating picker sorted by name on demand
-- The states and marks are fixed by https://github.com/marivaldo/orca-term/issues/10. Only
-- presentation is in question. Nothing is read from disk: lanes live in memory, and `t` replays
-- scripted events so you can watch rows move.
--
-- Run from the repo root:  nvim -c 'luafile prototypes/fleet-view/fleet_view_prototype.lua'

if _G.OrcaFleetProto and _G.OrcaFleetProto.quit then _G.OrcaFleetProto.quit() end

local api = vim.api
local ns = api.nvim_create_namespace("orca_fleet_proto")
local P = { variant = 1, frame = 0, ev = 0, last = "press t to replay an event", rowmap = {} }
_G.OrcaFleetProto = P

-- Mock fleet --------------------------------------------------------------------------------------

local function mock_lanes()
  return {
    { name = "auth-refactor", agent = "claude-code", state = "working", turn = 4, ended = 3, seen = 3, age = "2m", detail = 'turn 4 running: "extract the session store"' },
    { name = "fix-flaky-ci", agent = "opencode", state = "waiting", turn = 3, ended = 2, seen = 2, age = "6m", detail = "question: keep vitest or jest config?" },
    { name = "sse-client", agent = "opencode", state = "failed", turn = 2, ended = 2, seen = 1, age = "14m", detail = "session.error ProviderAuthError: token expired" },
    { name = "docs-adr", agent = "claude-code", state = "finished", turn = 5, ended = 5, seen = 4, age = "9m", detail = "4 files changed · 2 permission denials (Bash: pnpm lint)" },
    { name = "lane-prune", agent = "claude-code", state = "finished", turn = 2, ended = 2, seen = 2, age = "1h", detail = "1 file changed" },
    { name = "nvim-rpc", agent = "claude-code", state = "working", turn = 7, ended = 6, seen = 6, age = "40s", takeover = true, detail = "you hold the session in :terminal" },
    { name = "pnpm-bump", state = "not ready", turn = 0, ended = 0, seen = 0, age = "22m", detail = "setup exited 1: ERR_PNPM_FETCH_404 left-pad@9.9.9" },
    { name = "spike-ts", state = "no agent", turn = 0, ended = 0, seen = 0, age = "3h", detail = "ready, pick an agent to start" },
    { name = "old-experiment", state = "broken", turn = 0, ended = 0, seen = 0, age = "2d", detail = "registered with git, directory gone · `lane prune` removes it" },
    { name = "turn-diff", agent = "claude-code", state = "interrupted", turn = 3, ended = 3, seen = 2, age = "31m", detail = "pid 48121 gone without an end · diff may be partial" },
    { name = "bootstrap-cli", state = "setting up", turn = 0, ended = 0, seen = 0, age = "18s", detail = "running setup: pnpm install" },
    { name = "new-idea", agent = "claude-code", state = "idle", turn = 0, ended = 0, seen = 0, age = "5m", detail = "agent attached, no turns yet" },
  }
end

local function by(name)
  for _, l in ipairs(P.lanes) do
    if l.name == name then return l end
  end
end

local EVENTS = {
  function() local l = by("auth-refactor"); l.state, l.ended, l.age, l.detail = "finished", 4, "now", "3 files changed"; return "auth-refactor finished turn 4" end,
  function() local l = by("nvim-rpc"); l.state, l.age, l.detail = "waiting", "now", "PermissionRequest: Bash(rm -rf build)"; return "nvim-rpc (takeover) asks permission" end,
  function() local l = by("bootstrap-cli"); l.state, l.age, l.detail = "no agent", "now", "setup exited 0 in 41s · pick an agent"; return "bootstrap-cli setup done" end,
  function() local l = by("fix-flaky-ci"); l.state, l.age, l.detail = "working", "now", "question answered, turn 3 resumed"; return "fix-flaky-ci question answered" end,
  function() local l = by("new-idea"); l.state, l.turn, l.age, l.detail = "working", 1, "now", "turn 1 running"; return "new-idea started turn 1" end,
  function() local l = by("sse-client"); l.state, l.turn, l.seen, l.age, l.detail = "working", 3, 2, "now", "re-prompted after login, turn 3 running"; return "sse-client re-prompted" end,
  function() local l = by("auth-refactor"); l.state, l.turn, l.age, l.detail = "working", 5, "now", "turn 5 running"; return "auth-refactor started turn 5" end,
  function() local l = by("new-idea"); l.state, l.ended, l.age, l.detail = "failed", 1, "now", "api_error_status 429 rate_limit"; return "new-idea turn 1 failed" end,
  function() local l = by("pnpm-bump"); l.state, l.age, l.detail = "setting up", "now", "lane setup re-run: pnpm install"; return "pnpm-bump setup re-run" end,
  function() local l = by("nvim-rpc"); l.state, l.age, l.detail = "working", "now", "permission granted in :terminal"; return "nvim-rpc permission granted" end,
}

-- Presentation vocabulary -------------------------------------------------------------------------

local GLYPH = {
  working = "◐", waiting = "?", finished = "✓", failed = "✗", interrupted = "↯",
  idle = "○", ["no agent"] = "·", ["not ready"] = "△", ["setting up"] = "◌", broken = "!",
}
local SPIN = { "◐", "◓", "◑", "◒" }
local STATE_ORDER = { "waiting", "failed", "interrupted", "broken", "not ready", "finished", "working", "setting up", "idle", "no agent" }

local function hlname(state) return "OrcaProto_" .. state:gsub(" ", "_") end

local function define_hl()
  local links = {
    working = "DiagnosticInfo", ["setting up"] = "DiagnosticInfo", waiting = "DiagnosticWarn",
    finished = "DiagnosticOk", failed = "DiagnosticError", interrupted = "DiagnosticError",
    idle = "Comment", ["no agent"] = "Comment", ["not ready"] = "DiagnosticWarn", broken = "ErrorMsg",
  }
  for state, link in pairs(links) do api.nvim_set_hl(0, hlname(state), { link = link }) end
  api.nvim_set_hl(0, "OrcaProtoUnseen", { link = "Special" })
  api.nvim_set_hl(0, "OrcaProtoTakeover", { link = "Identifier" })
  api.nvim_set_hl(0, "OrcaProtoHeader", { link = "Title" })
  api.nvim_set_hl(0, "OrcaProtoBold", { bold = true })
end

local function unseen(l) return l.ended > l.seen end
local function glyph(l)
  if l.state == "working" or l.state == "setting up" then return SPIN[P.frame % #SPIN + 1] end
  return GLYPH[l.state]
end
local function abbr(agent) return ({ ["claude-code"] = "cc", opencode = "oc" })[agent] or "" end

local function rank(l)
  if l.state == "waiting" then return 1 end
  if unseen(l) and (l.state == "failed" or l.state == "interrupted") then return 2 end
  if l.state == "broken" or l.state == "not ready" then return 3 end
  if unseen(l) then return 4 end
  if l.state == "working" or l.state == "setting up" then return 5 end
  if l.state == "finished" or l.state == "failed" or l.state == "interrupted" then return 6 end
  if l.state == "idle" then return 7 end
  return 8
end

local function sorted(cmp)
  local out = vim.list_extend({}, P.lanes)
  table.sort(out, cmp)
  return out
end
local function by_attention(a, b)
  if rank(a) ~= rank(b) then return rank(a) < rank(b) end
  return a.name < b.name
end
local function by_name(a, b) return a.name < b.name end

-- Rendering helpers -------------------------------------------------------------------------------

local function pad(s, w)
  local d = vim.fn.strdisplaywidth(s)
  return d >= w and s or s .. string.rep(" ", w - d)
end
local function trunc(s, w)
  if vim.fn.strdisplaywidth(s) <= w then return s end
  return vim.fn.strcharpart(s, 0, w - 1) .. "…"
end

local function row(segs, lane)
  local parts, hls, col = {}, {}, 0
  for _, s in ipairs(segs) do
    parts[#parts + 1] = s[1]
    if s[2] then hls[#hls + 1] = { col, col + #s[1], s[2] } end
    col = col + #s[1]
  end
  return { text = table.concat(parts), hls = hls, lane = lane }
end

local function paint(buf, rows)
  local lines, map = {}, {}
  for i, r in ipairs(rows) do
    lines[i] = r.text
    map[i] = r.lane
  end
  vim.bo[buf].modifiable = true
  api.nvim_buf_set_lines(buf, 0, -1, false, lines)
  vim.bo[buf].modifiable = false
  api.nvim_buf_clear_namespace(buf, ns, 0, -1)
  for i, r in ipairs(rows) do
    for _, h in ipairs(r.hls) do
      api.nvim_buf_set_extmark(buf, ns, i - 1, h[1], { end_col = h[2], hl_group = h[3] })
    end
  end
  P.rowmap[buf] = map
end

local function follow(win, buf)
  if not (win and api.nvim_win_is_valid(win)) then return end
  for i, name in pairs(P.rowmap[buf] or {}) do
    if name == P.selected then
      if api.nvim_win_get_cursor(win)[1] ~= i then api.nvim_win_set_cursor(win, { i, 0 }) end
      return
    end
  end
end

local function detail_rows(width)
  local l = by(P.selected) or P.lanes[1]
  local rule = string.rep("─", width)
  local rows = {
    row({ { rule, "Comment" } }),
    row({ { " " .. l.name, "OrcaProtoHeader" }, { l.takeover and "  ⌨ takeover" or "", "OrcaProtoTakeover" } }),
    row({ { " state  ", "Comment" }, { l.state, hlname(l.state) }, { "  · " .. l.age, "Comment" } }),
    row({ { " agent  ", "Comment" }, { l.agent or "none" }, { l.turn > 0 and ("  · turn " .. l.turn) or "", "Comment" } }),
    row({ { " branch ", "Comment" }, { l.name } }),
  }
  if unseen(l) then
    rows[#rows + 1] = row({ { " unseen ", "Comment" }, { "turn " .. l.ended .. " ended, seen up to " .. l.seen, "OrcaProtoUnseen" } })
  end
  for _, chunk in ipairs(vim.split(l.detail, " · ", { plain = true })) do
    rows[#rows + 1] = row({ { " " .. trunc(chunk, width - 1) } })
  end
  return rows
end

local function new_buf(name, ft)
  local b = api.nvim_create_buf(false, true)
  pcall(api.nvim_buf_set_name, b, name)
  vim.bo[b].filetype = ft or "orcafleetproto"
  vim.bo[b].bufhidden = "hide"
  return b
end

local function list_opts(win)
  local o = vim.wo[win]
  o.number, o.relativenumber, o.signcolumn, o.wrap = false, false, "no", false
  o.cursorline, o.foldcolumn, o.list, o.spell = true, "0", false, false
end

-- Variant A: sidebar ------------------------------------------------------------------------------

local A = { name = "Sidebar, sorted by attention" }

function A.build()
  A.buf = new_buf("orca-fleet://sidebar")
  A.win = api.nvim_open_win(A.buf, true, { split = "left", win = P.ed_win, width = 36 })
  list_opts(A.win)
  vim.wo[A.win].winfixwidth = true
  return { A.buf }
end

function A.draw(move)
  local rows = { row({ { " FLEET ", "OrcaProtoHeader" }, { "orca-term · " .. #P.lanes .. " lanes", "Comment" } }), row({ { "" } }) }
  for _, l in ipairs(sorted(by_attention)) do
    rows[#rows + 1] = row({
      { " " }, { unseen(l) and "●" or " ", "OrcaProtoUnseen" }, { " " },
      { glyph(l), hlname(l.state) }, { " " },
      { pad(trunc(l.name, 22), 22), unseen(l) and "OrcaProtoBold" or nil },
      { " " }, { pad(abbr(l.agent), 2), "Comment" },
      { l.takeover and " ⌨" or "", "OrcaProtoTakeover" },
    }, l.name)
  end
  rows[#rows + 1] = row({ { "" } })
  vim.list_extend(rows, detail_rows(35))
  paint(A.buf, rows)
  if move then follow(A.win, A.buf) end
end

function A.teardown()
  if A.win and api.nvim_win_is_valid(A.win) then api.nvim_win_close(A.win, true) end
end

-- Variant B: status buffer ------------------------------------------------------------------------

local B = { name = "Status buffer, grouped into sections" }

function B.build()
  B.buf = new_buf("orca-fleet://status")
  B.saved = {}
  for _, k in ipairs({ "number", "relativenumber", "signcolumn", "wrap", "cursorline", "foldcolumn", "list", "spell" }) do
    B.saved[k] = vim.wo[P.ed_win][k]
  end
  api.nvim_win_set_buf(P.ed_win, B.buf)
  api.nvim_set_current_win(P.ed_win)
  list_opts(P.ed_win)
  return { B.buf }
end

local function section(l)
  local r = rank(l)
  if r <= 4 then return 1 end
  if l.state == "working" or l.state == "setting up" then return 2 end
  return 3
end

function B.draw(move)
  local titles = { "Needs you", "Running", "Quiet" }
  local groups = { {}, {}, {} }
  for _, l in ipairs(sorted(by_attention)) do table.insert(groups[section(l)], l) end
  local rows = {
    row({ { "Fleet: ", "OrcaProtoHeader" }, { "orca-term" }, { "   " .. #P.lanes .. " lanes · base main", "Comment" } }),
    row({ { "⏎ open review   u toggle unseen   t replay event   ? raw state", "Comment" } }),
  }
  for gi, group in ipairs(groups) do
    rows[#rows + 1] = row({ { "" } })
    rows[#rows + 1] = row({ { titles[gi], "OrcaProtoHeader" }, { " (" .. #group .. ")", "Comment" } })
    for _, l in ipairs(group) do
      rows[#rows + 1] = row({
        { glyph(l), hlname(l.state) }, { " " },
        { unseen(l) and "●" or " ", "OrcaProtoUnseen" }, { " " },
        { pad(l.name, 16), unseen(l) and "OrcaProtoBold" or nil }, { " " },
        { pad(l.agent or "-", 12), "Comment" },
        { pad(l.state, 12), hlname(l.state) },
        { pad(l.turn > 0 and ("turn " .. l.turn) or "", 8), "Comment" },
        { pad(l.age, 5), "Comment" },
        { l.takeover and "⌨ " or "", "OrcaProtoTakeover" },
        { l.detail },
      }, l.name)
    end
  end
  paint(B.buf, rows)
  if move then follow(P.ed_win, B.buf) end
end

function B.teardown()
  if api.nvim_win_is_valid(P.ed_win) then
    api.nvim_win_set_buf(P.ed_win, P.ed_buf)
    for k, v in pairs(B.saved or {}) do vim.wo[P.ed_win][k] = v end
  end
end

-- Variant C: strip + picker -----------------------------------------------------------------------

local C = { name = "Tabline strip + floating picker, sorted by name" }

_G.OrcaFleetProtoTabline = function()
  local counts, n_unseen = {}, 0
  for _, l in ipairs(P.lanes) do
    counts[l.state] = (counts[l.state] or 0) + 1
    if unseen(l) then n_unseen = n_unseen + 1 end
  end
  local s = "%#TabLineSel# orca-term %#TabLineFill#  "
  for _, st in ipairs(STATE_ORDER) do
    if counts[st] then s = s .. "%#" .. hlname(st) .. "#" .. GLYPH[st] .. " " .. counts[st] .. "%#TabLineFill#   " end
  end
  if n_unseen > 0 then s = s .. "%#OrcaProtoUnseen#● " .. n_unseen .. " unseen" end
  return s .. "%#TabLineFill#%=%#TabLine# <Tab> lanes "
end

function C.build()
  C.saved = { showtabline = vim.o.showtabline, tabline = vim.o.tabline }
  vim.o.showtabline = 2
  vim.o.tabline = "%!v:lua.OrcaFleetProtoTabline()"
  C.buf = new_buf("orca-fleet://picker")
  C.open()
  return { C.buf }
end

function C.open()
  if C.win and api.nvim_win_is_valid(C.win) then return end
  local w, h = 74, #P.lanes + 10
  C.win = api.nvim_open_win(C.buf, true, {
    relative = "editor", width = w, height = h, style = "minimal", border = "rounded",
    row = math.floor((vim.o.lines - h) / 2) - 2, col = math.floor((vim.o.columns - w) / 2),
    title = " lanes ", title_pos = "center",
  })
  vim.wo[C.win].cursorline = true
  C.draw(true)
end

function C.close()
  if C.win and api.nvim_win_is_valid(C.win) then api.nvim_win_close(C.win, true) end
  C.win = nil
  if api.nvim_win_is_valid(P.ed_win) then api.nvim_set_current_win(P.ed_win) end
end

function C.toggle()
  if C.win and api.nvim_win_is_valid(C.win) then C.close() else C.open() end
end

function C.draw(move)
  vim.cmd.redrawtabline()
  if not (C.win and api.nvim_win_is_valid(C.win)) then return end
  local rows = {}
  for _, l in ipairs(sorted(by_name)) do
    rows[#rows + 1] = row({
      { " " }, { glyph(l), hlname(l.state) }, { " " },
      { unseen(l) and "●" or " ", "OrcaProtoUnseen" }, { " " },
      { pad(l.name, 18), unseen(l) and "OrcaProtoBold" or nil },
      { pad(l.state, 13), hlname(l.state) },
      { pad(abbr(l.agent), 4), "Comment" },
      { pad(l.age, 5), "Comment" },
      { l.takeover and "⌨" or "", "OrcaProtoTakeover" },
    }, l.name)
  end
  rows[#rows + 1] = row({ { "" } })
  vim.list_extend(rows, detail_rows(73))
  paint(C.buf, rows)
  if move then follow(C.win, C.buf) end
end

function C.teardown()
  C.close()
  if C.saved then
    vim.o.showtabline, vim.o.tabline = C.saved.showtabline, C.saved.tabline
  end
end

-- Switcher bar, actions, lifecycle ----------------------------------------------------------------

local VARIANTS = { A, B, C }
local KEYS = { "A", "B", "C" }

local function bar_text()
  return string.format("  ◀  %s · %s  ▶     t event  ⏎ review  u unseen  ? state  Q quit   │ %s  ",
    KEYS[P.variant], VARIANTS[P.variant].name, P.last)
end

local function draw_bar()
  local text = bar_text()
  local w = math.min(vim.fn.strdisplaywidth(text), vim.o.columns - 2)
  local cfg = {
    relative = "editor", width = w, height = 1, style = "minimal", focusable = false, zindex = 250,
    row = vim.o.lines - vim.o.cmdheight - 3, col = math.floor((vim.o.columns - w) / 2),
  }
  if not (P.bar_win and api.nvim_win_is_valid(P.bar_win)) then
    P.bar_buf = P.bar_buf or new_buf("orca-fleet://switcher")
    P.bar_win = api.nvim_open_win(P.bar_buf, false, cfg)
    vim.wo[P.bar_win].winhighlight = "Normal:PmenuSel"
  else
    api.nvim_win_set_config(P.bar_win, cfg)
  end
  vim.bo[P.bar_buf].modifiable = true
  api.nvim_buf_set_lines(P.bar_buf, 0, -1, false, { text })
  vim.bo[P.bar_buf].modifiable = false
end

local function draw(move)
  VARIANTS[P.variant].draw(move)
  draw_bar()
end

local function lane_at_cursor()
  local buf = api.nvim_get_current_buf()
  local map = P.rowmap[buf]
  return map and map[api.nvim_win_get_cursor(0)[1]]
end

local actions = {}

function actions.tick()
  P.ev = P.ev + 1
  if P.ev > #EVENTS then
    P.last = "script exhausted · R resets"
  else
    P.last = EVENTS[P.ev]()
  end
  draw(true)
end

function actions.reset()
  P.lanes, P.ev, P.last, P.selected = mock_lanes(), 0, "reset", "fix-flaky-ci"
  draw(true)
end

function actions.review()
  local l = by(lane_at_cursor() or P.selected)
  if not l then return end
  P.selected = l.name
  l.seen = l.ended
  P.last = "opened review of " .. l.name .. " · unseen cleared"
  draw(true)
end

function actions.toggle_unseen()
  local l = by(lane_at_cursor() or P.selected)
  if not l or l.ended == 0 then return end
  P.selected = l.name
  l.seen = unseen(l) and l.ended or l.ended - 1
  P.last = l.name .. (unseen(l) and " marked unseen" or " marked seen")
  draw(true)
end

function actions.raw()
  local dump = {}
  for _, l in ipairs(P.lanes) do
    dump[#dump + 1] = vim.tbl_extend("force", l, { unseen = unseen(l) })
  end
  local lines = vim.split("-- what the view reads per lane (lane.json + liveness), mocked\n" .. vim.inspect(dump), "\n")
  local b = new_buf("orca-fleet://raw-" .. P.ev, "lua")
  vim.bo[b].bufhidden = "wipe"
  api.nvim_buf_set_lines(b, 0, -1, false, lines)
  local w, h = math.min(100, vim.o.columns - 6), math.min(#lines, vim.o.lines - 8)
  api.nvim_open_win(b, true, {
    relative = "editor", width = w, height = h, border = "rounded", title = " raw state ",
    row = 2, col = math.floor((vim.o.columns - w) / 2),
  })
  for _, k in ipairs({ "q", "<Esc>" }) do
    vim.keymap.set("n", k, "<cmd>close<cr>", { buffer = b, nowait = true })
  end
end

local switch

local function map_buf(buf)
  local m = function(lhs, fn) vim.keymap.set("n", lhs, fn, { buffer = buf, nowait = true }) end
  m("]v", function() switch(1) end)
  m("[v", function() switch(-1) end)
  m("<Right>", function() switch(1) end)
  m("<Left>", function() switch(-1) end)
  m("t", actions.tick)
  m("R", actions.reset)
  m("<CR>", actions.review)
  m("u", actions.toggle_unseen)
  m("?", actions.raw)
  m("Q", function() P.quit() end)
  m("<Tab>", function() if P.variant == 3 then C.toggle() end end)
  if buf == C.buf then m("q", C.close) end
end

local function build()
  local bufs = VARIANTS[P.variant].build()
  table.insert(bufs, P.ed_buf)
  P.group = api.nvim_create_augroup("OrcaFleetProto", { clear = true })
  for _, b in ipairs(bufs) do
    map_buf(b)
    if b ~= P.ed_buf then
      api.nvim_create_autocmd("CursorMoved", {
        group = P.group, buffer = b,
        callback = function()
          local name = lane_at_cursor()
          if name and name ~= P.selected then
            P.selected = name
            draw(false)
          end
        end,
      })
    end
  end
  api.nvim_create_autocmd({ "VimResized", "ColorScheme" }, {
    group = P.group,
    callback = function() define_hl(); draw(false) end,
  })
  draw(true)
end

switch = function(step)
  VARIANTS[P.variant].teardown()
  P.variant = (P.variant - 1 + step) % #VARIANTS + 1
  build()
end

function P.quit()
  if P.timer then P.timer:stop(); P.timer:close(); P.timer = nil end
  pcall(VARIANTS[P.variant].teardown)
  pcall(api.nvim_del_augroup_by_name, "OrcaFleetProto")
  if P.bar_win and api.nvim_win_is_valid(P.bar_win) then api.nvim_win_close(P.bar_win, true) end
  if P.tab and api.nvim_tabpage_is_valid(P.tab) and #api.nvim_list_tabpages() > 1 then
    api.nvim_set_current_tabpage(P.tab)
    vim.cmd.tabclose()
  end
  for _, b in ipairs({ A.buf, B.buf, C.buf, P.bar_buf, P.ed_buf }) do
    if b and api.nvim_buf_is_valid(b) then pcall(api.nvim_buf_delete, b, { force = true }) end
  end
  _G.OrcaFleetProto = nil
end

-- Start -------------------------------------------------------------------------------------------

define_hl()
P.lanes, P.selected = mock_lanes(), "fix-flaky-ci"

vim.cmd.tabnew()
P.tab, P.ed_win = api.nvim_get_current_tabpage(), api.nvim_get_current_win()
P.ed_buf = new_buf("orca-fleet://editor (mock code buffer)", "markdown")
local ok, readme = pcall(vim.fn.readfile, "CONTEXT.md")
api.nvim_buf_set_lines(P.ed_buf, 0, -1, false, ok and readme or { "# your code would be here" })
api.nvim_win_set_buf(P.ed_win, P.ed_buf)

build()

P.timer = vim.uv.new_timer()
P.timer:start(200, 200, vim.schedule_wrap(function()
  if not _G.OrcaFleetProto then return end
  P.frame = P.frame + 1
  pcall(draw, false)
end))
