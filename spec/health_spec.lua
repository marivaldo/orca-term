-- Seam 2: `:checkhealth orca-term` (lua/orca-term/health.lua) against the real binary `cargo
-- build` made, with the fake claude and sandbox tools of `support.agent_repo()` on PATH. The
-- `vim.health` calls are captured by stubbing its functions, so each line is checked as written;
-- one spec runs the real `:checkhealth` to see Neovim find the module. diffview.nvim is made
-- present or absent through `package.preload`.

local support = require('support')
local orca_term = require('orca-term')
local core = require('orca-term.core')
local health = require('orca-term.health')

---The `vim.health` functions the stubs replace, put back after each spec.
local real_health = {}
local HEALTH_FUNCTIONS = { 'start', 'ok', 'warn', 'error', 'info' }

---Runs `check()` and returns every `vim.health` call it made, as `{ kind, message }`.
---@return string[][]
local function checkhealth()
  local calls = {}
  for _, name in ipairs(HEALTH_FUNCTIONS) do
    vim.health[name] = function(message)
      table.insert(calls, { name, message })
    end
  end
  health.check()
  return calls
end

---Makes `require('diffview')` succeed (`present`) or fail, whatever is installed.
---@param present boolean
local function diffview(present)
  package.loaded['diffview'] = nil
  -- rawset: review_spec.lua stubs the same field, which LuaLS would take for a duplicate.
  rawset(package.preload, 'diffview', function()
    if present then
      return {}
    end
    error('diffview.nvim is not installed in this spec')
  end)
end

---The sandbox line on this OS, where the fake sandbox tools are all present.
local SANDBOX = vim.fn.has('mac') == 1 and 'sandbox sandbox-exec' or 'sandbox bwrap and socat'

---The git line: the real git, linked on the fixture's PATH.
---@return string
local function git_line()
  local version =
    vim.trim(support.run({ 'git', '--version' })):match('^git version (%d+%.%d+%.%d+)')
  return 'git ' .. version
end

---The binary's version, from its own report.
---@return string
local function core_version()
  return vim.json.decode(support.run({ support.binary, 'doctor', '--json' })).version
end

---The Neovim section, for a Neovim of at least 0.11 and diffview `present` or not.
---@param present boolean
---@return string[][]
local function neovim_section(present)
  local diffview_line = present and { 'ok', 'diffview.nvim found' }
    or { 'warn', 'diffview.nvim not found: the review (`d`) needs diffview.nvim' }
  return {
    { 'start', 'orca-term: Neovim' },
    { 'ok', 'Neovim ' .. tostring(vim.version()) },
    diffview_line,
  }
end

describe(':checkhealth orca-term', function()
  before_each(function()
    for _, name in ipairs(HEALTH_FUNCTIONS) do
      real_health[name] = vim.health[name]
    end
  end)

  after_each(function()
    for _, name in ipairs(HEALTH_FUNCTIONS) do
      vim.health[name] = real_health[name]
    end
    package.preload['diffview'] = nil
    package.loaded['diffview'] = nil
    support.cleanup()
  end)

  it('reports the binary, every check of the core, the contract and Neovim', function()
    orca_term.setup({ binary = support.binary })
    local version = core_version()
    local git = git_line()
    support.agent_repo()
    diffview(true)

    local calls = checkhealth()

    assert.are.same(
      vim.list_extend({
        { 'start', 'orca-term: core binary' },
        { 'ok', 'using ' .. support.binary },
        { 'start', 'orca-term: prerequisites (orca-term doctor)' },
        { 'ok', git },
        { 'ok', SANDBOX },
        { 'ok', 'claude 2.1.296' },
        { 'ok', 'opencode not installed (optional)' },
        { 'info', 'contract 1, orca-term ' .. version },
        { 'ok', 'the core speaks contract ' .. core.CONTRACT .. ', as this client does' },
      }, neovim_section(true)),
      calls
    )
  end)

  it('shows a missing program as an error naming the fix, and a missing diffview', function()
    orca_term.setup({ binary = support.binary })
    local repo = support.agent_repo()
    vim.fn.delete(repo.bin .. '/claude')
    diffview(false)

    local calls = checkhealth()

    assert.are.same({
      'error',
      'claude: not found on PATH; install Claude Code '
        .. '(https://docs.anthropic.com/en/docs/claude-code)',
    }, calls[6])
    assert.are.same(neovim_section(false), vim.list_slice(calls, #calls - 2))
  end)

  it('says no binary is found, and still checks Neovim', function()
    local missing = support.temp_dir() .. '/orca-term'
    orca_term.setup({ binary = missing })
    diffview(true)

    local calls = checkhealth()

    assert.are.same(
      vim.list_extend({
        { 'start', 'orca-term: core binary' },
        {
          'error',
          'orca-term: the core binary set in setup() is not an executable file: '
            .. missing
            .. '; fix `binary` in setup()',
        },
      }, neovim_section(true)),
      calls
    )
  end)

  it('refuses a core that speaks another contract', function()
    local fake = support.temp_dir() .. '/orca-term'
    support.write_script(fake, {
      [[echo '{"contract":999,"version":"9.9.9","checks":[]}']],
    })
    orca_term.setup({ binary = fake })
    diffview(true)

    local calls = checkhealth()

    assert.are.equal('error', calls[4][1])
    assert(calls[4][2]:find('speaks contract 999, this plugin speaks 1', 1, true), calls[4][2])
    assert.are.same(neovim_section(true), vim.list_slice(calls, 5))
  end)

  it('is found by Neovim as `:checkhealth orca-term`', function()
    orca_term.setup({ binary = support.binary })
    support.agent_repo()
    diffview(true)
    vim.opt.runtimepath:append(support.root)

    local tabs = #vim.api.nvim_list_tabpages()
    vim.cmd('silent checkhealth orca-term')
    local buffer = vim.api.nvim_get_current_buf()
    local lines = vim.api.nvim_buf_get_lines(buffer, 0, -1, false)
    -- The report opens in a tab of its own, or in the current window when nothing else is shown.
    if #vim.api.nvim_list_tabpages() > tabs then
      vim.cmd('tabclose')
    end
    if vim.api.nvim_buf_is_valid(buffer) then
      vim.api.nvim_buf_delete(buffer, { force = true })
    end
    vim.opt.runtimepath:remove(support.root)

    local text = table.concat(lines, '\n')
    assert(text:find('orca-term: core binary', 1, true), text)
    assert(text:find('claude 2.1.296', 1, true), text)
    assert(text:find('diffview.nvim found', 1, true), text)
  end)
end)
