-- Throwaway git repositories for the specs, and the real core binary `cargo build` made.
--
-- Mirrors tests/support/mod.rs: every git process, the helper's and the core's, reads a gitconfig
-- of the fixture's own, and the core gets a HOME and an XDG_CONFIG_HOME inside the fixture, so
-- the specs never read the developer's config. The variables are set in this Neovim, whose
-- children inherit them, and `cleanup()` puts them back.
--
-- `agent_repo()` mirrors tests/support/agent.rs: a fake `claude` and fake sandbox tools on a PATH
-- of their own, so the agent specs need no claude, no sandbox and no network. `cleanup()` stops
-- every agent a spec left running.

local M = {}

---The repository root: one directory up from this file, `<root>/spec/support.lua`.
M.root = vim.fn.fnamemodify(debug.getinfo(1, 'S').source:sub(2), ':p:h:h')

---The core binary `cargo build` made. `just lua-test` builds it before running the specs.
M.binary = M.root .. '/target/debug/orca-term'

assert(
  vim.fn.executable(M.binary) == 1,
  'no core binary at ' .. M.binary .. '; run `cargo build` first, or `just lua-test`'
)

---@type table<string, { value: string? }>
local saved_env = {}
---The directory Neovim was in before `cd()`, put back by `cleanup()`.
---@type string?
local saved_cwd = nil
---@type string[]
local temp_dirs = {}
---The functions `stub_ui()` replaced, put back by `cleanup()`.
local real_ui = { input = vim.ui.input, select = vim.ui.select, notify = vim.notify }
---The repositories `agent_repo()` made, whose agents `cleanup()` stops.
---@type spec.Repo[]
local agent_repos = {}

---Sets an environment variable of this Neovim and its children until `cleanup()`.
---@param name string
---@param value string
function M.set_env(name, value)
  if saved_env[name] == nil then
    saved_env[name] = { value = vim.env[name] }
  end
  vim.env[name] = value
end

---Runs `command` in `cwd` and returns its stdout; fails the spec when it exits non-zero.
---@param command string[]
---@param cwd? string
---@return string
function M.run(command, cwd)
  local result = vim.system(command, { cwd = cwd, text = true }):wait()
  assert(
    result.code == 0,
    ('`%s` exited %d: %s'):format(table.concat(command, ' '), result.code, result.stderr)
  )
  return result.stdout or ''
end

---A new empty directory, removed by `cleanup()`. Its path has no symlinks in it, as the paths
---the core prints have none (on macOS the temporary directory is under the symlink /var).
---@return string
function M.temp_dir()
  local dir = vim.fn.tempname()
  vim.fn.mkdir(dir, 'p')
  dir = assert(vim.uv.fs_realpath(dir))
  table.insert(temp_dirs, dir)
  return dir
end

---Writes an executable shell script at `path`.
---@param path string
---@param lines string[] the script's lines, after `#!/bin/sh`
function M.write_script(path, lines)
  vim.fn.writefile(vim.list_extend({ '#!/bin/sh' }, lines), path)
  assert(vim.uv.fs_chmod(path, tonumber('755', 8)))
end

---@class spec.Fixture
---@field tmp string the fixture's own directory
---@field home string the core's HOME
---@field xdg string the core's XDG_CONFIG_HOME

---A fixture directory with a gitconfig of its own, a HOME and an XDG_CONFIG_HOME, set in the
---environment until `cleanup()`.
---@return spec.Fixture
function M.isolate()
  local tmp = M.temp_dir()
  local gitconfig = tmp .. '/gitconfig'
  vim.fn.writefile({
    '[user]',
    '\tname = Fixture',
    '\temail = fixture@example.invalid',
    '[init]',
    '\tdefaultBranch = main',
  }, gitconfig)
  local fixture = { tmp = tmp, home = tmp .. '/home', xdg = tmp .. '/xdg' }
  vim.fn.mkdir(fixture.home, 'p')
  vim.fn.mkdir(fixture.xdg, 'p')
  M.set_env('GIT_CONFIG_GLOBAL', gitconfig)
  M.set_env('GIT_CONFIG_NOSYSTEM', '1')
  M.set_env('HOME', fixture.home)
  M.set_env('XDG_CONFIG_HOME', fixture.xdg)
  return fixture
end

---@class spec.Repo: spec.Fixture
---@field root string the primary checkout

---A repository with one commit on `main`, at `<tmp>/repo`, in an isolated fixture.
---@return spec.Repo
function M.repo()
  local repo = M.isolate() --[[@as spec.Repo]]
  repo.root = repo.tmp .. '/repo'
  vim.fn.mkdir(repo.root, 'p')
  M.git(repo, { 'init', '--quiet' })
  M.git(repo, { 'commit', '--quiet', '--allow-empty', '-m', 'init' })
  return repo
