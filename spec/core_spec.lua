-- Seam 2: lua/orca-term/core.lua, the client's link to the core, against the real binary that
-- `cargo build` made. The core is never mocked; the only fake binaries stand in for a broken or
-- incompatible core, to check that the client refuses it.

local support = require('support')
local orca_term = require('orca-term')
local core = require('orca-term.core')

describe('core', function()
  after_each(support.cleanup)

  it('finds the binary cargo build made in this repository', function()
    local release = support.root .. '/target/release/orca-term'
    local expected = vim.fn.executable(release) == 1 and release or support.binary
    assert.are.equal(expected, core.find_binary())
  end)

  it('uses the binary set in setup()', function()
    local dir = support.temp_dir()
    local binary = dir .. '/my-orca-term'
    support.write_script(binary, { 'exit 0' })
    orca_term.setup({ binary = binary })
    assert.are.equal(binary, core.find_binary())
  end)

  it('lists the fleet of a repository with one worktree', function()
    orca_term.setup({ binary = support.binary })
    local repo = support.repo()
    local path = support.add_worktree(repo, repo.tmp .. '/fix', 'fix')

    local fleet, err = core.worktree_ls(repo.root)

    assert.is_nil(err)
    assert(fleet)
    assert.are.equal(core.CONTRACT, fleet.contract)
    assert.are.same({ path = repo.root, branch = 'main' }, fleet.primary)
    assert.are.same(
      { { name = 'fix', path = path, branch = 'fix', state = 'no_agent' } },
      fleet.worktrees
    )
  end)

  it('returns the error the core prints as JSON, outside a git repository', function()
    orca_term.setup({ binary = support.binary })
    local outside = support.isolate().tmp

    local fleet, err = core.worktree_ls(outside)

    assert.is_nil(fleet)
    assert(err)
    assert.are.equal('other', err.kind)
    assert.are.equal('orca-term: not inside a git repository: ' .. outside, err.message)
  end)

  it('names the fix when the binary set in setup() does not exist', function()
    local missing = support.temp_dir() .. '/orca-term'
    orca_term.setup({ binary = missing })

    local fleet, err = core.worktree_ls(support.root)

    assert.is_nil(fleet)
    assert(err)
    assert.are.equal('no_binary', err.kind)
    assert.are.equal(
      'orca-term: the core binary set in setup() is not an executable file: '
        .. missing
        .. '; fix `binary` in setup()',
      err.message
    )
  end)

  it('names the fix when no binary is set, built or on PATH', function()
    -- A copy of core.lua in a directory with no target/ finds no binary that cargo build made,
    -- and PATH holds only an empty directory.
    local copy = support.temp_dir()
    local copied_file = copy .. '/lua/orca-term/core.lua'
    vim.fn.mkdir(copy .. '/lua/orca-term', 'p')
    assert(vim.uv.fs_copyfile(support.root .. '/lua/orca-term/core.lua', copied_file))
    local copied_core = dofile(copied_file)
    support.set_env('PATH', support.temp_dir())

    local fleet, err = copied_core.worktree_ls(support.root)

    assert.is_nil(fleet)
    assert(err)
    assert.are.equal('no_binary', err.kind)
    assert.are.equal(
      'orca-term: no core binary found; run `cargo build` in '
        .. copy
        .. ' or set `binary` in setup()',
      err.message
    )
  end)

  it('refuses a core that speaks another contract, naming the fix', function()
    local fake = support.temp_dir() .. '/orca-term'
    support.write_script(fake, {
      [[echo '{"contract":999,"version":"9.9.9","primary":{"path":"/x","branch":"main"},"worktrees":[]}']],
    })
    orca_term.setup({ binary = fake })

    local fleet, err = core.worktree_ls(support.root)

    assert.is_nil(fleet)
    assert(err)
    assert.are.equal('contract', err.kind)
    assert.are.equal(
      'orca-term: the core at '
        .. fake
        .. ' speaks contract 999, this plugin speaks 1; rebuild it with `cargo build` in '
        .. support.root
        .. ' or set `binary` in setup()',
      err.message
    )
  end)

  it('refuses output that is not JSON, showing what the core said on stderr', function()
    local fake = support.temp_dir() .. '/orca-term'
    support.write_script(fake, { 'echo hello', 'echo broken >&2', 'exit 3' })
    orca_term.setup({ binary = fake })

    local fleet, err = core.worktree_ls(support.root)

    assert.is_nil(fleet)
    assert(err)
    assert.are.equal('failed', err.kind)
    assert.are.equal(
      'orca-term: `' .. fake .. ' worktree ls --json` printed no JSON (exit 3): broken',
      err.message
    )
  end)
end)
