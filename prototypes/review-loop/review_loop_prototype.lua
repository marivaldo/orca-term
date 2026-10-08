-- PROTOTYPE, THROWAWAY. Answers issue #11: can review comments be anchored to diff lines and
-- collected on top of diffview.nvim, or does anchoring force a buffer of our own?
--
-- Run from the repo root:  nvim -c 'luafile prototypes/review-loop/review_loop_prototype.lua'
--
-- Builds a mock lane in a temp dir (PROTOTYPE, wipe me): a base commit plus a real chain of turn
-- snapshots (ADR 0006), written with commit-tree so nothing is signed and no HEAD moves.
--   turn 1  claude-code  session S1   "Add a discount parameter to total()"
--   turn 2  claude-code  session S1   "Guard against negative quantities"
--   (the person edits cart.ts and README between turns 2 and 3)
--   turn 3  opencode     session S2   "Format money with a currency code"   (agent swapped, S1 discarded)
--
-- Keys (global unless noted):
--   ]v / [v   switch variant: A = diffview, B = our own unified diff buffer
--   ]t / [t   switch scope: turn 1, turn 2, turn 3, whole lane (base..tip)
--   cc        (diff buffers only, normal or visual) comment on the line / selection
--   gp        toggle the pending-comments panel
--   gs        preview the prompts that would be sent, one per turn (nothing is sent)
--   g?        raw state: scope, turns, comments and their anchors

local api = vim.api
local ns = api.nvim_create_namespace("review_loop_proto")

local function sh(args, cwd, env)
  local r = vim.system(args, { cwd = cwd, env = env, text = true }):wait()
  if r.code ~= 0 then error(table.concat(args, " ") .. "\n" .. (r.stderr or "")) end
  return vim.trim(r.stdout or "")
end

---------------------------------------------------------------------------------------------------
-- Mock lane
---------------------------------------------------------------------------------------------------

local FILES = {
  base = {
    ["README.md"] = "# shop\n",
    ["src/cart.ts"] = [[
export type Item = { sku: string; price: number; qty: number };

export function total(items: Item[]): number {
  let sum = 0;
  for (const item of items) {
    sum += item.price * item.qty;
  }
  return sum;
}
]],
    ["src/format.ts"] = [[
export function money(cents: number): string {
  return "$" + (cents / 100).toFixed(2);
}
]],
  },
}

local function edit(files, path, content) files[path] = content end

local repo = vim.fn.tempname() .. "-review-loop-PROTOTYPE-wipe-me"
vim.fn.mkdir(repo, "p")
sh({ "git", "init", "-q", "-b", "main" }, repo)

local tree = vim.deepcopy(FILES.base)
local function write_tree()
  for path, content in pairs(tree) do
    vim.fn.mkdir(vim.fs.dirname(repo .. "/" .. path), "p")
    vim.fn.writefile(vim.split(content, "\n", { plain = true }), repo .. "/" .. path, "b")
  end
end

local function snapshot(author, msg, parent)
  write_tree()
  local env = { GIT_INDEX_FILE = repo .. "/.git/orca-proto-index", GIT_AUTHOR_NAME = author,
    GIT_AUTHOR_EMAIL = author .. "@lane", GIT_COMMITTER_NAME = "orca-term", GIT_COMMITTER_EMAIL = "core@lane" }
  sh({ "git", "add", "-A" }, repo, env)
  local t = sh({ "git", "write-tree" }, repo, env)
  local args = { "git", "commit-tree", t, "-m", msg }
  if parent then vim.list_extend(args, { "-p", parent }) end
  return sh(args, repo, env)
end

local base = snapshot("Marivaldo", "base")
sh({ "git", "update-ref", "refs/heads/main", base }, repo)
sh({ "git", "reset", "-q", "--hard", base }, repo)

local SESSIONS = {
  S1 = { agent = "claude-code", id = "8f1c2a4e-3b7d-4e61-9a2f-5c0d1e7b9a33", alive = false },
  S2 = { agent = "opencode", id = "ses_7Hq2mWkR4pXcN9", alive = true },
}

local turns = {}
local function turn(n, session, prompt, mutate)
  local parent = turns[n - 1] and turns[n - 1].end_sha or base
  local begin_sha = snapshot("orca-term", ("lane shop · turn %d begin"):format(n), parent)
  mutate()
  local s = SESSIONS[session]
  local end_sha = snapshot(s.agent, ("lane shop · turn %d end · %s · %s"):format(n, s.agent, s.id), begin_sha)
  turns[n] = { n = n, session = session, agent = s.agent, prompt = prompt, begin_sha = begin_sha, end_sha = end_sha }
end

turn(1, "S1", "Add a discount parameter to total()", function()
  edit(tree, "src/cart.ts", [[
export type Item = { sku: string; price: number; qty: number };

export function total(items: Item[], discount = 0): number {
  let sum = 0;
  for (const item of items) {
    sum += item.price * item.qty;
  }
  return Math.round(sum * (1 - discount));
}
]])
end)

turn(2, "S1", "Guard against negative quantities", function()
  edit(tree, "src/cart.ts", [[
export type Item = { sku: string; price: number; qty: number };

export function total(items: Item[], discount = 0): number {
  let sum = 0;
  for (const item of items) {
    if (item.qty < 0) throw new Error("negative qty");
    sum += item.price * item.qty;
  }
  return Math.round(sum * (1 - discount));
}
]])
  edit(tree, "src/cart.test.ts", [[
import { total } from "./cart";

test("rejects negative qty", () => {
  expect(() => total([{ sku: "a", price: 100, qty: -1 }])).toThrow();
});
]])
end)

-- The person edits between turns 2 and 3: this lands in turn 3's begin snapshot.
edit(tree, "src/cart.ts", [[
export type Item = { sku: string; price: number; qty: number };

// TODO(person): discount should be capped at 1
export function total(items: Item[], discount = 0): number {
  let sum = 0;
  for (const item of items) {
    if (item.qty < 0) throw new Error("negative qty");
    sum += item.price * item.qty;
  }
  return Math.round(sum * (1 - discount));
}
]])
edit(tree, "README.md", "# shop\n\nRun `pnpm test`.\n")

turn(3, "S2", "Format money with a currency code", function()
  edit(tree, "src/format.ts", [[
export function money(cents: number, currency = "USD"): string {
  return new Intl.NumberFormat("en-US", { style: "currency", currency }).format(cents / 100);
}
]])
  edit(tree, "src/cart.ts", [[
export type Item = { sku: string; price: number; qty: number };

// TODO(person): discount should be capped at 1
export function total(items: Item[], discount = 0): number {
  let sum = 0;
  for (const item of items) {
    if (item.qty < 0) throw new Error("negative qty");
    sum += item.price * item.qty;
  }
  return Math.round(sum * (1 - Math.min(discount, 1)));
}
]])
end)

local tip = turns[#turns].end_sha
sh({ "git", "update-ref", "refs/orca-term/lanes/shop", tip }, repo)
write_tree()

---------------------------------------------------------------------------------------------------
-- Scope, blame and anchors
---------------------------------------------------------------------------------------------------

local SCOPES = { "turn 1", "turn 2", "turn 3", "whole lane" }
local state = { variant = "A", scope = 3, comments = {}, log = {} }

local function scope_revs()
  if state.scope <= #turns then
    local t = turns[state.scope]
    return t.begin_sha, t.end_sha, t
  end
  return base, tip, nil
end

local function note(msg)
  table.insert(state.log, 1, msg)
  vim.notify("[review-loop] " .. msg)
end

local function turn_by_sha(sha)
  for _, t in ipairs(turns) do
    if t.end_sha == sha then return t, "end" end
    if t.begin_sha == sha then return t, "begin" end
  end
end

local blame_cache = {}
-- final line in `rev` -> { sha, orig }
local function blame(rev, path)
  local key = rev .. ":" .. path
  if blame_cache[key] then return blame_cache[key] end
  local ok, out = pcall(sh, { "git", "blame", "--porcelain", rev, "--", path }, repo)
  local map = {}
  if ok then
    for line in out:gmatch("[^\n]+") do
      local sha, orig, final = line:match("^(%x+) (%d+) (%d+)")
      if sha and #sha == 40 then map[tonumber(final)] = { sha = sha, orig = tonumber(orig) } end
    end
  end
  blame_cache[key] = map
  return map
end

local function file_lines(rev, path)
  local ok, out = pcall(sh, { "git", "show", rev .. ":" .. path }, repo)
  return ok and vim.split(out, "\n", { plain = true }) or {}
end

-- A comment's anchor is (turn, side, path, l1..l2) in that turn's end snapshot (side "new") or begin
-- snapshot (side "old"). Returns a list: a selection spanning turns is split, one anchor per turn.
local function resolve(side, path, l1, l2)
  local _, _, t = scope_revs()
  if t then
    return { { turn = t.n, side = side, path = path, l1 = l1, l2 = l2 } }
  end
  if side == "old" then
    return nil, "whole-lane view, old side: a base line has no turn, and which turn deleted it needs a reverse blame"
  end
  local map, out, refused = blame(tip, path), {}, {}
  for l = l1, l2 do
    local b = map[l]
    local bt, which = turn_by_sha(b and b.sha or "")
    if bt and which == "end" then
      local last = out[#out]
      if last and last.turn == bt.n and last.l2 == b.orig - 1 then
        last.l2 = b.orig
      else
        table.insert(out, { turn = bt.n, side = "new", path = path, l1 = b.orig, l2 = b.orig })
      end
    else
      table.insert(refused, ("%d (%s)"):format(l, bt and "written by the person between turns" or "base, no turn"))
    end
  end
  if #refused > 0 then note("lines not addressable to an agent: " .. table.concat(refused, ", ")) end
  if #out == 0 then return nil, "no line in the selection was written by a turn" end
  if #out > 1 then note(("selection spans %d turns: the comment was split, one per turn"):format(#out)) end
  return out
end

-- Where does an anchor land in a buffer showing `rev`, `path`, `side`? Returns l1, l2 or nil.
local function locate(c, rev, path, side)
  if c.path ~= path or c.side ~= side then return end
  local t = turns[c.turn]
  if side == "old" then
    if rev == t.begin_sha then return c.l1, c.l2 end
    return
  end
  if rev == t.end_sha then return c.l1, c.l2 end
  -- Another rev (the lane tip in the whole-lane view): follow the anchor through blame.
  local lo, hi
  for final, b in pairs(blame(rev, path)) do
    if b.sha == t.end_sha and b.orig >= c.l1 and b.orig <= c.l2 then
      lo, hi = math.min(lo or final, final), math.max(hi or final, final)
    end
  end
  return lo, hi
end

local function outdated(c)
  if c.side == "old" then return false end
  local l1 = locate(c, tip, c.path, "new")
  return l1 == nil
end

---------------------------------------------------------------------------------------------------
-- Rendering marks into whatever buffer shows a (rev, path, side)
---------------------------------------------------------------------------------------------------

local function virt(c)
  local t = turns[c.turn]
  return { { ("  ▎ %s  "):format(c.text), "DiagnosticInfo" },
    { ("→ turn %d · %s%s"):format(t.n, t.agent, SESSIONS[t.session].alive and "" or " · session discarded"), "Comment" } }
end

local function mark(bufnr, row0, c)
  pcall(api.nvim_buf_set_extmark, bufnr, ns, row0, 0, {
    virt_lines = { virt(c) }, sign_text = "▎", sign_hl_group = "DiagnosticInfo",
  })
end

---------------------------------------------------------------------------------------------------
-- Variant A: on top of diffview.nvim
---------------------------------------------------------------------------------------------------

local A = {}

function A.layout()
  local ok, lib = pcall(require, "diffview.lib")
  if not ok then return end
  local view = lib.get_current_view()
  return view and view.cur_layout
end

function A.sides()
  local layout = A.layout()
  if not layout or not layout.a or not layout.b then return {} end
  local res = {}
  for side, w in pairs({ old = layout.a, new = layout.b }) do
    local f = w.file
    if f and f.bufnr and api.nvim_buf_is_valid(f.bufnr) and f.rev and f.rev.commit then
      res[side] = { bufnr = f.bufnr, winid = w.id, rev = f.rev.commit, path = f.path }
    end
  end
  return res
end

function A.render()
  for side, s in pairs(A.sides()) do
    api.nvim_buf_clear_namespace(s.bufnr, ns, 0, -1)
    for _, c in ipairs(state.comments) do
      local _, l2 = locate(c, s.rev, s.path, side)
      if l2 then mark(s.bufnr, l2 - 1, c) end
    end
    vim.keymap.set({ "n", "x" }, "cc", function() A.comment() end, { buffer = s.bufnr, desc = "review-loop comment" })
  end
end

function A.comment()
  local win = api.nvim_get_current_win()
  for side, s in pairs(A.sides()) do
    if s.winid == win then
      local l1, l2 = vim.fn.line("v"), vim.fn.line(".")
      if l1 > l2 then l1, l2 = l2, l1 end
      if vim.fn.mode() == "n" then l1 = l2 end
      api.nvim_feedkeys(api.nvim_replace_termcodes("<Esc>", true, false, true), "nx", false)
      return A.add(side, s.path, l1, l2)
    end
  end
  note("cursor is not in a diff window")
end

function A.add(side, path, l1, l2)
  local anchors, why = resolve(side, path, l1, l2)
  if not anchors then return note("refused: " .. why) end
  vim.ui.input({ prompt = ("comment on %s:%d-%d (%s side): "):format(path, l1, l2, side) }, function(text)
    if not text or text == "" then return end
    for _, a in ipairs(anchors) do
      a.text = text
      table.insert(state.comments, a)
    end
    A.render()
  end)
end

function A.open()
  local a, b = scope_revs()
  pcall(vim.cmd, "Lazy load diffview.nvim")
  if A.layout() then pcall(vim.cmd, "DiffviewClose") end
  vim.cmd(("DiffviewOpen -C%s %s..%s"):format(vim.fn.fnameescape(repo), a, b))
  vim.defer_fn(A.render, 200)
end

function A.close()
  if A.layout() then pcall(vim.cmd, "DiffviewClose") end
end

api.nvim_create_autocmd("User", {
  pattern = { "DiffviewDiffBufRead", "DiffviewDiffBufWinEnter" },
  callback = function()
    if state.variant == "A" then vim.schedule(A.render) end
  end,
})

---------------------------------------------------------------------------------------------------
-- Variant B: our own unified diff buffer
---------------------------------------------------------------------------------------------------

local B = { buf = nil, map = {} }

function B.build()
  local a, b = scope_revs()
  local out = sh({ "git", "diff", "-U3", a, b }, repo)
  local lines = { ("lane shop · %s · %s..%s"):format(SCOPES[state.scope], a:sub(1, 8), b:sub(1, 8)), "" }
  local map = {}
  local path, old, new
  for _, l in ipairs(vim.split(out, "\n", { plain = true })) do
    local entry
    if l:match("^%+%+%+ ") then
      path = l:match("^%+%+%+ b/(.*)") or path
    elseif l:match("^@@") then
      local o, n = l:match("^@@ %-(%d+),?%d* %+(%d+)")
      old, new = tonumber(o), tonumber(n)
    elseif old and l:sub(1, 1) == "+" then
      entry = { path = path, side = "new", line = new, rev = b }; new = new + 1
    elseif old and l:sub(1, 1) == "-" and not l:match("^%-%-%- ") then
      entry = { path = path, side = "old", line = old, rev = a }; old = old + 1
    elseif old and l:sub(1, 1) == " " then
      entry = { path = path, side = "new", line = new, rev = b }; old, new = old + 1, new + 1
    elseif l:match("^diff ") then
      old = nil
    end
    table.insert(lines, l)
    map[#lines] = entry
  end
  return lines, map
end

function B.render()
  if not (B.buf and api.nvim_buf_is_valid(B.buf)) then return end
  api.nvim_buf_clear_namespace(B.buf, ns, 0, -1)
  for _, c in ipairs(state.comments) do
    -- mark under the last buffer line that shows the anchor's l2
    local best
    for row, e in pairs(B.map) do
      local _, l2 = locate(c, e.rev, e.path, e.side)
      if l2 and e.line == l2 then best = math.max(best or 0, row) end
    end
    if best then mark(B.buf, best - 1, c) end
  end
end

function B.open()
  local lines, map = B.build()
  if not (B.buf and api.nvim_buf_is_valid(B.buf)) then
    B.buf = api.nvim_create_buf(false, true)
    api.nvim_buf_set_name(B.buf, "review-loop://diff")
    vim.bo[B.buf].filetype = "diff"
    vim.keymap.set({ "n", "x" }, "cc", function() B.comment() end, { buffer = B.buf })
  end
  vim.bo[B.buf].modifiable = true
  api.nvim_buf_set_lines(B.buf, 0, -1, false, lines)
  vim.bo[B.buf].modifiable = false
  B.map = map
  vim.cmd("tabnew")
  api.nvim_win_set_buf(0, B.buf)
  B.render()
end

function B.close()
  if B.buf and api.nvim_buf_is_valid(B.buf) then
    for _, w in ipairs(vim.fn.win_findbuf(B.buf)) do pcall(api.nvim_win_close, w, true) end
  end
end

function B.comment()
  local l1, l2 = vim.fn.line("v"), vim.fn.line(".")
  if l1 > l2 then l1, l2 = l2, l1 end
  if vim.fn.mode() == "n" then l1 = l2 end
  api.nvim_feedkeys(api.nvim_replace_termcodes("<Esc>", true, false, true), "nx", false)
  -- keep only the side and file of the first mapped row; ranges are contiguous in one file and side
  local first
  local lo, hi
  for row = l1, l2 do
    local e = B.map[row]
    if e then
      first = first or e
      if e.path == first.path and e.side == first.side then
        lo, hi = math.min(lo or e.line, e.line), math.max(hi or e.line, e.line)
      end
    end
  end
  if not first then return note("not a diff line") end
  A.add(first.side, first.path, lo, hi)
  vim.schedule(B.render)
end

---------------------------------------------------------------------------------------------------
-- Collection: pending panel and prompt preview
---------------------------------------------------------------------------------------------------

local function grouped()
  local by = {}
  for _, c in ipairs(state.comments) do
    by[c.turn] = by[c.turn] or {}
    table.insert(by[c.turn], c)
  end
  return by
end

local panel = { buf = nil }
local function panel_lines()
  local lines = { ("Pending review comments (%d) · scope %s · variant %s"):format(#state.comments, SCOPES[state.scope], state.variant), "" }
  for n, cs in pairs(grouped()) do
    local t = turns[n]
    local s = SESSIONS[t.session]
    table.insert(lines, ("── turn %d · %s · session %s%s"):format(n, t.agent, s.id:sub(1, 12), s.alive and " (current)" or " (DISCARDED, see #16)"))
    for _, c in ipairs(cs) do
      table.insert(lines, ("   %s:%d%s [%s]%s  %s"):format(c.path, c.l1, c.l2 ~= c.l1 and ("-" .. c.l2) or "", c.side,
        outdated(c) and " (outdated at lane tip)" or "", c.text))
    end
  end
  if #state.log > 0 then
    vim.list_extend(lines, { "", "Log:" })
    for i = 1, math.min(6, #state.log) do table.insert(lines, "   " .. state.log[i]) end
  end
  return lines
end

local function toggle_panel()
  if panel.buf and api.nvim_buf_is_valid(panel.buf) and #vim.fn.win_findbuf(panel.buf) > 0 then
    for _, w in ipairs(vim.fn.win_findbuf(panel.buf)) do pcall(api.nvim_win_close, w, true) end
    return
  end
  panel.buf = panel.buf and api.nvim_buf_is_valid(panel.buf) and panel.buf or api.nvim_create_buf(false, true)
  api.nvim_buf_set_lines(panel.buf, 0, -1, false, panel_lines())
  vim.cmd("botright 12split")
  api.nvim_win_set_buf(0, panel.buf)
end

local function float(lines, title)
  local buf = api.nvim_create_buf(false, true)
  api.nvim_buf_set_lines(buf, 0, -1, false, lines)
  vim.bo[buf].filetype = "markdown"
  local w, h = math.floor(vim.o.columns * 0.8), math.floor(vim.o.lines * 0.8)
  api.nvim_open_win(buf, true, { relative = "editor", width = w, height = h, row = math.floor((vim.o.lines - h) / 2),
    col = math.floor((vim.o.columns - w) / 2), border = "rounded", title = title })
  vim.keymap.set("n", "q", "<cmd>close<cr>", { buffer = buf })
end

local function preview_prompts()
  local lines = {}
  for n, cs in pairs(grouped()) do
    local t = turns[n]
    local s = SESSIONS[t.session]
    local cmd = t.agent == "claude-code" and ("claude -p --resume %s"):format(s.id)
      or ("opencode run --format json -s %s </dev/null"):format(s.id)
    vim.list_extend(lines, {
      ("## → %s, session %s%s"):format(t.agent, s.id, s.alive and "" or "  ⚠ DISCARDED: not resumable as-is (#16)"),
      ("would run: `%s`"):format(cmd), "",
      ("Review comments on the changes you made in turn %d (\"%s\"). Address each one."):format(n, t.prompt), "",
    })
    for _, c in ipairs(cs) do
      local snap = c.side == "new" and t.end_sha or t.begin_sha
      local src = file_lines(snap, c.path)
      table.insert(lines, ("%s:%d%s%s"):format(c.path, c.l1, c.l2 ~= c.l1 and ("-" .. c.l2) or "",
        c.side == "old" and " (a line your turn removed)" or ""))
      table.insert(lines, "```")
      for l = c.l1, c.l2 do table.insert(lines, src[l] or "") end
      table.insert(lines, "```")
      table.insert(lines, "> " .. c.text)
      if outdated(c) then table.insert(lines, "(note: a later turn or the person has since changed these lines)") end
      table.insert(lines, "")
    end
    table.insert(lines, "---")
  end
  if #lines == 0 then lines = { "No pending comments." } end
  float(lines, " prompts that would be sent (nothing is sent) ")
end

local function show_state()
  local t = {}
  for _, x in ipairs(turns) do
    table.insert(t, ("turn %d %s %s begin=%s end=%s"):format(x.n, x.agent, x.session, x.begin_sha:sub(1, 8), x.end_sha:sub(1, 8)))
  end
  local lines = { "repo: " .. repo, "base: " .. base:sub(1, 8), "tip:  " .. tip:sub(1, 8) .. "  (refs/orca-term/lanes/shop)", "" }
  vim.list_extend(lines, t)
  vim.list_extend(lines, { "", "variant " .. state.variant .. " · scope " .. SCOPES[state.scope], "", "comments:" })
  vim.list_extend(lines, vim.split(vim.inspect(state.comments), "\n"))
  float(lines, " raw state ")
end

---------------------------------------------------------------------------------------------------
-- Switching
---------------------------------------------------------------------------------------------------

local V = { A = A, B = B }
local function reopen()
  A.close(); B.close()
  V[state.variant].open()
  note(("variant %s · %s"):format(state.variant, SCOPES[state.scope]))
end

local function cycle(list, cur, d) return ((cur - 1 + d) % #list) + 1 end

vim.keymap.set("n", "]v", function() state.variant = state.variant == "A" and "B" or "A"; reopen() end)
vim.keymap.set("n", "[v", function() state.variant = state.variant == "A" and "B" or "A"; reopen() end)
vim.keymap.set("n", "]t", function() state.scope = cycle(SCOPES, state.scope, 1); reopen() end)
vim.keymap.set("n", "[t", function() state.scope = cycle(SCOPES, state.scope, -1); reopen() end)
vim.keymap.set("n", "gp", toggle_panel)
vim.keymap.set("n", "gs", preview_prompts)
vim.keymap.set("n", "g?", show_state)

reopen()