end

---Runs git in the repository's primary checkout and returns its stdout.
---@param repo spec.Repo
---@param args string[]
---@return string
function M.git(repo, args)
  return M.run(vim.list_extend({ 'git', '-C', repo.root }, args))
end

---Adds a worktree by hand, as a person would with plain git, on a new branch.
---@param repo spec.Repo
---@param path string
---@param branch string
---@return string path the worktree's path, with no symlinks in it
function M.add_worktree(repo, path, branch)
  M.git(repo, { 'worktree', 'add', '--quiet', '-b', branch, path })
  return assert(vim.uv.fs_realpath(path))
end

---The fake sandbox tools, one set for each OS, all present so the specs run the same way on macOS
---and Linux.
M.SANDBOX_TOOLS = { 'sandbox-exec', 'bwrap', 'socat' }

---The fake `claude`, as in tests/support/agent.rs: answers `--version`, says it is ready, echoes
---every line typed, and ends on `SIGHUP` as claude does. The idle timeout ends a leaked one on its
---own. Bash builtins and absolute paths only: its PATH holds nothing else.
local FAKE_CLAUDE = {
  '#!/bin/bash',
  'if [ "$1" = "--version" ]; then echo "2.1.296 (Claude Code)"; exit 0; fi',
  'trap \'echo "got HUP"; exit 129\' HUP',
  'echo "fake claude ready"',
  'while IFS= read -r -t 120 line; do',
  '  echo "you said: $line"',
  'done',
  'exit 0',
}

---@class spec.AgentRepo: spec.Repo
---@field bin string the one directory on PATH: the fake claude and sandbox tools, git and bash

---A repository whose agents are the fake claude: PATH, for this Neovim and its children, is one
---directory holding the fake `claude`, the fake sandbox tools and links to the real `git` and
---`bash`. A child Neovim must then be started by its full path, `vim.v.progpath`.
---@return spec.AgentRepo
function M.agent_repo()
  local repo = M.repo() --[[@as spec.AgentRepo]]
  repo.bin = repo.tmp .. '/bin'
  vim.fn.mkdir(repo.bin, 'p')
  for _, tool in ipairs({ 'git', 'bash' }) do
    local real = vim.fn.exepath(tool)
    assert(real ~= '', tool .. ' is not on PATH')
    assert(vim.uv.fs_symlink(real, repo.bin .. '/' .. tool))
  end
  for _, tool in ipairs(M.SANDBOX_TOOLS) do
    M.write_script(repo.bin .. '/' .. tool, { 'exit 0' })
  end
  local claude = repo.bin .. '/claude'
  vim.fn.writefile(FAKE_CLAUDE, claude)
  assert(vim.uv.fs_chmod(claude, tonumber('755', 8)))
  M.set_env('PATH', repo.bin)
  table.insert(agent_repos, repo)
  return repo
end

---Removes the fake sandbox tools from the PATH of `repo`, so starting an agent fails as
---`no_sandbox`.
---@param repo spec.AgentRepo
function M.remove_sandbox(repo)
  for _, tool in ipairs(M.SANDBOX_TOOLS) do
    vim.fn.delete(repo.bin .. '/' .. tool)
  end
end

---The worktree named `name`, as `worktree ls --json` prints it.
---@param repo spec.Repo
---@param name string
---@return table
function M.listed(repo, name)
  local fleet = vim.json.decode(M.run({ M.binary, 'worktree', 'ls', '--json' }, repo.root))
  for _, worktree in ipairs(fleet.worktrees) do
    if worktree.name == name then
      return worktree
    end
  end
  error('no worktree named ' .. name)
end

---Where the recordings of claude's hooks are, shared with tests/hooks.rs.
M.HOOK_FIXTURES = M.root .. '/tests/fixtures/claude-hooks'

---`text` with every `from` replaced by `to`, both taken as plain text (not gsub patterns).
---@param text string
---@param from string
---@param to string
---@return string
local function replace(text, from, to)
  local out = ''
  local start = 1
  while true do
    local first, last = text:find(from, start, true)
    if not first then
      return out .. text:sub(start)
    end
    out = out .. text:sub(start, first - 1) .. to
    start = last + 1
  end
end

