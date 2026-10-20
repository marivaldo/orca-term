-- orca-term's `:checkhealth orca-term`: the core binary this client uses, the core's own report
-- on the programs it needs (`orca-term doctor --json`, run through core.lua like every other core
-- command), the contract, and what the Neovim side needs: Neovim 0.11 or later, and diffview.nvim
-- for the review. It only reports: nothing is installed.

local core = require('orca-term.core')
local review = require('orca-term.review')

local M = {}

---Shows one check of the core's report as `vim.health` does: ok, a warning, an error for a
---program missing or too old, or information.
---@param check orca_term.Check
local function report_check(check)
  if check.status == 'ok' then
    vim.health.ok(check.message)
  elseif check.status == 'warn' then
    vim.health.warn(check.message)
  elseif check.status == 'info' then
    vim.health.info(check.message)
  else
    vim.health.error(check.message)
  end
end

---The core's report, each check on a line of its own, and whether the core speaks this client's
---contract.
local function check_core()
  vim.health.start('orca-term: prerequisites (orca-term doctor)')
  local report, err = core.doctor()
  if not report then
    vim.health.error(err and err.message or 'orca-term: doctor gave no report')
    return
  end
  for _, check in ipairs(report.checks or {}) do
    report_check(check)
  end
  -- core.doctor() refuses a core speaking another contract, so here they agree.
  vim.health.ok(('the core speaks contract %d, as this client does'):format(report.contract))
end

---The Neovim version, and diffview.nvim for the review.
local function check_neovim()
  vim.health.start('orca-term: Neovim')
  local version = tostring(vim.version())
  if vim.fn.has('nvim-0.11') == 1 then
    vim.health.ok('Neovim ' .. version)
  else
    vim.health.error('Neovim ' .. version .. ': orca-term needs 0.11 or later')
  end
  if review.available() then
    vim.health.ok('diffview.nvim found')
  else
    vim.health.warn(
      'diffview.nvim not found: the review (`d`) needs diffview.nvim',
      { 'install it from ' .. review.DIFFVIEW_URL }
    )
  end
end

---What `:checkhealth orca-term` runs.
function M.check()
  vim.health.start('orca-term: core binary')
  local binary, err = core.find_binary()
  if binary then
    vim.health.ok('using ' .. binary)
    check_core()
  else
    vim.health.error(err and err.message or 'orca-term: no core binary found')
  end
  check_neovim()
end

return M
