-- orca-term's sidebar: the attention order, which worktree comes first and under which heading.
--
-- Pure: data in, data out. No window, no buffer, no process. The rank comes from the spec (issue
-- #25, "Attention rank"): what needs the person first, then what is running, then the rest.

local M = {}

---One section of the sidebar: a heading and the worktrees under it, in order.
---@class orca_term.Section
---@field title string `Needs you`, `Running` or `Quiet`
---@field worktrees orca_term.RankedWorktree[]

---A worktree as the attention order reads it: the core's worktree, its state and unseen mark.
---@class orca_term.RankedWorktree: orca_term.Worktree

---The section titles, in the order they are shown.
M.TITLES = { 'Needs you', 'Running', 'Quiet' }

---The states a worktree's turn can end in. Only `unseen` ones jump to "Needs you".
local ENDED = { finished = true, failed = true, interrupted = true }

---A worktree's attention rank, 1 (needs the person most) to 8 (needs nothing).
---@param worktree orca_term.RankedWorktree
---@return integer
function M.rank(worktree)
  local state = worktree.state
  local unseen = worktree.unseen == true
  if state == 'waiting' then
    return 1
  end
  if unseen and (state == 'failed' or state == 'interrupted') then
    return 2
  end
  if state == 'broken' or state == 'not_ready' then
    return 3
  end
  if unseen then
    return 4
  end
  if state == 'working' or state == 'setting_up' then
    return 5
  end
  if ENDED[state] then
    return 6
  end
  if state == 'idle' then
    return 7
  end
  -- `no_agent`, and any state this client does not know yet.
  return 8
end

---The section a rank falls in: 1 "Needs you" (ranks 1-4), 2 "Running" (5), 3 "Quiet" (the rest).
---@param rank integer
---@return integer
function M.section_index(rank)
  if rank <= 4 then
    return 1
  end
  if rank == 5 then
    return 2
  end
  return 3
end

---Whether `a` comes before `b`: by rank, then by recency (the newest last activity first, a
---worktree with none after those with one), then by name, then by path (two worktrees may share a
---name).
---@param a orca_term.RankedWorktree
---@param b orca_term.RankedWorktree
---@return boolean
local function comes_first(a, b)
  local rank_a, rank_b = M.rank(a), M.rank(b)
  if rank_a ~= rank_b then
    return rank_a < rank_b
  end
  if a.last_activity ~= b.last_activity then
    if a.last_activity == nil or b.last_activity == nil then
      return b.last_activity == nil
    end
    return a.last_activity > b.last_activity
  end
  if a.name ~= b.name then
    return a.name < b.name
  end
  return a.path < b.path
end

---The fleet's worktrees in sections, in the order they are shown. Empty sections are left out.
---@param worktrees orca_term.RankedWorktree[]
---@return orca_term.Section[]
function M.sections(worktrees)
  local sorted = {}
  for _, worktree in ipairs(worktrees) do
    table.insert(sorted, worktree)
  end
  table.sort(sorted, comes_first)

  local grouped = {}
  for index = 1, #M.TITLES do
    grouped[index] = {}
  end
  for _, worktree in ipairs(sorted) do
    table.insert(grouped[M.section_index(M.rank(worktree))], worktree)
  end

  local sections = {}
  for index, title in ipairs(M.TITLES) do
    if #grouped[index] > 0 then
      table.insert(sections, { title = title, worktrees = grouped[index] })
    end
  end
  return sections
end

return M