---Acts as claude does, as tests/hooks.rs does: plays the hooks `first` to `last` (1-based, both
---included; from the first and to the last when `nil`) of the recording `fixture` against the
---worktree at `worktree`, whose agent was started by the core. For each hook (a line: the event, a space, the payload), it runs every handler the
---worktree's `claude-settings.json` registers for the event whose matcher takes the payload's
---tool, exactly as written: the command with its arguments, the payload on stdin, the worktree as
---the directory. The recording's `{{worktree}}`, `{{transcripts}}` and `{{denied}}` become paths of
---the repository's own. Fails the spec when a handler does not exit 0.
---@param repo spec.Repo
---@param worktree string
---@param fixture string a file name in `M.HOOK_FIXTURES`
---@param first? integer
---@param last? integer
function M.play_hooks(repo, worktree, fixture, first, last)
  local admin = vim.trim(M.run({
    'git',
    '-C',
    worktree,
    'rev-parse',
    '--path-format=absolute',
    '--git-path',
    'orca-term',
  }))
  local settings = vim.json.decode(table.concat(vim.fn.readfile(admin .. '/claude-settings.json')))
  local hooks = vim.tbl_filter(function(line)
    return line ~= ''
  end, vim.fn.readfile(M.HOOK_FIXTURES .. '/' .. fixture))
  for index = first or 1, last or #hooks do
    local event, payload = hooks[index]:match('^(%S+) (.*)$')
    payload = replace(payload, '{{worktree}}', worktree)
    payload = replace(payload, '{{transcripts}}', repo.tmp .. '/transcripts')
    payload = replace(payload, '{{denied}}', repo.tmp .. '/denied')
    local tool = vim.json.decode(payload).tool_name
    for _, entry in ipairs(settings.hooks[event] or {}) do
      local matcher = entry.matcher or '*'
      if matcher == '*' or matcher == '' or matcher == tool then
        for _, handler in ipairs(entry.hooks) do
          local command = vim.list_extend({ handler.command }, handler.args)
          local result =
            vim.system(command, { stdin = payload, cwd = worktree, text = true }):wait()
          assert(
            result.code == 0,
            ('%s: the %s hook exited %d: %s'):format(fixture, event, result.code, result.stderr)
          )
        end
      end
    end
  end
end

---Stops every agent of `repo` whose holder is alive, so no spec leaves a process behind. Never
---fails: it runs while a failed spec is cleaned up.
---@param repo spec.Repo
local function stop_agents(repo)
  local ls = M.core(repo, { 'worktree', 'ls', '--json' })
  local decoded, fleet = pcall(vim.json.decode, ls.stdout or '')
  if not decoded or type(fleet) ~= 'table' then
    return
  end
  for _, worktree in ipairs(fleet.worktrees or {}) do
    if worktree.holder == true then
      M.core(repo, { 'agent', 'stop', worktree.path })
    end
  end
end

---Runs the core binary with `args` in the repository's primary checkout, without checking how it
---exits, and returns what `vim.system` reports.
---@param repo spec.Repo
---@param args string[]
---@return vim.SystemCompleted
function M.core(repo, args)
  return vim.system(vim.list_extend({ M.binary }, args), { cwd = repo.root, text = true }):wait()
end

---Writes the repository's uncommitted config override, `.git/orca-term.yaml`, with `lines` as its
---setup script; no lines removes it.
---@param repo spec.Repo
---@param lines string[]
function M.set_setup(repo, lines)
  local path = repo.root .. '/.git/orca-term.yaml'
  if #lines == 0 then
    vim.fn.delete(path)
    return
  end
  local yaml = { 'scripts:', '  setup: |' }
  for _, line in ipairs(lines) do
    table.insert(yaml, '    ' .. line)
  end
  vim.fn.writefile(yaml, path)
end

---The path of the worktree named `name`, as `worktree ls --json` prints it.
---@param repo spec.Repo
---@param name string
---@return string
function M.worktree_path(repo, name)
  return M.listed(repo, name).path
end

---The addresses of the running Neovims registered with the repository's core, sorted: what the
---files of its registry, `<git common dir>/orca-term/nvim/`, hold.
---@param repo spec.Repo
---@return string[]
function M.registry(repo)
  local dir = repo.root .. '/.git/orca-term/nvim'
  local addresses = {}
  if vim.fn.isdirectory(dir) == 0 then
    return addresses
  end
  for _, name in ipairs(vim.fn.readdir(dir)) do
    -- A registry file is named with 16 hex digits; anything else is a file being written.
    if name:match('^%x+$') and #name == 16 then
      local address = table.concat(vim.fn.readfile(dir .. '/' .. name), '\n')
      table.insert(addresses, vim.trim(address))
    end
  end
  table.sort(addresses)
  return addresses
end

