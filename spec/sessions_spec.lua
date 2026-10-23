-- Seam 2: a worktree's sessions from the sidebar (#41), in headless Neovim against the real binary,
-- the fake claude of `support.agent_repo()` and a fake opencode, with fake sandbox tools that run
-- what they wrap. `s` lists the sessions of the worktree under the cursor and resumes the one
-- chosen through the core (`session resume`), showing its agent in the worktree's tab in a terminal
-- attached again, swapping the agent for a session of the other one; `.` then `New session` asks
-- first, then starts a new session (`session new`). A change the core refuses (an agent busy in a
-- turn) is told with its reason. `vim.ui` and `vim.notify` are stubbed; the core never is.

local support = require('support')
local actions = require('orca-term.actions')
local sidebar = require('orca-term.sidebar')
local terminal = require('orca-term.terminal')

---The fake `opencode`: answers `--version`, lists the session `ses_1` to `session list`, records
---its arguments, says it is ready, echoes every line typed, and ends on `SIGHUP`. Bash builtins
---only: its PATH holds nothing else.
local FAKE_OPENCODE = {
  '#!/bin/bash',
  'if [ "$1" = "--version" ]; then echo "1.18.0"; exit 0; fi',
  'if [ "$1" = "session" ]; then echo \'[{"id":"ses_1","title":"add tests"}]\'; exit 0; fi',
  'printf \'%s\\0\' "$@" > "$ORCA_TERM_FAKE_CLAUDE_LOG.opencode-argv"',
  "trap 'exit 129' HUP",
  'echo "fake opencode ready"',
  'while IFS= read -r -t 120 line; do echo "you said: $line"; done',
  'exit 0',
}

---The fake `sandbox-exec`, which runs what follows `-f <profile>`.
local FAKE_SANDBOX_EXEC = { '#!/bin/bash', 'shift 2', 'exec "$@"' }

---The fake `bwrap`, which runs what follows `--`.
local FAKE_BWRAP = {
  '#!/bin/bash',
  'while [ "$#" -gt 0 ] && [ "$1" != "--" ]; do shift; done',
  'shift',
  'exec "$@"',
}

---The two claude sessions the specs play, each one turn of the recording `p-hello`.
local FIRST = { session_id = '0b983a87-4074-466e-b864-422ce6f063c8', prompt_id = 'p-first' }
local SECOND = { session_id = '5d1c2e9f-8a7b-4c3d-9e2f-1a0b9c8d7e6f', prompt_id = 'p-second' }

---How the list of sessions shows a claude session of one `hello there` turn, current or not.
local CURRENT_HELLO = '* claude-code  1 turn  just now  hello there'
local OLD_HELLO = '  claude-code  1 turn  just now  hello there'

---Writes the executable `name` on the PATH of `repo`, with `lines`.
---@param repo spec.AgentRepo
---@param name string
---@param lines string[]
local function write_tool(repo, name, lines)
  local path = repo.bin .. '/' .. name
  vim.fn.delete(path)
  vim.fn.writefile(lines, path)
  assert(vim.uv.fs_chmod(path, tonumber('755', 8)))
end

---Presses `key` in the sidebar of the current tab, on the line of the worktree `name`.
---@param key string
---@param name string
local function press_on(key, name)
  for _, win in ipairs(vim.api.nvim_tabpage_list_wins(0)) do
    if vim.api.nvim_win_get_buf(win) == sidebar.buffer() then
      vim.api.nvim_set_current_win(win)
      for index, line in ipairs(vim.api.nvim_buf_get_lines(0, 0, -1, false)) do
        if line:find(' ' .. name, 1, true) then
          vim.api.nvim_win_set_cursor(win, { index, 0 })
          vim.cmd('normal ' .. vim.keycode(key))
          return
        end
      end
      error('no sidebar line for ' .. name)
    end
  end
  error('no sidebar in the current tab')
end

---Waits until the attached agent terminal of the worktree at `path` shows `text`, in a buffer
---other than `old`, and returns it.
---@param path string
---@param text string
---@param old? integer
---@return integer buf
local function wait_for_terminal_showing(path, text, old)
  support.wait_for(function()
    local buf = terminal.find(path)
    if not buf or buf == old or not terminal.running_job(buf) then
      return false
    end
    for _, line in ipairs(vim.api.nvim_buf_get_lines(buf, 0, -1, false)) do
      if line:find(text, 1, true) then
        return true
      end
    end
    return false
  end, '`' .. text .. '` on the agent terminal of ' .. path)
  return assert(terminal.find(path))
end

---The admin directory of the worktree at `worktree`.
---@param worktree string
---@return string
local function admin_dir(worktree)
  return vim.trim(support.run({
    'git',
    '-C',
    worktree,
    'rev-parse',
    '--path-format=absolute',
    '--git-path',
    'orca-term',
  }))
end

---Writes the transcript claude keeps the session `ids` in, as it does once the session has a turn.
---@param repo spec.AgentRepo
---@param ids spec.HookIds
local function write_transcript(repo, ids)
  vim.fn.mkdir(repo.tmp .. '/transcripts', 'p')
  vim.fn.writefile({ '{}' }, repo.tmp .. '/transcripts/' .. ids.session_id .. '.jsonl')
