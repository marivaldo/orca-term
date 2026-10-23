-- Seam 2: loading the client, as Neovim does at startup, defines the `:OrcaTerm` command and its
-- `<Plug>` maps, and no global keymap in any mode. Every key the client adds is local to one of its
-- buffers.

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
    assert.are.same({
      'install-cli',
      'jump',
      'notes',
      'pr-refresh',
      'review',
      'sessions',
      'sidebar',
      'turns',
      'worktree',
    }, vim.fn.getcompletion('OrcaTerm ', 'cmdline'))
    assert.are.same({ 'worktree' }, vim.fn.getcompletion('OrcaTerm wo', 'cmdline'))
    assert.are.same({}, vim.fn.getcompletion('OrcaTerm worktree ', 'cmdline'))
  end)

  it('defines the <Plug> maps a person can bind keys to', function()
    support.load_client()
    local wanted = {
      '<Plug>(OrcaTermSidebar)',
      '<Plug>(OrcaTermJump)',
      '<Plug>(OrcaTermReview)',
      '<Plug>(OrcaTermNotes)',
      '<Plug>(OrcaTermTurns)',
      '<Plug>(OrcaTermInstallCli)',
      '<Plug>(OrcaTermPrRefresh)',
      '<Plug>(OrcaTermSessions)',
    }
    for n = 1, 9 do
      table.insert(wanted, ('<Plug>(OrcaTermWorktree%d)'):format(n))
    end
    for _, lhs in ipairs(wanted) do
      -- maparg() returns an empty dictionary when no such map exists.
      assert(vim.fn.maparg(lhs, 'n', false, true).callback, lhs)
    end
  end)
end)
