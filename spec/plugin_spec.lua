-- Seam 2: loading the client, as Neovim does at startup, defines the `:OrcaTerm` command and no
-- global keymap in any mode. Every key the client adds is local to one of its buffers.

local support = require('support')

describe('loading the client', function()
  after_each(support.cleanup)

  it('defines no global keymap in any mode', function()
    local before = support.global_keymaps()
    support.load_client()
    assert.are.same(before, support.global_keymaps())
  end)

  it('defines :OrcaTerm, completing its subcommands', function()
    support.load_client()
    assert.are.equal(2, vim.fn.exists(':OrcaTerm'))
    assert.are.same({ 'sidebar' }, vim.fn.getcompletion('OrcaTerm ', 'cmdline'))
  end)
end)