---Makes `dir` Neovim's current directory until `cleanup()`.
---@param dir string
function M.cd(dir)
  saved_cwd = saved_cwd or vim.fn.getcwd()
  vim.cmd.cd(vim.fn.fnameescape(dir))
end

---Loads the client the way Neovim does at startup, then calls setup() with the binary `cargo
---build` made, so a stale release build is never picked up. The spec's own Neovim stops listening,
---so it never registers with the core and the core's events never redraw its sidebar behind a
---spec's back; the live updates are covered in child Neovims, by spec/live_spec.lua.
function M.load_client()
  for _, address in ipairs(vim.fn.serverlist()) do
    vim.fn.serverstop(address)
  end
  vim.opt.runtimepath:prepend(M.root)
  vim.cmd.runtime('plugin/orca-term.lua')
  require('orca-term').setup({ binary = M.binary })
end

---Closes the sidebar if it is open, then every tab and window but one, and wipes every agent
---terminal (which detaches it), so the next spec starts from a single empty window.
function M.reset_editor()
  local sidebar = require('orca-term.sidebar')
  if sidebar.is_open() then
    sidebar.close()
  end
  vim.cmd('silent! tabonly!')
  vim.cmd('silent! only!')
  vim.cmd('enew!')
  local mark = require('orca-term.terminal').MARK
  for _, buf in ipairs(vim.api.nvim_list_bufs()) do
    if vim.b[buf][mark] then
      vim.api.nvim_buf_delete(buf, { force = true })
    end
  end
end

local MODES = { 'n', 'v', 'x', 's', 'o', 'i', 'c', 't' }

---Every global keymap, as `<mode> <lhs>`, sorted. `<Plug>` maps are left out: nobody can type
---them, they only exist for a person to bind a key of their own to.
---@return string[]
function M.global_keymaps()
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

---What the stubbed `vim.ui` and `vim.notify` saw.
---@class spec.Ui
---@field prompts string[] every prompt asked, in order
---@field shown string[][] the items each `vim.ui.select` offered, as shown to the person
---@field notes { message: string, level: integer }[] every notification, in order

---Answers the client's questions until `cleanup()`, and records them: `vim.ui.input` answers
---`answers.input`, and each `vim.ui.select` answers the next of `answers.select`, an item as shown
---(a select with no answer left, or whose answer is not offered, is cancelled). `vim.notify` only
---records.
---@param answers { input?: string, select?: string[] }
---@return spec.Ui
function M.stub_ui(answers)
  local ui = { prompts = {}, shown = {}, notes = {} }
  local queue = vim.deepcopy(answers.select or {})
  ---@diagnostic disable-next-line: duplicate-set-field
  vim.ui.input = function(opts, on_confirm)
    table.insert(ui.prompts, opts.prompt)
    on_confirm(answers.input)
  end
  ---@diagnostic disable-next-line: duplicate-set-field
  vim.ui.select = function(items, opts, on_choice)
    table.insert(ui.prompts, opts.prompt)
    local wanted = table.remove(queue, 1)
    local shown = {}
    local chosen, index = nil, nil
    for i, item in ipairs(items) do
      shown[i] = opts.format_item and opts.format_item(item) or item
      if shown[i] == wanted and not chosen then
        chosen, index = item, i
      end
    end
    table.insert(ui.shown, shown)
    on_choice(chosen, index)
  end
  ---@diagnostic disable-next-line: duplicate-set-field
  vim.notify = function(message, level)
    table.insert(ui.notes, { message = message, level = level })
  end
  return ui
end

---Waits, letting Neovim's event loop run, until `condition()` holds; fails the spec after 20 s.
---@param condition fun(): boolean
---@param what string what is awaited, for the failure
function M.wait_for(condition, what)
  assert(vim.wait(20000, condition, 20), 'timed out waiting for ' .. what)
end

---Stops every agent a spec started, puts the environment, the current directory and the client's
---settings back, and removes every temporary directory.
function M.cleanup()
  for _, repo in ipairs(agent_repos) do
    stop_agents(repo)
  end
  agent_repos = {}
  if saved_cwd then
    vim.cmd.cd(vim.fn.fnameescape(saved_cwd))
    saved_cwd = nil
  end
  for name, saved in pairs(saved_env) do
    vim.env[name] = saved.value
  end
  saved_env = {}
  for _, dir in ipairs(temp_dirs) do
    vim.fn.delete(dir, 'rf')
  end
  temp_dirs = {}
  vim.ui.input, vim.ui.select, vim.notify = real_ui.input, real_ui.select, real_ui.notify
  require('orca-term').setup()
end

return M
