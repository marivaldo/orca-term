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
---environment until `cleanup()`. The other XDG base directories are set inside that HOME too: an
---opencode the core starts makes its own directories under them (#35), never in the developer's.
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
  M.set_env('XDG_DATA_HOME', fixture.home .. '/.local/share')
  M.set_env('XDG_CACHE_HOME', fixture.home .. '/.cache')
  M.set_env('XDG_STATE_HOME', fixture.home .. '/.local/state')
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

---The fake `claude`, as in tests/support/agent.rs: answers `--version`, records its arguments in
---`$ORCA_TERM_FAKE_CLAUDE_LOG.argv`, says it is ready, echoes every line typed and records it in
---`$ORCA_TERM_FAKE_CLAUDE_LOG.input`, and ends on `SIGHUP` as claude does. The idle timeout ends a leaked one on its own. Bash builtins and
---absolute paths only: its PATH holds nothing else.
local FAKE_CLAUDE = {
  '#!/bin/bash',
  'if [ "$1" = "--version" ]; then echo "2.1.296 (Claude Code)"; exit 0; fi',
  'printf \'%s\\0\' "$@" > "$ORCA_TERM_FAKE_CLAUDE_LOG.argv"',
  'trap \'echo "got HUP"; exit 129\' HUP',
  'echo "fake claude ready"',
  'while IFS= read -r -t 120 line; do',
  '  echo "you said: $line"',
  '  printf \'%s\\n\' "$line" >> "$ORCA_TERM_FAKE_CLAUDE_LOG.input"',
  'done',
  'exit 0',
}

---@class spec.AgentRepo: spec.Repo
---@field bin string the one directory on PATH: the fake claude and sandbox tools, git and bash
---@field log string the prefix of the fake claude's log files (`<log>.argv`)

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
  repo.log = repo.tmp .. '/claude'
  M.set_env('ORCA_TERM_FAKE_CLAUDE_LOG', repo.log)
  M.set_env('PATH', repo.bin)
  table.insert(agent_repos, repo)
  return repo
end

---The arguments the fake claude of `repo` was last started with, or `nil` before it starts.
---@param repo spec.AgentRepo
---@return string[]?
function M.claude_argv(repo)
  local file = io.open(repo.log .. '.argv', 'rb')
  if not file then
    return nil
  end
  local raw = file:read('*a')
  file:close()
  return vim.split(raw, '\0', { plain = true, trimempty = true })
end

---The lines typed into the fake claude of `repo` so far, as its terminal handed them over (a
---bracketed paste keeps its escape sequences), or an empty list before anything is typed.
---@param repo spec.AgentRepo
---@return string[]
function M.claude_input(repo)
  local path = repo.log .. '.input'
  if vim.fn.filereadable(path) == 0 then
    return {}
  end
  return vim.fn.readfile(path)
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

---The ids of a recorded hook that tie it to its session and its turn.
---@class spec.HookIds
---@field session_id string
---@field prompt_id string

---The session and turn ids of the hook `index` (1-based) of the recording `fixture`.
---@param fixture string a file name in `M.HOOK_FIXTURES`
---@param index integer
---@return spec.HookIds
function M.hook_ids(fixture, index)
  local hooks = vim.tbl_filter(function(line)
    return line ~= ''
  end, vim.fn.readfile(M.HOOK_FIXTURES .. '/' .. fixture))
  local payload = vim.json.decode(assert(hooks[index]:match('^%S+ (.*)$')))
  return { session_id = payload.session_id, prompt_id = payload.prompt_id }
end

---Acts as claude does, as tests/hooks.rs does: plays the hooks `first` to `last` (1-based, both
---included; from the first and to the last when `nil`) of the recording `fixture` against the
---worktree at `worktree`, whose agent was started by the core. For each hook (a line: the event, a
---space, the payload), it runs every handler the worktree's `claude-settings.json` registers for
---the event whose matcher takes the payload's tool, exactly as written: the command with its
---arguments, the payload on stdin, the worktree as the directory. The recording's `{{worktree}}`,
---`{{transcripts}}` and `{{denied}}` become paths of the repository's own. With `as`, every hook
---takes those ids instead of its own (`M.hook_ids`), so the hooks of one recording land on the turn
---another recording left open. Fails the spec when a handler does not exit 0.
---@param repo spec.Repo
---@param worktree string
---@param fixture string a file name in `M.HOOK_FIXTURES`
---@param first? integer
---@param last? integer
---@param as? spec.HookIds
function M.play_hooks(repo, worktree, fixture, first, last, as)
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
    if as then
      local own = vim.json.decode(payload)
      payload = replace(payload, own.session_id, as.session_id)
      payload = replace(payload, own.prompt_id, as.prompt_id)
    end
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

---Commits the repository's `orca-term.yaml` with `lines` as its setup script: a script the core
---runs only once the person has approved it.
---@param repo spec.Repo
---@param lines string[]
function M.commit_setup(repo, lines)
  local yaml = { 'scripts:', '  setup: |' }
  for _, line in ipairs(lines) do
    table.insert(yaml, '    ' .. line)
  end
  vim.fn.writefile(yaml, repo.root .. '/orca-term.yaml')
  M.git(repo, { 'add', 'orca-term.yaml' })
  M.git(repo, { 'commit', '--quiet', '-m', 'setup script' })
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
---@field defaults string[] the answer each `vim.ui.input` offered to change, `''` for none
---@field shown string[][] the items each `vim.ui.select` offered, as shown to the person
---@field notes { message: string, level: integer }[] every notification, in order

---Answers the client's questions until `cleanup()`, and records them: `vim.ui.input` answers
---`answers.input`, and each `vim.ui.select` answers the next of `answers.select`, an item as shown
---(a select with no answer left, or whose answer is not offered, is cancelled). `vim.notify` only
---records.
---@param answers { input?: string, select?: string[] }
---@return spec.Ui
function M.stub_ui(answers)
  local ui = { prompts = {}, defaults = {}, shown = {}, notes = {} }
  local queue = vim.deepcopy(answers.select or {})
  ---@diagnostic disable-next-line: duplicate-set-field
  vim.ui.input = function(opts, on_confirm)
    table.insert(ui.prompts, opts.prompt)
    table.insert(ui.defaults, opts.default or '')
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

---Waits up to 30 seconds for `path` to exist; fails the spec if it never does.
---@param path string
local function wait_for_file(path)
  local appeared = vim.wait(30000, function()
    return vim.uv.fs_stat(path) ~= nil
  end, 50)
  assert(appeared, path .. ' never appeared')
end

---A setup script that says it started by creating `<dir>/started`, then waits for
---`<dir>/release` for at most 30 seconds, so a failing spec cannot hang.
---@param dir string
---@return string[]
local function blocking_script(dir)
  return {
    'touch ' .. dir .. '/started',
    'i=0',
    'while [ ! -e ' .. dir .. '/release ]; do',
    '  i=$((i + 1))',
    '  if [ "$i" -gt 600 ]; then exit 1; fi',
    '  sleep 0.05',
    'done',
  }
end

---A repository holding a worktree in every state the core reports today, unseen and seen, read
---by the real binary: the fleet of spec/golden/sidebar.txt and spec/golden/strip.txt. The agents
---are the fake claude of `agent_repo()`, and their states come from recordings of claude's hooks
---played through the handlers their settings register (`play_hooks`). A turn that ends leaves its
---worktree unseen; the core's `worktree seen` and `worktree unseen` clear and set the mark of
---others. Two worktrees have pending notes, one the denials of its latest turn, and one (`docs`)
---a merge in progress. The repository's directory is named `repo`.
---
---`install-deps` stays *setting up* until `release()` is called: call it in a `finally`, so its
---setup ends even when the spec fails.
---@return spec.AgentRepo repo
---@return fun() release ends the setup of `install-deps` and waits for it
function M.every_state_repo()
  local repo = M.agent_repo()
  -- The setup scripts below need `touch` and `sleep`, which the fake claude's PATH leaves out;
  -- its own directory stays first, so the fake claude is the one found.
  M.set_env('PATH', repo.bin .. ':/usr/bin:/bin')

  -- no_agent: plain `worktree new`. `spike` then moves to a branch of another name.
  for _, name in ipairs({ 'install-deps', 'docs', 'old-spike', 'spike' }) do
    M.run({ M.binary, 'worktree', 'new', name }, repo.root)
  end

  -- The agent states: an agent started by the core, then the hooks of a recording, played up to
  -- the hook that leaves the worktree in that state. A turn that ended leaves it unseen.
  local agents = {
    { name = 'run-tests', fixture = 'tui-queuetool-2.1.300.log', last = 3 }, -- working
    { name = 'pick-colour', fixture = 'tui-ask-2.1.300.log', last = 4 }, -- waiting
    { name = 'greet', fixture = 'p-hello-2.1.300.log' }, -- finished, unseen
    { name = 'api-call', fixture = 'p-stopfailure-2.1.300.log' }, -- failed, unseen
    { name = 'slow-job', fixture = 'tui-kill-2.1.300.log' }, -- interrupted, unseen
    { name = 'reviewed', fixture = 'p-hello-2.1.300.log' }, -- finished, then seen below
    { name = 'standby' }, -- idle: no turn yet
    { name = 'nudged' }, -- idle, then marked unseen below
  }
  for _, agent in ipairs(agents) do
    M.run({ M.binary, 'worktree', 'new', agent.name }, repo.root)
    M.run({ M.binary, 'agent', 'start', agent.name }, repo.root)
    if agent.fixture then
      local path = M.worktree_path(repo, agent.name)
      M.play_hooks(repo, path, agent.fixture, 1, agent.last)
    end
  end
  M.run({ M.binary, 'worktree', 'seen', 'reviewed' }, repo.root)
  M.run({ M.binary, 'worktree', 'unseen', 'nudged' }, repo.root)
  M.run({ 'git', 'switch', '--quiet', '-c', 'other-branch' }, M.worktree_path(repo, 'spike'))

  -- Denials: the three Write calls claude's permission settings refused in a recording of its own,
  -- played into the turn `pick-colour` waits in (the selected worktree, in the card). A batch never
  -- changes the turn's state nor its activity.
  local ask = M.hook_ids('tui-ask-2.1.300.log', 2)
  M.play_hooks(repo, M.worktree_path(repo, 'pick-colour'), 'tui-deny-multi-2.1.303.log', 3, 4, ask)

  -- Pending notes: two on `docs`, one on `pick-colour` (the selected worktree, in the card). A note
  -- on a whole file needs no file on disk.
  for _, note in ipairs({ { 'docs', 'one' }, { 'docs', 'two' }, { 'pick-colour', 'one' } }) do
    M.run({ M.binary, 'note', 'add', note[1], 'notes.txt', '--body=' .. note[2] }, repo.root)
  end

  -- A git operation in progress: a merge of `main` into `docs`, stopped before its commit, which
  -- leaves git's `MERGE_HEAD` as a conflicting merge does.
  local docs = M.worktree_path(repo, 'docs')
  M.run({ 'git', 'commit', '--quiet', '--allow-empty', '-m', 'docs' }, docs)
  M.git(repo, { 'commit', '--quiet', '--allow-empty', '-m', 'main moves on' })
  M.run({ 'git', 'merge', '--quiet', '--no-ff', '--no-commit', 'main' }, docs)

  -- not_ready: a setup that fails.
  M.set_setup(repo, { 'echo "npm ERR! missing script: build"', 'exit 1' })
  local failed = M.core(repo, { 'worktree', 'new', 'fix-login' })
  assert(failed.code == 1, 'the failing setup exited ' .. failed.code .. ': ' .. failed.stderr)

  -- broken: the worktree's directory is gone.
  vim.fn.delete(M.worktree_path(repo, 'old-spike'), 'rf')

  -- setting_up: a setup that waits for a file, run in the background.
  local signals = M.temp_dir()
  M.set_setup(repo, blocking_script(signals))
  local running = vim.system(
    { M.binary, 'worktree', 'setup', 'install-deps' },
    { cwd = repo.root, text = true }
  )
  local function release()
    vim.fn.writefile({}, signals .. '/release')
    running:wait(30000)
  end
  -- A spec that fails here has no `finally` yet: the setup is ended before the failure.
  local started, problem = pcall(wait_for_file, signals .. '/started')
  if not started then
    release()
    error(problem, 0)
  end
  return repo, release
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
