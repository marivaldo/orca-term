-- Throwaway git repositories for the specs, and the real core binary `cargo build` made.
--
-- Mirrors tests/support/mod.rs: every git process, the helper's and the core's, reads a gitconfig
-- of the fixture's own, and the core gets a HOME and an XDG_CONFIG_HOME inside the fixture, so
-- the specs never read the developer's config. The variables are set in this Neovim, whose
-- children inherit them, and `cleanup()` puts them back.

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
  local fleet = vim.json.decode(M.run({ M.binary, 'worktree', 'ls', '--json' }, repo.root))
  for _, worktree in ipairs(fleet.worktrees) do
    if worktree.name == name then
      return worktree.path
    end
  end
  error('no worktree named ' .. name)
end

---Makes `dir` Neovim's current directory until `cleanup()`.
---@param dir string
function M.cd(dir)
  saved_cwd = saved_cwd or vim.fn.getcwd()
  vim.cmd.cd(vim.fn.fnameescape(dir))
end

---Loads the client the way Neovim does at startup, then calls setup() with the binary `cargo
---build` made, so a stale release build is never picked up.
function M.load_client()
  vim.opt.runtimepath:prepend(M.root)
  vim.cmd.runtime('plugin/orca-term.lua')
  require('orca-term').setup({ binary = M.binary })
end

---Closes the sidebar if it is open, then every tab and window but one, so the next spec starts
---from a single empty window.
function M.reset_editor()
  local sidebar = require('orca-term.sidebar')
  if sidebar.is_open() then
    sidebar.close()
  end
  vim.cmd('silent! tabonly!')
  vim.cmd('silent! only!')
  vim.cmd('enew!')
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

---Puts the environment, the current directory and the client's settings back, and removes every
---temporary directory.
function M.cleanup()
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
  require('orca-term').setup()
end

return M
