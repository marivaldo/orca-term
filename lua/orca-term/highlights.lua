-- orca-term's highlight groups: every colour the sidebar uses.
--
-- Each group links to a standard group of the colorscheme (`Title`, `Comment`, `Diagnostic*`), so
-- the sidebar reads well in any theme. The links are defaults: a person who defines the group in
-- their config keeps their own. `sidebar.lua` defines them again after each `:colorscheme`.

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
}

---The group a state is drawn in; a state this client does not know yet is muted.
---@param state string
---@return string
function M.state_group(state)
  return M.STATE_GROUPS[state] or 'OrcaTermMuted'
end

---Defines every group as a default link. Safe to call again.
function M.define()
  for group, link in pairs(LINKS) do
    vim.api.nvim_set_hl(0, group, { link = link, default = true })
  end
end

return M
