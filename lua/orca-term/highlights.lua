-- orca-term's highlight groups: every colour the sidebar, its detail float, the status strip and
-- the notes in a review use.
--
-- Each group links to a standard group of the colorscheme (`Title`, `Comment`, `Diagnostic*`), so
-- the sidebar reads well in any theme, except the unseen worktree's name, which is only bold. Every
-- group is a default: a person who defines the group in their config keeps their own. `sidebar.lua`
-- and `notes.lua` define them again after each `:colorscheme`.

local M = {}

---The group each state's glyph (and, under "Needs you", its second line) is drawn in.
---@type table<string, string>
M.STATE_GROUPS = {
  no_agent = 'OrcaTermStateNoAgent',
  setting_up = 'OrcaTermStateSettingUp',
  not_ready = 'OrcaTermStateNotReady',
  broken = 'OrcaTermStateBroken',
  idle = 'OrcaTermStateIdle',
  working = 'OrcaTermStateWorking',
  waiting = 'OrcaTermStateWaiting',
  finished = 'OrcaTermStateFinished',
  failed = 'OrcaTermStateFailed',
  interrupted = 'OrcaTermStateInterrupted',
}

---Each group and the standard group it links to.
---@type table<string, string>
local LINKS = {
  OrcaTermHeader = 'Title',
  OrcaTermMuted = 'Comment',
  OrcaTermBorder = 'FloatBorder',
  OrcaTermError = 'DiagnosticError',
  OrcaTermStateNoAgent = 'DiagnosticUnnecessary',
  OrcaTermStateSettingUp = 'DiagnosticInfo',
  OrcaTermStateNotReady = 'DiagnosticWarn',
  OrcaTermStateBroken = 'DiagnosticError',
  OrcaTermStateIdle = 'DiagnosticUnnecessary',
  OrcaTermStateWorking = 'DiagnosticWarn',
  OrcaTermStateWaiting = 'DiagnosticWarn',
  OrcaTermStateFinished = 'DiagnosticOk',
  OrcaTermStateFailed = 'DiagnosticError',
  OrcaTermStateInterrupted = 'DiagnosticUnnecessary',
  OrcaTermUnseen = 'DiagnosticWarn',
  -- The icons at the right of a worktree's name line, such as its pending notes `✎N`.
  OrcaTermBadge = 'DiagnosticHint',
  -- The tool calls the agent's permission rules refused: their count `⊘N` at the right of a
  -- worktree's name line, and the `⊘` before each one in the detail float.
  OrcaTermDenied = 'DiagnosticWarn',
  -- A git operation in progress (a merge, a rebase, a cherry-pick or a revert): the `≠` at the
  -- right of a worktree's name line, and its line in the detail float.
  OrcaTermOperation = 'DiagnosticWarn',
  -- The agent's label and the age of its last activity at the right end of a worktree's name
  -- line, `cc 9m`: muted.
  OrcaTermAge = 'Comment',
  -- The notes drawn in a review: their signs, their text and their count in the file panel.
  OrcaTermNote = 'DiagnosticInfo',
  -- A worktree's pull request, `PR` at the right of its name line and its line in the detail
  -- float: an open one with no checks reads green, a merged one purple (`Constant` is the group
  -- the bundled colorschemes most often draw purple or magenta), a closed one or a draft muted.
  OrcaTermPrOpen = 'DiagnosticOk',
  OrcaTermPrMerged = 'Constant',
  OrcaTermPrClosed = 'Comment',
  OrcaTermPrDraft = 'Comment',
  -- An open pull request takes the colour of its checks.
  OrcaTermPrPassing = 'DiagnosticOk',
  OrcaTermPrFailing = 'DiagnosticError',
  OrcaTermPrPending = 'DiagnosticWarn',
}

---Each group drawn with attributes of its own rather than a link.
---@type table<string, vim.api.keyset.highlight>
local ATTRIBUTES = {
  -- The name of an unseen worktree.
  OrcaTermUnseenName = { bold = true },
}

---The group a state is drawn in; a state this client does not know yet is muted.
---@param state string
---@return string
function M.state_group(state)
  return M.STATE_GROUPS[state] or 'OrcaTermMuted'
end

---Defines every group as a default. Safe to call again.
function M.define()
  for group, link in pairs(LINKS) do
    vim.api.nvim_set_hl(0, group, { link = link, default = true })
  end
  for group, attributes in pairs(ATTRIBUTES) do
    vim.api.nvim_set_hl(0, group, vim.tbl_extend('force', attributes, { default = true }))
  end
end

return M