end

---Writes the plugin's `line` as opencode's events pipe would, through the hook the holder runs.
---@param worktree string
---@param line table
local function opencode_event(worktree, line)
  local command = { support.binary, 'hook', 'opencode', '--admin', admin_dir(worktree) }
  local result = vim.system(command, { stdin = vim.json.encode(line), cwd = worktree }):wait()
  assert(result.code == 0, result.stderr)
end

---The sessions of `demo` as `session ls --json` lists them: `id` and whether current.
---@param repo spec.AgentRepo
---@return string[]
local function sessions(repo)
  local listed =
    vim.json.decode(support.run({ support.binary, 'session', 'ls', 'demo', '--json' }, repo.root))
  return vim.tbl_map(function(session)
    return session.id .. (session.current and ' (current)' or '')
  end, listed.sessions)
end

---A repository whose agents are the fake claude and the fake opencode, with the worktree `demo`,
---whose claude the core started, and its claude sessions `FIRST` then `SECOND`, the current one,
---both with their transcripts. Neovim is in the primary checkout, the sidebar open.
---@return spec.AgentRepo repo
---@return string demo the worktree's path
local function two_sessions_on_demo()
  support.load_client()
  local repo = support.agent_repo()
  write_tool(repo, 'opencode', FAKE_OPENCODE)
  write_tool(repo, 'sandbox-exec', FAKE_SANDBOX_EXEC)
  write_tool(repo, 'bwrap', FAKE_BWRAP)
  support.run({ support.binary, 'worktree', 'new', 'demo' }, repo.root)
  support.run({ support.binary, 'agent', 'start', 'demo' }, repo.root)
  local demo = support.worktree_path(repo, 'demo')
  support.play_hooks(repo, demo, 'p-hello-2.1.300.log', 2, 3, FIRST)
  support.play_hooks(repo, demo, 'p-hello-2.1.300.log', 2, 3, SECOND)
  write_transcript(repo, FIRST)
  write_transcript(repo, SECOND)
  support.cd(repo.root)
  vim.cmd('OrcaTerm sidebar')
  return repo, demo
end

