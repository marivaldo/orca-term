-- Seam 2: the status strip (`strip.lua`, and its lualine component) against the real binary.
--
-- The golden: for the fleet of every state the core reports today (`support.every_state_repo()`,
-- the fleet of spec/golden/sidebar.txt), spec/golden/strip.txt holds the strip as a statusline
-- expression, then the text and the highlights Neovim draws from it
-- (`nvim_eval_statusline(..., { highlights = true })`), each highlight as `<byte> <group>` (where
-- it starts, 0-based). After a deliberate change to the strip's look, rewrite it and review the
-- diff:
--
--     UPDATE_GOLDEN=1 just lua-test
--
-- The strip starts once and stays subscribed for the rest of a session, so in this Neovim every
-- spec forgets it afterwards (`forget_strip`), and the live and lualine specs run in child
-- Neovims, as spec/live_store_spec.lua does: embedded, driven over RPC, with only this client (and
-- lualine, from .rocks/lualine.nvim) loaded and the sidebar never opened.

local support = require('support')

local GOLDEN = support.root .. '/spec/golden/strip.txt'

---Where `just lua-rocks` puts lualine.nvim; `just lua-test` refuses to run without it.
local LUALINE = support.root .. '/.rocks/lualine.nvim'

---The strip, in the statusline expression the README gives.
local EXPRESSION = "%{%v:lua.require'orca-term.strip'.statusline()%}"

---The options the strip must never change.
local LAYOUT_OPTIONS = { 'statusline', 'tabline', 'winbar', 'laststatus', 'showtabline' }

---The child Neovims the spec started, with their socket paths, stopped after it.
---@type { channel: integer, socket: string? }[]
local children = {}

---The strip module, loaded afresh: as Neovim would have it before the first call.
---@return table
local function fresh_strip()
  package.loaded['orca-term.strip'] = nil
  return require('orca-term.strip')
end

---Forgets the strip in this Neovim: its subscription, its autocommands and its module, so the next
---spec starts from a Neovim where the strip never ran.
local function forget_strip()
  local live = require('orca-term.live')
  live.unsubscribe('strip')
  live.unsubscribe('spec')
  pcall(vim.api.nvim_del_augroup_by_name, 'orca-term-strip')
  package.loaded['orca-term.strip'] = nil
end

---Subscribes a spy to the live fleet, under `spec`, and returns what it got: `got.reads` is how
---many reads were kept since, `got.fleet` and `got.err` the last one.
---@return { reads: integer, fleet: orca_term.Fleet?, err: orca_term.Error? }
local function spy_on_reads()
  local got = { reads = 0 }
  require('orca-term.live').subscribe('spec', function(fleet, err)
    got.reads = got.reads + 1
    got.fleet, got.err = fleet, err
  end)
  return got
end

---Starts the strip of a fresh module, as a statusline does, and waits for its first read.
---@return table strip the strip module
---@return { reads: integer, fleet: orca_term.Fleet?, err: orca_term.Error? } got the spy's reads
local function start_strip()
  local strip = fresh_strip()
  local got = spy_on_reads()
  strip.statusline()
  support.wait_for(function()
    return got.reads > 0
  end, "the strip's first read")
  return strip, got
end

---The strip and what Neovim draws from it, in the golden file's format.
---@param line string the strip as a statusline expression
---@return string[]
local function snapshot(line)
  local drawn = vim.api.nvim_eval_statusline(line, { highlights = true })
  local out = { line, '', '# drawn:', drawn.str, '', '# highlights: <byte> <group>' }
  for _, highlight in ipairs(drawn.highlights) do
    table.insert(out, ('%d %s'):format(highlight.start, highlight.group))
  end
  return out
end

---Every option of `LAYOUT_OPTIONS`, global and local to the current window.
---@return table<string, any>
local function layout()
  local values = {}
  for _, name in ipairs(LAYOUT_OPTIONS) do
    values[name] = vim.api.nvim_get_option_value(name, { scope = 'global' })
    values[name .. ' (local)'] = vim.api.nvim_get_option_value(name, { scope = 'local' })
  end
  return values
