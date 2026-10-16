-- Seam 2, golden: the sidebar as Neovim shows it, for a real repository holding a worktree in
-- every state the core reports today, read by the real binary (`support.every_state_repo()`: the
-- agents are the fake claude, their states come from recordings of claude's hooks, and unseen and
-- seen worktrees show side by side). The buffer's text and its highlights are compared with
-- spec/golden/sidebar.txt.
--
-- The golden file holds the buffer's lines, then one highlight per line as
-- `<line>:<first>-<last> <group>` (1-based line, 0-based byte columns, the last one excluded).
-- After a deliberate change to the sidebar's look, rewrite it and review the diff:
--
--     UPDATE_GOLDEN=1 just lua-test
--
-- The repository's directory is named `repo` and no path is shown, so the file is the same on
-- every machine.

local support = require('support')
local sidebar = require('orca-term.sidebar')

local GOLDEN = support.root .. '/spec/golden/sidebar.txt'

---The sidebar buffer's text and highlights, in the golden file's format.
---@return string[]
local function snapshot()
  local buf = assert(sidebar.buffer())
  local out = vim.api.nvim_buf_get_lines(buf, 0, -1, false)
  table.insert(out, '')
  table.insert(out, '# highlights: <line>:<first>-<last> <group>')
  local marks = vim.api.nvim_buf_get_extmarks(buf, sidebar.NAMESPACE, 0, -1, { details = true })
  for _, mark in ipairs(marks) do
    local row, col, details = mark[2], mark[3], mark[4] or {}
    table.insert(out, ('%d:%d-%d %s'):format(row + 1, col, details.end_col, details.hl_group))
  end
  return out
end

describe('the sidebar, golden', function()
  after_each(function()
    support.reset_editor()
    support.cleanup()
  end)

  it('renders every state the core reports today, unseen and seen', function()
    support.load_client()
    local repo, release = support.every_state_repo()
    finally(release)

    support.cd(repo.root)
    vim.cmd('OrcaTerm sidebar')
    local actual = snapshot()

    if os.getenv('UPDATE_GOLDEN') == '1' then
      vim.fn.mkdir(vim.fs.dirname(GOLDEN), 'p')
      vim.fn.writefile(actual, GOLDEN)
    end
    assert(
      vim.uv.fs_stat(GOLDEN),
      'no ' .. GOLDEN .. '; write it with UPDATE_GOLDEN=1 just lua-test'
    )
    assert.are.same(vim.fn.readfile(GOLDEN), actual)
  end)
end)
