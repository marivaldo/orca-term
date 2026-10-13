-- Seam 2: loading the client, as Neovim does at startup, defines the `:OrcaTerm` command and no
-- global keymap in any mode. Every key the client adds is local to one of its buffers.

local support = require('support')

local MODES = { 'n', 'v', 'x', 's', 'o', 'i', 'c', 't' }

---Every global keymap, as `<mode> <lhs>`, sorted. `<Plug>` maps are left out: nobody can type
---them, they only exist for a person to bind a key of their own to.
---@return string[]
local function global_keymaps()
  local found = {}
  for _, mode in ipairs(MODES) do
    for _, keymap in ipairs(vim.api.nvim_get_keymap(mode)) do
      if not vim.startswith(keymap.lhs, '<Plug>') then
        table.insert(found, mode .. ' ' .. keymap.lhs)
      end
    end
  end
  table.sort(found)
  return found
end

---Loads the client the way Neovim does at startup, then calls setup().
local function load_client()
  vim.opt.runtimepath:prepend(support.root)
  vim.cmd.runtime('plugin/orca-term.lua')
  require('orca-term').setup()
end

describe('loading the client', function()
  after_each(support.cleanup)

  it('defines no global keymap in any mode', function()
    local before = global_keymaps()
    load_client()
    assert.are.same(before, global_keymaps())
  end)

  it('defines :OrcaTerm, completing its subcommands', function()
    load_client()
    assert.are.equal(2, vim.fn.exists(':OrcaTerm'))
    assert.are.same({ 'sidebar' }, vim.fn.getcompletion('OrcaTerm ', 'cmdline'))
  end)
end)