describe('Sessions', function()
  local columns = vim.o.columns

  before_each(function()
    vim.o.columns = 160
  end)

  after_each(function()
    support.reset_editor()
    support.cleanup()
    vim.o.columns = columns
  end)

  it('s lists the sessions and resumes the one chosen, attaching the terminal again', function()
    local repo, demo = two_sessions_on_demo()
    support.stub_ui({})
    press_on('<CR>', 'demo')
    local before = wait_for_terminal_showing(demo, 'fake claude ready')

    local ui = support.stub_ui({ select = { OLD_HELLO } })
    press_on('s', 'demo')

    local after = wait_for_terminal_showing(demo, 'fake claude ready', before)
    assert.are.equal('Sessions of demo', ui.prompts[1])
    assert.are.same({ { CURRENT_HELLO, OLD_HELLO } }, ui.shown)
    assert.are.same(
      { '--resume', FIRST.session_id },
      vim.list_slice(assert(support.claude_argv(repo)), 5)
    )
    assert(not vim.api.nvim_buf_is_valid(before), 'the old terminal is gone')
    assert.are.equal(after, vim.api.nvim_get_current_buf(), 'shown in the current window')
    assert.are.equal(demo, vim.fn.getcwd(-1, vim.fn.tabpagenr()), "in the worktree's tab")
    assert.are.same(
      { SECOND.session_id, FIRST.session_id .. ' (current)' },
      sessions(repo),
      'the most recently active first'
    )
    assert.are.same({}, ui.notes)
  end)

  it('choosing the current session only goes to it', function()
    local repo, demo = two_sessions_on_demo()
    local ui = support.stub_ui({ select = { CURRENT_HELLO } })

    press_on('s', 'demo')

    wait_for_terminal_showing(demo, 'fake claude ready')
    assert.are.equal(demo, vim.fn.getcwd(-1, vim.fn.tabpagenr()), "in the worktree's tab")
    assert.are.same(
      {},
      vim.list_slice(assert(support.claude_argv(repo)), 5),
      'the agent started first, on no session, was not restarted'
    )
    assert.are.same({}, ui.notes)
  end)

  it("resuming the other agent's session swaps the agent", function()
    local repo, demo = two_sessions_on_demo()
    opencode_event(
      demo,
      { type = 'prompt', session = 'ses_1', parent = vim.NIL, text = 'add tests' }
    )
    opencode_event(demo, {
      type = 'idle',
      session = 'ses_1',
      parent = vim.NIL,
      text = 'Added.',
      finish = 'stop',
      error = vim.NIL,
    })
    local item = '* opencode  1 turn  just now  add tests'
    local ui = support.stub_ui({ select = { item } })

    press_on('s', 'demo')

    wait_for_terminal_showing(demo, 'fake opencode ready')
    assert.are.same({ item, OLD_HELLO, OLD_HELLO }, ui.shown[1])
    local file = assert(io.open(repo.log .. '.opencode-argv', 'rb'))
    local argv = file:read('*a')
    file:close()
    assert.are.same({ '-s', 'ses_1' }, vim.split(argv, '\0', { plain = true, trimempty = true }))
    local listed = support.listed(repo, 'demo')
    assert.are.equal('opencode', listed.agent)
    assert.is_true(listed.holder)
    assert.are.same({}, ui.notes)
  end)

  it('New session asks first, then starts a new session and attaches to it', function()
    local repo, demo = two_sessions_on_demo()
    support.stub_ui({})
    press_on('<CR>', 'demo')
    local before = wait_for_terminal_showing(demo, 'fake claude ready')

    local ui = support.stub_ui({ select = { 'New session', 'Cancel' } })
    press_on('.', 'demo')
    assert.are.same(actions.MENU, ui.shown[1])
    assert.are.same(actions.NEW_SESSION, ui.shown[2])
    assert.are.equal(
      'Start a new session in demo? The current one keeps its turns; s brings it back.',
      ui.prompts[2]
    )
    assert.are.same({ SECOND.session_id .. ' (current)', FIRST.session_id }, sessions(repo))

    ui = support.stub_ui({ select = { 'New session', 'Start a new session' } })
    press_on('.', 'demo')

    local after = wait_for_terminal_showing(demo, 'fake claude ready', before)
    assert.are.equal(after, vim.api.nvim_get_current_buf(), 'shown in the current window')
    assert.are.same({}, vim.list_slice(assert(support.claude_argv(repo)), 5), 'no --resume')
    assert.are.same({ SECOND.session_id, FIRST.session_id }, sessions(repo))
    assert.are.same({}, ui.notes)
  end)

  it('tells why the core refuses an agent busy in a turn, and changes nothing', function()
    local repo, demo = two_sessions_on_demo()
    support.play_hooks(repo, demo, 'tui-queuetool-2.1.300.log', 1, 3)
    local ui = support.stub_ui({ select = { OLD_HELLO } })

    press_on('s', 'demo')

    support.wait_for(function()
      return #ui.notes > 0
    end, 'a notification')
    assert.are.equal(vim.log.levels.ERROR, ui.notes[1].level)
    assert(ui.notes[1].message:find('its agent is in a turn', 1, true), ui.notes[1].message)
    assert.are.equal('working', support.listed(repo, 'demo').state)
  end)

  it('says so when the worktree has no session yet', function()
    support.load_client()
    local repo = support.agent_repo()
    support.run({ support.binary, 'worktree', 'new', 'demo' }, repo.root)
    support.cd(repo.root)
    vim.cmd('OrcaTerm sidebar')
    local ui = support.stub_ui({})

    press_on('s', 'demo')

    support.wait_for(function()
      return #ui.notes > 0
    end, 'a notification')
    assert.are.same(
      { message = 'orca-term: no sessions in worktree demo', level = vim.log.levels.INFO },
      ui.notes[1]
    )
    assert.are.same({}, ui.shown)
  end)

  it(':OrcaTerm sessions lists the same sessions as s, in the sidebar and in a tab', function()
    local _, demo = two_sessions_on_demo()
    local ui = support.stub_ui({})

    press_on(':OrcaTerm sessions<CR>', 'demo')

    support.wait_for(function()
      return #ui.shown > 0
    end, 'the sessions from the sidebar')
    assert.are.equal('Sessions of demo', ui.prompts[1])
    assert.are.same({ CURRENT_HELLO, OLD_HELLO }, ui.shown[1])

    require('orca-term.tabs').go_to(demo)
    ui = support.stub_ui({})
    vim.cmd('OrcaTerm sessions')

    support.wait_for(function()
      return #ui.shown > 0
    end, "the sessions from the worktree's tab")
    assert.are.equal('Sessions of demo', ui.prompts[1])
    assert.are.same({ CURRENT_HELLO, OLD_HELLO }, ui.shown[1])
    assert.are.same({}, ui.notes)
  end)

  it(':OrcaTerm sessions says what it needs outside a worktree', function()
    support.load_client()
    local repo = support.agent_repo()
    support.cd(repo.root)
    local ui = support.stub_ui({})

    vim.cmd('OrcaTerm sessions')

    assert.are.same({
      message = "orca-term: the sessions need a worktree's tab or its review; "
        .. 'go to one, or press s on a worktree in the sidebar',
      level = vim.log.levels.ERROR,
    }, ui.notes[1])
    assert.are.same({}, ui.shown)
  end)

  it('shows an age in words', function()
    local minute = 60 * 1000
    assert.are.equal('just now', actions.age(59000, 0))
    assert.are.equal('5 min ago', actions.age(5 * minute, 0))
    assert.are.equal('3 h ago', actions.age(3 * 60 * minute, 0))
    assert.are.equal('1 day ago', actions.age(30 * 60 * minute, 0))
    assert.are.equal('3 days ago', actions.age(3 * 24 * 60 * minute, 0))
    assert.are.equal('just now', actions.age(0, minute), 'a clock behind is now')
  end)
end)