end

---A child Neovim in the primary checkout of `repo`, with this client loaded and set up with the
---binary `cargo build` made; listening at a short socket path under /tmp when `listen` (so the
---core can reach it), and with lualine on its runtimepath when `lualine`. It is started by its full
---path: an agent repository's PATH holds no nvim.
---@param repo spec.Repo
---@param opts { listen?: boolean, lualine?: boolean }
---@return integer channel
---@return string? socket
local function start_child(repo, opts)
  local command = {
    vim.v.progpath,
    '--embed',
    '--headless',
    '--clean',
    '--cmd',
    'set runtimepath^=' .. vim.fn.fnameescape(support.root),
  }
  local socket = nil
  if opts.listen then
    socket = ('/tmp/ot-strip-%d-%d.sock'):format(vim.fn.getpid(), #children + 1)
    vim.fn.delete(socket)
    vim.list_extend(command, { '--listen', socket })
  end
  if opts.lualine then
    vim.list_extend(command, { '--cmd', 'set runtimepath+=' .. vim.fn.fnameescape(LUALINE) })
  end
  local channel = vim.fn.jobstart(command, { rpc = true, cwd = repo.root })
  assert(channel > 0, 'the child Neovim did not start')
  table.insert(children, { channel = channel, socket = socket })
  vim.rpcrequest(channel, 'nvim_exec_lua', "require('orca-term').setup({ binary = ... })", {
    support.binary,
  })
  return channel, socket
end

---Runs `code` in the child Neovim with `...` as its arguments and returns what it returns.
---@param child integer
---@param code string
---@param ... any
---@return any
local function child_lua(child, code, ...)
  return vim.rpcrequest(child, 'nvim_exec_lua', code, { ... })
end

---What the child Neovim draws from its global statusline: the text and the highlights.
---@param child integer
---@return { str: string, highlights: { start: integer, group: string }[] }
local function child_drawn(child)
  return child_lua(
    child,
    'return vim.api.nvim_eval_statusline(vim.o.statusline, { highlights = true })'
  )
end

---The lualine config the specs set up: only the strip, in `lualine_x`, over a fixed theme.
local LUALINE_SETUP = [[
  require('lualine').setup({
    options = { theme = 'gruvbox_dark', icons_enabled = false },
    sections = {
      lualine_a = {},
      lualine_b = {},
      lualine_c = {},
      lualine_x = { 'orca-term' },
      lualine_y = {},
      lualine_z = {},
    },
  })
]]

---The colours of the highlight group `name` in the child Neovim, its links followed.
---@param child integer
---@param name string
---@return { fg?: integer, bg?: integer }
local function child_colours(child, name)
  return vim.rpcrequest(child, 'nvim_get_hl', 0, { name = name, link = false }) or {}
end

---Whether lualine draws each item of the strip `text` in the foreground of its group over the
---background of `lualine_x` (with no `x` of its own, the theme draws it in the colours of its `c`),
---and why not.
---@param child integer
---@param items orca_term.StripItem[]
---@param text string the items' texts, joined by spaces
---@return boolean fits
---@return string? why
local function colours_fit(child, items, text)
  local drawn = child_drawn(child)
  local section_bg = child_lua(child, "return require('lualine.themes.gruvbox_dark').normal.c.bg")
  -- The strip ends one byte before the end: lualine pads it with a space on each side.
  local at = #drawn.str - #text - 1
  for _, item in ipairs(items) do
    -- The highlight drawing the item's first byte: the last one starting at or before it.
    local group = 'StatusLine'
    for _, highlight in ipairs(drawn.highlights) do
      if highlight.start <= at then
        group = highlight.group
      end
    end
    local wanted = child_colours(child, item.group)
    local got = child_colours(child, group)
    if wanted.fg == nil or wanted.fg ~= got.fg then
      return false,
        ('%s is drawn in %s, not in the colour of %s'):format(item.text, group, item.group)
    end
    if got.bg ~= tonumber(section_bg:sub(2), 16) then
      return false, ("%s is not over the section's background"):format(item.text)
    end
    at = at + #item.text + 1
  end
  return true
end

describe('the status strip', function()
  after_each(function()
    for _, child in ipairs(children) do
      vim.fn.jobstop(child.channel)
      vim.fn.jobwait({ child.channel }, 5000)
      if child.socket then
        vim.fn.delete(child.socket)
      end
    end
    children = {}
    forget_strip()
    support.cleanup()
  end)

  it('golden-checks the strip of every state the core reports today, and its counts', function()
    support.load_client()
    local repo, release = support.every_state_repo()
    finally(release)
    support.cd(repo.root)

    local strip, got = start_strip()
    assert.is_nil(got.err)
    local actual = snapshot(strip.statusline())

    if os.getenv('UPDATE_GOLDEN') == '1' then
      vim.fn.writefile(actual, GOLDEN)
    end
    assert(
      vim.uv.fs_stat(GOLDEN),
      'no ' .. GOLDEN .. '; write it with UPDATE_GOLDEN=1 just lua-test'
    )
    assert.are.same(vim.fn.readfile(GOLDEN), actual)

    assert.are.same({
      states = {
        { state = 'waiting', count = 1 },
        { state = 'failed', count = 1 },
        { state = 'interrupted', count = 1 },
        { state = 'broken', count = 1 },
        { state = 'not_ready', count = 1 },
        { state = 'working', count = 1 },
        { state = 'setting_up', count = 1 },
        { state = 'finished', count = 2 },
        { state = 'idle', count = 2 },
        { state = 'no_agent', count = 2 },
      },
      unseen = 4,
    }, strip.counts())
  end)

  it('is the empty string when there is no worktree', function()
    support.load_client()
    local repo = support.repo()
    support.cd(repo.root)

    local strip, got = start_strip()

    assert.is_not_nil(got.fleet)
    assert.are.equal('', strip.statusline())
    assert.are.same({ states = {}, unseen = 0 }, strip.counts())
  end)

  it('is the empty string outside a git repository, showing no error', function()
    support.load_client()
    support.isolate()
    support.cd(support.temp_dir())

    local strip, got = start_strip()

    assert.is_not_nil(got.err)
    assert.are.equal('', strip.statusline())
  end)

  it('returns at once on the first call, and reads the fleet only after a schedule', function()
    support.load_client()
    local repo = support.repo()
    support.run({ support.binary, 'worktree', 'new', 'demo' }, repo.root)
    support.cd(repo.root)
    local core = require('orca-term.core')
    local real = { worktree_ls = core.worktree_ls, worktree_ls_async = core.worktree_ls_async }
    local reads = 0
    for name, read in pairs(real) do
      core[name] = function(...)
        reads = reads + 1
        return read(...)
      end
    end
    finally(function()
      for name, read in pairs(real) do
        core[name] = read
      end
    end)
    local strip = fresh_strip()

    assert.are.equal('', strip.statusline())
    assert.are.equal(0, reads)

    support.wait_for(function()
      return reads > 0 and strip.statusline() == '%#OrcaTermStateNoAgent#·1%*'
    end, 'the strip to show the fleet')
    assert.are.equal(1, reads)
  end)

  it('starts from inside a statusline being drawn, without an error', function()
    support.load_client()
    local repo = support.repo()
    support.run({ support.binary, 'worktree', 'new', 'demo' }, repo.root)
    support.cd(repo.root)
    fresh_strip()
    vim.v.errmsg = ''

    local first = vim.api.nvim_eval_statusline('[' .. EXPRESSION .. ']', {})
    assert.are.equal('[]', first.str)

    support.wait_for(function()
      return vim.api.nvim_eval_statusline('[' .. EXPRESSION .. ']', {}).str == '[·1]'
    end, 'the drawn statusline to show the strip')
    assert.are.equal('', vim.v.errmsg)
  end)

  it('changes none of statusline, tabline, winbar, laststatus and showtabline', function()
    support.load_client()
    local repo = support.repo()
    support.run({ support.binary, 'worktree', 'new', 'demo' }, repo.root)
    support.cd(repo.root)
    local before = layout()

    local strip = start_strip()
    assert.are.equal('%#OrcaTermStateNoAgent#·1%*', strip.statusline())

    assert.are.same(before, layout())
  end)

  it("shows the core's write in a Neovim that never opened the sidebar", function()
    support.load_client()
    local repo = support.repo()
    local child, socket = start_child(repo, { listen = true })
    child_lua(child, 'vim.o.statusline = ...', EXPRESSION)

    -- Drawing the statusline starts the strip; nothing else is asked of the child.
    assert.are.equal('', child_drawn(child).str)
    support.wait_for(function()
      return vim.tbl_contains(support.registry(repo), socket)
    end, 'the child in the registry')

    support.run({ support.binary, 'worktree', 'new', 'x' }, repo.root)

    support.wait_for(function()
      return child_drawn(child).str == '·1'
    end, "the child's statusline to show the new worktree")
    assert.is_true(child_lua(child, "return package.loaded['orca-term.sidebar'] == nil"))
  end)

  it("draws in lualine as the strip, each item in its group's colour over the section's", function()
    support.load_client()
    local repo, release = support.every_state_repo()
    finally(release)
    local child = start_child(repo, { lualine = true })
    child_lua(child, LUALINE_SETUP)

    ---@type orca_term.StripItem[]
    local items = {}
    local text = ''
    support.wait_for(function()
      items = child_lua(child, "return require('orca-term.strip').items()")
      text = table.concat(
        vim.tbl_map(function(item)
          return item.text
        end, items),
        ' '
      )
      return text ~= '' and vim.trim(child_drawn(child).str) == text
    end, 'lualine to draw the strip')
    assert.are.equal('?1 ●1 ●1 !1 △1 ◐1 ◌1 ●2 ○2 ·2 •4', text)

    local drawn = child_drawn(child)
    -- lualine pads the component with a space on each side.
    assert.is_truthy(vim.endswith(drawn.str, ' ' .. text .. ' '))
    local fits, why = colours_fit(child, items, text)
    assert(fits, why)

    -- After a `:colorscheme`, the items take the new colours of their groups.
    local before =
      vim.rpcrequest(child, 'nvim_get_hl', 0, { name = 'OrcaTermStateWaiting', link = false })
    child_lua(child, "vim.cmd.colorscheme('habamax')")
    assert.are_not.same(
      before,
      vim.rpcrequest(child, 'nvim_get_hl', 0, { name = 'OrcaTermStateWaiting', link = false })
    )
    support.wait_for(function()
      return (colours_fit(child, items, text))
    end, 'the items to take the colours of the new colorscheme')
  end)

  it("hides in lualine when there is no worktree, and leaves lualine's config alone", function()
    support.load_client()
    local repo = support.repo()
    local child = start_child(repo, { lualine = true })
    child_lua(child, LUALINE_SETUP)
    local config = child_lua(child, "return vim.inspect(require('lualine').get_config())")

    support.wait_for(function()
      return child_lua(child, "return require('orca-term.live').fleet() ~= nil")
    end, "the strip's first read in the child")
    child_lua(child, "require('lualine').refresh({ force = true })")

    local drawn = child_drawn(child)
    assert.are.equal('', vim.trim(drawn.str))
    for _, highlight in ipairs(drawn.highlights) do
      assert(not highlight.group:find('orca-term', 1, true), highlight.group)
    end
    assert.are.equal(
      config,
      child_lua(child, "return vim.inspect(require('lualine').get_config())")
    )
  end)
end)
