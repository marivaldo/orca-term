-- orca-term's Neovim client: its settings.
--
-- Calling `require('orca-term').setup(opts)` is optional. Every module reads `config`, which holds
-- the defaults until setup() is called.

---@class orca_term.Config
---Path to the core binary. When unset, the client looks for the release binary the build hook
---installed in the repository it is loaded from, then for the binary `cargo build` makes there,
---then for `orca-term` on PATH.
---@field binary? string

local M = {}

---@type orca_term.Config
local defaults = {
  binary = nil,
}

---The settings in use.
---@type orca_term.Config
M.config = vim.deepcopy(defaults)

---Sets the options given over the defaults. Calling it again starts from the defaults again.
---@param opts? orca_term.Config
function M.setup(opts)
  vim.validate('opts', opts, 'table', true)
  opts = opts or {}
  vim.validate('opts.binary', opts.binary, 'string', true)
  M.config = vim.tbl_extend('force', vim.deepcopy(defaults), opts)
end

return M
