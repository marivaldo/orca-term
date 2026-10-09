-- PROTOTYPE: throwaway, never production.
-- Ticket: "Prototype the fleet view as an Orca-style sidebar"
-- https://github.com/marivaldo/orca-term/issues/19
--
-- Question: what does the fleet view look like as an Orca-style sidebar (row, order, primary
-- checkout, right-hand icons, selection)?
-- Plan: three structurally different sidebars over the same mock fleet, in a new tab of your own
-- nvim, switchable with ]v / [v or <Left> / <Right>. Each copies Orca's Projects sidebar
-- (stablyai/orca @ ad7af63b, legacy "detailed" card, 280px ~ 36 columns) and differs on one axis:
--   A  Orca literal      Orca's default: "Recent" order, primary checkout first with a `primary`
--                        badge, line 2 = branch + icons, an agent sub-row only when it has
--                        something to say, unseen = amber bell replacing the glyph, selected row
--                        drawn as a rounded card
--   B  Attention         Orca's "Agent Activity" sort with #17's three sections. Primary checkout
--                        only in the header. Line 2 = what the lane is doing (question, error,
--                        diff stat); branch shown only when it differs from the lane name.
--                        Selected row = a background wash
--   C  Compact + card    one line per lane (Orca's compact cards), attention order without
--                        headings; the selected lane expands into a full card with every detail
-- Closing the sidebar (S) leaves #17's strip of counts in the tabline. States and marks come from
-- #10, denials from #18, pending notes from #11. Nothing is read from disk; `t` replays events.
--
-- Run from the repo root:  nvim -c 'luafile prototypes/fleet-sidebar/fleet_sidebar_prototype.lua'

if _G.OrcaSideProto and _G.OrcaSideProto.quit then _G.OrcaSideProto.quit() end

local api = vim.api
local ns = api.nvim_create_namespace("orca_side_proto")
local P = { variant = 1, frame = 0, ev = 0, last = "press t to replay an event", rowmap = {}, firsts = {}, open = true }
_G.OrcaSideProto = P

local WIDTH = 36

-- Mock fleet --------------------------------------------------------------------------------------
-- act = minutes since last activity (Orca's "Recent" key). branch differs from name only where
-- `git switch` moved it. pr = mocked PR state, notes = pending notes (#11), denied = denials (#18).

local PRIMARY = { name = "orca-term", branch = "main", path = "~/Projects/own/orca-term" }

local function mock_lanes()
  return {
    { name = "auth-refactor", branch = "auth-refactor", agent = "claude-code", state = "working", turn = 4, ended = 3, seen = 3, act = 2, detail = 'turn 4 running · "extract the session store"' },
    { name = "fix-flaky-ci", branch = "fix-flaky-ci", agent = "opencode", state = "waiting", turn = 3, ended = 2, seen = 2, act = 6, detail = "question: keep vitest or jest config?", pr = "open-pending" },
    { name = "sse-client", branch = "sse-client", agent = "opencode", state = "failed", turn = 2, ended = 2, seen = 1, act = 14, detail = "ProviderAuthError: token expired" },
    { name = "docs-adr", branch = "docs-adr", agent = "claude-code", state = "finished", turn = 5, ended = 5, seen = 4, act = 9, detail = "4 files changed", denied = 2, notes = 3, pr = "open-failing" },
    { name = "lane-prune", branch = "lane-prune", agent = "claude-code", state = "finished", turn = 2, ended = 2, seen = 2, act = 60, detail = "1 file changed", pr = "merged" },
    { name = "nvim-rpc", branch = "nvim-rpc", agent = "claude-code", state = "working", turn = 7, ended = 6, seen = 6, act = 1, takeover = true, detail = "you hold the session in :terminal" },
    { name = "pnpm-bump", branch = "pnpm-bump", state = "not ready", turn = 0, ended = 0, seen = 0, act = 22, detail = "setup exited 1: ERR_PNPM_FETCH_404 left-pad@9.9.9" },
    { name = "spike-ts", branch = "feat/ts-strict-mode", state = "no agent", turn = 0, ended = 0, seen = 0, act = 180, detail = "ready, pick an agent to start" },
    { name = "old-experiment", branch = "old-experiment", state = "broken", turn = 0, ended = 0, seen = 0, act = 2880, detail = "directory gone · `lane prune` removes it" },
    { name = "turn-diff", branch = "turn-diff", agent = "claude-code", state = "interrupted", turn = 3, ended = 3, seen = 2, act = 31, detail = "turn ended without an end snapshot" },
    { name = "bootstrap-cli", branch = "bootstrap-cli", state = "setting up", turn = 0, ended = 0, seen = 0, act = 0, detail = "running setup: pnpm install" },
    { name = "new-idea", branch = "new-idea", agent = "claude-code", state = "idle", turn = 0, ended = 0, seen = 0, act = 5, detail = "agent attached, no turns yet" },
  }
end

local function by(name)
  for _, l in ipairs(P.lanes) do
    if l.name == name then return l end
  end
end

local EVENTS = {
  function() local l = by("auth-refactor"); l.state, l.ended, l.act, l.detail = "finished", 4, 0, "3 files changed"; return "auth-refactor finished turn 4" end,
  function() local l = by("nvim-rpc"); l.state, l.act, l.detail = "waiting", 0, "permission: Bash(rm -rf build)"; return "nvim-rpc (takeover) asks permission" end,
  function() local l = by("bootstrap-cli"); l.state, l.act, l.detail = "no agent", 0, "setup exited 0 in 41s · pick an agent"; return "bootstrap-cli setup done" end,
  function() local l = by("fix-flaky-ci"); l.state, l.act, l.detail = "working", 0, "question answered, turn 3 resumed"; return "fix-flaky-ci question answered" end,
  function() local l = by("new-idea"); l.state, l.turn, l.act, l.detail = "working", 1, 0, "turn 1 running"; return "new-idea started turn 1" end,
  function() local l = by("new-idea"); l.state, l.ended, l.act, l.detail, l.denied = "finished", 1, 0, "2 files changed", 1; return "new-idea finished with 1 denial" end,
  function() local l = by("spike-ts"); l.agent, l.state, l.act, l.detail = "opencode", "idle", 0, "agent attached, no turns yet"; return "spike-ts got opencode" end,
  function() local l = by("nvim-rpc"); l.state, l.act, l.detail = "working", 0, "permission granted in :terminal"; return "nvim-rpc permission granted" end,
}

-- Presentation vocabulary -------------------------------------------------------------------------
-- Glyphs follow Orca's StatusIndicator: spinner = working, ? = needs you, red dot = failed,
-- green = done. Colours link to Diagnostic*, as #17 fixed.

local GLYPH = {
  working = "◐", waiting = "?", finished = "●", failed = "●", interrupted = "●",
  idle = "○", ["no agent"] = "·", ["not ready"] = "△", ["setting up"] = "◌", broken = "!",
}
local SPIN = { "◐", "◓", "◑", "◒" }
local STATE_ORDER = { "waiting", "failed", "interrupted", "broken", "not ready", "finished", "working", "setting up", "idle", "no agent" }

local function hlname(state) return "OrcaSide_" .. state:gsub(" ", "_") end

local function define_hl()
  local links = {
    working = "DiagnosticWarn", ["setting up"] = "DiagnosticInfo", waiting = "DiagnosticWarn",
    finished = "DiagnosticOk", failed = "DiagnosticError", interrupted = "Comment",
    idle = "Comment", ["no agent"] = "Comment", ["not ready"] = "DiagnosticWarn", broken = "ErrorMsg",
  }
  for state, link in pairs(links) do api.nvim_set_hl(0, hlname(state), { link = link }) end
  api.nvim_set_hl(0, "OrcaSideUnseen", { link = "DiagnosticWarn" })
  api.nvim_set_hl(0, "OrcaSideTakeover", { link = "Identifier" })
  api.nvim_set_hl(0, "OrcaSideHeader", { link = "Title" })
  api.nvim_set_hl(0, "OrcaSideBold", { bold = true })
  api.nvim_set_hl(0, "OrcaSideMuted", { link = "Comment" })
  api.nvim_set_hl(0, "OrcaSideBadge", { link = "DiagnosticHint" })
  api.nvim_set_hl(0, "OrcaSideCard", { link = "CursorLine" })
  api.nvim_set_hl(0, "OrcaSideBorder", { link = "FloatBorder" })
  api.nvim_set_hl(0, "OrcaSidePrOpen", { link = "DiagnosticOk" })
  api.nvim_set_hl(0, "OrcaSidePrFail", { link = "DiagnosticError" })
  api.nvim_set_hl(0, "OrcaSidePrPend", { link = "DiagnosticWarn" })
  api.nvim_set_hl(0, "OrcaSidePrMerged", { link = "Constant" })
end

local function unseen(l) return l.ended > l.seen end
local function glyph(l)
  if l.state == "working" or l.state == "setting up" then return SPIN[P.frame % #SPIN + 1] end
  return GLYPH[l.state]
end
local function abbr(agent) return ({ ["claude-code"] = "cc", opencode = "oc" })[agent] or "" end
local function ago(m)
  if m < 1 then return "now" end
  if m < 60 then return m .. "m" end
  if m < 1440 then return math.floor(m / 60) .. "h" end
  return math.floor(m / 1440) .. "d"
end

-- Attention rank: Orca's smart-attention classes (needs you < done < working < idle), with #10's
-- broken / not ready lifted into "needs you" because nothing else will fix them.
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
local function section(l)
  local r = rank(l)
  if r <= 4 then return 1 end
  if r == 5 then return 2 end
  return 3
end

local function sorted(cmp)
  local out = vim.list_extend({}, P.lanes)
  table.sort(out, cmp)
  return out
end
local function by_attention(a, b)
  if rank(a) ~= rank(b) then return rank(a) < rank(b) end
  if a.act ~= b.act then return a.act < b.act end
  return a.name < b.name
end
local function by_recent(a, b)
  if a.act ~= b.act then return a.act < b.act end
  return a.name < b.name
end

-- Right-hand icons: Orca's PR icon (CI recolours it while open) and notes icon have equivalents
-- here; ports, issues and automation do not. Ours add takeover, denials and the agent.
local function icons(l)
  local out = {}
  if l.takeover then out[#out + 1] = { "⌨", "OrcaSideTakeover" } end
  if l.denied then out[#out + 1] = { "⊘" .. l.denied, "DiagnosticWarn" } end
  if l.notes then out[#out + 1] = { "✎" .. l.notes, "OrcaSideBadge" } end
  if l.pr then
    local hl = ({ ["open-pending"] = "OrcaSidePrPend", ["open-failing"] = "OrcaSidePrFail", merged = "OrcaSidePrMerged" })[l.pr] or "OrcaSidePrOpen"
    out[#out + 1] = { "PR", hl }
  end
  return out
end

-- Rendering helpers -------------------------------------------------------------------------------

local function dw(s) return vim.fn.strdisplaywidth(s) end
local function pad(s, w) return dw(s) >= w and s or s .. string.rep(" ", w - dw(s)) end
local function trunc(s, w)
  if w <= 0 then return "" end
  if dw(s) <= w then return s end
  return vim.fn.strcharpart(s, 0, w - 1) .. "…"
end

-- A row is a list of segments {text, hl}; `right` segments are flushed to the right edge.
local function row(segs, lane, right, opts)
  opts = opts or {}
  local width = opts.width or WIDTH
  local parts, hls, col = {}, {}, 0
  local function add(s)
    parts[#parts + 1] = s[1]
    if s[2] then hls[#hls + 1] = { col, col + #s[1], s[2] } end
    col = col + #s[1]
  end
  for _, s in ipairs(segs) do add(s) end
  if right and #right > 0 then
    local rtext = {}
    for i, s in ipairs(right) do rtext[#rtext + 1] = (i > 1 and " " or "") .. s[1] end
    local gap = width - dw(table.concat(parts)) - dw(table.concat(rtext)) - 1
    add({ string.rep(" ", math.max(gap, 1)) })
    for i, s in ipairs(right) do
      if i > 1 then add({ " " }) end
      add(s)
    end
  end
  return { text = table.concat(parts), hls = hls, lane = lane, card = opts.card, first = opts.first }
end

local function paint(buf, rows)
  local lines, map, firsts = {}, {}, {}
  for i, r in ipairs(rows) do
    lines[i] = r.text
    map[i] = r.lane
    if r.first then firsts[#firsts + 1] = i end
  end
  vim.bo[buf].modifiable = true
  api.nvim_buf_set_lines(buf, 0, -1, false, lines)
  vim.bo[buf].modifiable = false
  api.nvim_buf_clear_namespace(buf, ns, 0, -1)
  for i, r in ipairs(rows) do
    if r.card then api.nvim_buf_set_extmark(buf, ns, i - 1, 0, { line_hl_group = "OrcaSideCard", priority = 10 }) end
    for _, h in ipairs(r.hls) do
      if h[1] < #r.text then
        api.nvim_buf_set_extmark(buf, ns, i - 1, h[1], { end_col = math.min(h[2], #r.text), hl_group = h[3], priority = 20 })
      end
    end
  end
  P.rowmap[buf], P.firsts[buf] = map, firsts
end

local function follow(win, buf)
  if not (win and api.nvim_win_is_valid(win)) then return end
  for _, i in ipairs(P.firsts[buf] or {}) do
    if P.rowmap[buf][i] == P.selected then
      if api.nvim_win_get_cursor(win)[1] ~= i then api.nvim_win_set_cursor(win, { i, 0 }) end
      return
    end
  end
end

local function status_slot(l)
  -- Orca: an unread worktree's dot is replaced by an amber bell (✦ here, no Nerd Font assumed). Kept for A; B and C keep the
  -- state glyph and mark unseen beside it, so the state never hides.
  return { glyph(l), hlname(l.state) }
end

local function name_seg(l, w)
  return { trunc(l.name, w), unseen(l) and "OrcaSideBold" or nil }
end

-- Variant A: Orca literal -------------------------------------------------------------------------

local A = { name = "Orca literal: recent order, primary first, branch on line 2" }

function A.rows()
  local rows = {
    row({ { " ▾ ", "OrcaSideMuted" }, { PRIMARY.name, "OrcaSideHeader" } }, nil, { { "+", "OrcaSideMuted" }, { "…", "OrcaSideMuted" } }),
  }
  -- the primary checkout: Orca's `primary` badge, no state glyph, since it is not a lane
  rows[#rows + 1] = row({ { "   " }, { PRIMARY.name, "OrcaSideMuted" }, { "  " }, { "primary", "OrcaSideBadge" } })
  rows[#rows + 1] = row({ { "   " }, { PRIMARY.branch, "OrcaSideMuted" } })
  for _, l in ipairs(sorted(by_recent)) do
    local sel = l.name == P.selected
    local slot = unseen(l) and { "✦", "OrcaSideUnseen" } or status_slot(l)
    local body, bw = {}, sel and WIDTH - 3 or WIDTH
    body[#body + 1] = row({ { " " }, slot, { " " }, name_seg(l, bw - 10) }, l.name, { { ago(l.act), "OrcaSideMuted" } }, { first = true, width = bw })
    body[#body + 1] = row({ { "   " }, { trunc(l.branch, bw - 12), "OrcaSideMuted" } }, l.name, icons(l), { width = bw })
    -- Orca's inline agent row: state dot, agent, "primary - secondary". Only when the lane has
    -- something to say that line 2 cannot carry.
    if l.state == "waiting" or l.state == "failed" or l.state == "not ready" or l.state == "broken" or l.state == "interrupted" then
      body[#body + 1] = row({ { "   " }, { glyph(l), hlname(l.state) }, { " " }, { abbr(l.agent) ~= "" and (abbr(l.agent) .. " - ") or "", "OrcaSideMuted" }, { trunc(l.detail, bw - 11), hlname(l.state) } }, l.name)
    end
    if sel then
      -- rounded card around the selected row, as Orca draws the active worktree
      rows[#rows + 1] = row({ { " ╭" .. string.rep("─", WIDTH - 4) .. "╮", "OrcaSideBorder" } }, l.name)
      for _, b in ipairs(body) do
        local inner = vim.fn.strcharpart(b.text, 1)
        local t = " │" .. pad(inner, WIDTH - 4) .. "│"
        local shifted = {}
        for _, h in ipairs(b.hls) do shifted[#shifted + 1] = { h[1] - 1 + #" │", h[2] - 1 + #" │", h[3] } end
        shifted[#shifted + 1] = { 0, #" │", "OrcaSideBorder" }
        shifted[#shifted + 1] = { #t - #"│", #t, "OrcaSideBorder" }
        rows[#rows + 1] = { text = t, hls = shifted, lane = l.name, first = b.first }
      end
      rows[#rows + 1] = row({ { " ╰" .. string.rep("─", WIDTH - 4) .. "╯", "OrcaSideBorder" } }, l.name)
    else
      vim.list_extend(rows, body)
    end
  end
  return rows
end

-- Variant B: attention sections -------------------------------------------------------------------

local B = { name = "Attention: smart order in sections, detail on line 2" }

function B.rows()
  local rows = {
    row({ { " ▾ ", "OrcaSideMuted" }, { PRIMARY.name, "OrcaSideHeader" }, { "  " .. PRIMARY.branch, "OrcaSideMuted" } }, nil, { { "+", "OrcaSideMuted" }, { "…", "OrcaSideMuted" } }),
  }
  local titles = { "Needs you", "Running", "Quiet" }
  local groups = { {}, {}, {} }
  for _, l in ipairs(sorted(by_attention)) do table.insert(groups[section(l)], l) end
  for gi, group in ipairs(groups) do
    if #group > 0 then
      rows[#rows + 1] = row({ { "" } })
      rows[#rows + 1] = row({ { " " .. titles[gi]:upper(), "OrcaSideMuted" }, { " " .. #group, "OrcaSideMuted" } })
      for _, l in ipairs(group) do
        local sel = l.name == P.selected
        local mark = unseen(l) and { "•", "OrcaSideUnseen" } or { " " }
        local line2 = l.detail
        if l.branch ~= l.name then line2 = "⎇ " .. l.branch .. " · " .. line2 end
        rows[#rows + 1] = row({ mark, status_slot(l), { " " }, name_seg(l, WIDTH - 12) }, l.name,
          { { abbr(l.agent), "OrcaSideMuted" }, { ago(l.act), "OrcaSideMuted" } }, { first = true, card = sel })
        local ic = icons(l)
        local icw = 0
        for _, s in ipairs(ic) do icw = icw + dw(s[1]) + 1 end
        rows[#rows + 1] = row({ { "   " }, { trunc(line2, WIDTH - 4 - icw), section(l) == 1 and hlname(l.state) or "OrcaSideMuted" } }, l.name, ic, { card = sel })
      end
    end
  end
  return rows
end

-- Variant C: compact rows, selected expands -------------------------------------------------------

local C = { name = "Compact: one line each, the selected lane expands to a card" }

function C.rows()
  local rows = {
    row({ { " ▾ ", "OrcaSideMuted" }, { PRIMARY.name, "OrcaSideHeader" } }, nil, { { "+", "OrcaSideMuted" }, { "…", "OrcaSideMuted" } }),
    row({ { "   " }, { PRIMARY.branch, "OrcaSideMuted" }, { "  primary · not a lane", "OrcaSideMuted" } }),
    row({ { "" } }),
  }
  for _, l in ipairs(sorted(by_attention)) do
    local sel = l.name == P.selected
    local mark = unseen(l) and { "•", "OrcaSideUnseen" } or { " " }
    local right = icons(l)
    table.insert(right, { ago(l.act), "OrcaSideMuted" })
    rows[#rows + 1] = row({ mark, status_slot(l), { " " }, name_seg(l, WIDTH - 14) }, l.name, right, { first = true, card = sel })
    if sel then
      local function kv(k, v, hl)
        rows[#rows + 1] = row({ { "   " .. pad(k, 7), "OrcaSideMuted" }, { trunc(v, WIDTH - 11), hl } }, l.name, nil, { card = true })
      end
      kv("state", l.state .. (l.takeover and " · takeover" or ""), hlname(l.state))
      kv("agent", (l.agent or "none") .. (l.turn > 0 and (" · turn " .. l.turn) or ""))
      kv("branch", l.branch)
      if unseen(l) then kv("unseen", "turn " .. l.ended .. " not reviewed", "OrcaSideUnseen") end
      for i, chunk in ipairs(vim.split(l.detail, " · ", { plain = true })) do
        kv(i == 1 and "detail" or "", chunk, section(l) == 1 and hlname(l.state) or nil)
      end
      if l.denied then kv("denied", l.denied .. " · Bash(pnpm lint)…", "DiagnosticWarn") end
      if l.notes then kv("notes", l.notes .. " pending", "OrcaSideBadge") end
    end
  end
  return rows
end

-- Strip of counts (#17), shown in the tabline while the sidebar is closed -------------------------

_G.OrcaSideProtoTabline = function()
  local counts, n_unseen = {}, 0
  for _, l in ipairs(P.lanes) do
    counts[l.state] = (counts[l.state] or 0) + 1
    if unseen(l) then n_unseen = n_unseen + 1 end
  end
  local s = "%#TabLineSel# orca-term %#TabLineFill#  "
  for _, st in ipairs(STATE_ORDER) do
    if counts[st] then s = s .. "%#" .. hlname(st) .. "#" .. (GLYPH[st]) .. " " .. counts[st] .. "%#TabLineFill#   " end
  end
  if n_unseen > 0 then s = s .. "%#OrcaSideUnseen#• " .. n_unseen .. " unseen" end
  return s .. "%#TabLineFill#%=%#TabLine# S sidebar "
end

-- Switcher bar, sidebar window, actions, lifecycle ------------------------------------------------

local VARIANTS = { A, B, C }
local KEYS = { "A", "B", "C" }

local function new_buf(name, ft)
  local b = api.nvim_create_buf(false, true)
  pcall(api.nvim_buf_set_name, b, name)
  vim.bo[b].filetype = ft or "orcasideproto"
  vim.bo[b].bufhidden = "hide"
  return b
end

local function bar_text()
  return string.format("  ◀  %s · %s  ▶   t event  j/k lane  ⏎ review  u unseen  S sidebar  ? state  Q quit  │ %s  ",
    KEYS[P.variant], VARIANTS[P.variant].name, P.last)
end

local function draw_bar()
  local text = bar_text()
  local w = math.min(dw(text), vim.o.columns - 2)
  local cfg = {
    relative = "editor", width = w, height = 1, style = "minimal", focusable = false, zindex = 250,
    row = vim.o.lines - vim.o.cmdheight - 3, col = math.floor((vim.o.columns - w) / 2),
  }
  if not (P.bar_win and api.nvim_win_is_valid(P.bar_win)) then
    P.bar_buf = P.bar_buf or new_buf("orca-side://switcher")
    P.bar_win = api.nvim_open_win(P.bar_buf, false, cfg)
    vim.wo[P.bar_win].winhighlight = "Normal:PmenuSel"
  else
    api.nvim_win_set_config(P.bar_win, cfg)
  end
  vim.bo[P.bar_buf].modifiable = true
  api.nvim_buf_set_lines(P.bar_buf, 0, -1, false, { text })
  vim.bo[P.bar_buf].modifiable = false
end

local function side_open()
  if P.side_win and api.nvim_win_is_valid(P.side_win) then return end
  P.side_win = api.nvim_open_win(P.side_buf, true, { split = "left", win = P.ed_win, width = WIDTH })
  local o = vim.wo[P.side_win]
  o.number, o.relativenumber, o.signcolumn, o.wrap, o.cursorline = false, false, "no", false, false
  o.foldcolumn, o.list, o.spell, o.winfixwidth = "0", false, false, true
  vim.o.showtabline = 1
end

local function side_close()
  if P.side_win and api.nvim_win_is_valid(P.side_win) then api.nvim_win_close(P.side_win, true) end
  P.side_win = nil
  vim.o.showtabline = 2
  vim.o.tabline = "%!v:lua.OrcaSideProtoTabline()"
  if api.nvim_win_is_valid(P.ed_win) then api.nvim_set_current_win(P.ed_win) end
end

local function draw(move)
  paint(P.side_buf, VARIANTS[P.variant].rows())
  if move then follow(P.side_win, P.side_buf) end
  vim.cmd.redrawtabline()
  draw_bar()
end

local function lane_at_cursor()
  local map = P.rowmap[api.nvim_get_current_buf()]
  return map and map[api.nvim_win_get_cursor(0)[1]]
end

local actions = {}

function actions.step(dir)
  -- j/k move a whole lane, landing on its first line, as Orca's arrows move a whole card
  local order = {}
  for _, i in ipairs(P.firsts[P.side_buf] or {}) do order[#order + 1] = P.rowmap[P.side_buf][i] end
  local idx = 1
  for i, n in ipairs(order) do if n == P.selected then idx = i end end
  idx = math.max(1, math.min(#order, idx + dir))
  P.selected = order[idx]
  draw(true)
end

function actions.tick()
  P.ev = P.ev + 1
  P.last = P.ev > #EVENTS and "script exhausted · R resets" or EVENTS[P.ev]()
  draw(true)
end

function actions.reset()
  P.lanes, P.ev, P.last, P.selected = mock_lanes(), 0, "reset", "fix-flaky-ci"
  draw(true)
end

function actions.review()
  local l = by(lane_at_cursor() or P.selected)
  if not l then return end
  P.selected, l.seen = l.name, l.ended
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

function actions.toggle_side()
  if P.side_win and api.nvim_win_is_valid(P.side_win) then
    side_close()
    P.last = "sidebar closed · strip in the tabline"
  else
    side_open()
    P.last = "sidebar open"
  end
  draw(true)
end

function actions.raw()
  local dump = {}
  for _, l in ipairs(P.lanes) do dump[#dump + 1] = vim.tbl_extend("force", l, { unseen = unseen(l), rank = rank(l) }) end
  local lines = vim.split("-- what the view reads per lane (lane.json + turn records + liveness), mocked\n" .. vim.inspect({ primary = PRIMARY, lanes = dump }), "\n")
  local b = new_buf("orca-side://raw-" .. P.ev, "lua")
  vim.bo[b].bufhidden = "wipe"
  api.nvim_buf_set_lines(b, 0, -1, false, lines)
  local w, h = math.min(100, vim.o.columns - 6), math.min(#lines, vim.o.lines - 8)
  api.nvim_open_win(b, true, { relative = "editor", width = w, height = h, border = "rounded", title = " raw state ", row = 2, col = math.floor((vim.o.columns - w) / 2) })
  for _, k in ipairs({ "q", "<Esc>" }) do vim.keymap.set("n", k, "<cmd>close<cr>", { buffer = b, nowait = true }) end
end

local function switch(step)
  P.variant = (P.variant - 1 + step) % #VARIANTS + 1
  draw(true)
end

local function map_buf(buf)
  local m = function(lhs, fn) vim.keymap.set("n", lhs, fn, { buffer = buf, nowait = true }) end
  m("]v", function() switch(1) end)
  m("[v", function() switch(-1) end)
  m("<Right>", function() switch(1) end)
  m("<Left>", function() switch(-1) end)
  m("t", actions.tick)
  m("R", actions.reset)
  m("S", actions.toggle_side)
  m("?", actions.raw)
  m("Q", function() P.quit() end)
  if buf == P.side_buf then
    m("j", function() actions.step(1) end)
    m("k", function() actions.step(-1) end)
    m("<Down>", function() actions.step(1) end)
    m("<Up>", function() actions.step(-1) end)
    m("<CR>", actions.review)
    m("u", actions.toggle_unseen)
  end
end

function P.quit()
  if P.timer then P.timer:stop(); P.timer:close(); P.timer = nil end
  pcall(api.nvim_del_augroup_by_name, "OrcaSideProto")
  if P.saved then vim.o.showtabline, vim.o.tabline = P.saved.showtabline, P.saved.tabline end
  if P.bar_win and api.nvim_win_is_valid(P.bar_win) then api.nvim_win_close(P.bar_win, true) end
  if P.tab and api.nvim_tabpage_is_valid(P.tab) and #api.nvim_list_tabpages() > 1 then
    api.nvim_set_current_tabpage(P.tab)
    vim.cmd.tabclose()
  end
  for _, b in ipairs({ P.side_buf, P.bar_buf, P.ed_buf }) do
    if b and api.nvim_buf_is_valid(b) then pcall(api.nvim_buf_delete, b, { force = true }) end
  end
  _G.OrcaSideProto = nil
end

-- Start -------------------------------------------------------------------------------------------

define_hl()
P.lanes, P.selected = mock_lanes(), "fix-flaky-ci"
P.saved = { showtabline = vim.o.showtabline, tabline = vim.o.tabline }

vim.cmd.tabnew()
P.tab, P.ed_win = api.nvim_get_current_tabpage(), api.nvim_get_current_win()
P.ed_buf = new_buf("orca-side://editor (mock code buffer)", "markdown")
local ok, ctx = pcall(vim.fn.readfile, "CONTEXT.md")
api.nvim_buf_set_lines(P.ed_buf, 0, -1, false, ok and ctx or { "# your code would be here" })
api.nvim_win_set_buf(P.ed_win, P.ed_buf)
P.side_buf = new_buf("orca-side://fleet")
map_buf(P.side_buf)
map_buf(P.ed_buf)
side_open()

P.group = api.nvim_create_augroup("OrcaSideProto", { clear = true })
api.nvim_create_autocmd({ "VimResized", "ColorScheme" }, { group = P.group, callback = function() define_hl(); draw(false) end })
draw(true)

P.timer = vim.uv.new_timer()
P.timer:start(200, 200, vim.schedule_wrap(function()
  if not _G.OrcaSideProto then return end
  P.frame = P.frame + 1
  pcall(draw, false)
end))
