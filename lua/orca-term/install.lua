-- orca-term's Neovim client: installing the core binary.
--
-- `build()` is the plugin's build hook, which `build.lua` at the plugin's root runs when a plugin
-- manager installs or updates the plugin. When the plugin is checked out at a release tag, it
-- downloads that release's binary for this OS and architecture, verifies its sha256 against the
-- `.sha256` file the release publishes beside it, and installs it at `<root>/bin/orca-term`, where
-- `core.find_binary` looks first. When there is no tag, no binary for this machine, or the download
-- or the check fails, it says which and runs `cargo build --release` instead. A binary whose sha256
-- does not match is never installed.
--
-- `install_cli()` is `:OrcaTerm install-cli`: it links the binary the client uses into
-- `$XDG_BIN_HOME` (else `~/.local/bin`), for the shell.
--
-- With core.lua, this is the only module that starts processes (`cargo xtask client-boundary`
-- checks it): git, curl, tar, cargo and, on Neovim 0.11, a sha256 tool. It never runs the core.
-- `build.lua` loads this file by its path, before the plugin is on the runtimepath, so it requires
-- no other module at load time.

local M = {}

---The project's name: the binary's, the release assets' and the messages'.
local NAME = 'orca-term'

---Where the releases' assets are: `<RELEASES_URL>/<tag>/<asset>`.
M.RELEASES_URL = 'https://github.com/marivaldo/' .. NAME .. '/releases/download'

---How long `build()` may wait for one process before giving up on it: cargo may take minutes.
local PROCESS_TIMEOUT_MS = 30 * 60 * 1000

local levels = vim.log.levels

---What `build()` was given.
---@class orca_term.BuildOptions
---@field root string the plugin's root, where `bin/` and `target/` are
---@field base_url? string where the releases are, `RELEASES_URL` when `nil`; the specs point it at
---a `file://` directory
---@field yield? boolean whether it runs in lazy.nvim's coroutine, which it then yields to while it
---waits, so Neovim stays responsive

---The release target of a machine, from what `vim.uv.os_uname()` says about it, such as
---`aarch64-apple-darwin`; `nil` when no release binary is made for it (only macOS and Linux, on
---arm64 and x86_64, ADR 0008).
---@param uname { sysname: string, machine: string }
---@return string? target
function M.target(uname)
  local systems = { Darwin = 'apple-darwin', Linux = 'unknown-linux-musl' }
  local machines = { arm64 = 'aarch64', aarch64 = 'aarch64', x86_64 = 'x86_64' }
  local system, machine = systems[uname.sysname], machines[uname.machine]
  if system and machine then
    return machine .. '-' .. system
  end
  return nil
end

---The name of a release asset, such as `orca-term-v0.1.0-aarch64-apple-darwin.tar.gz`. Its
---checksum is the same name with `.sha256` after it.
---@param tag string
---@param target string
---@return string
function M.asset_name(tag, target)
  return ('%s-%s-%s.tar.gz'):format(NAME, tag, target)
end

---Tells the person how the build goes: in lazy.nvim's coroutine, as a progress message lazy.nvim
---shows; anywhere else, with `vim.notify`.
---@param message string
---@param level integer
---@param yield boolean
local function report(message, level, yield)
  message = NAME .. ': ' .. message
  if yield then
    coroutine.yield({ msg = message, level = level })
  else
    vim.notify(message, level)
  end
end

---Runs `command` in `cwd` and waits for it to finish, yielding while it waits when `yield` is
---true. A command that cannot start (not installed) comes back with exit code 127 and the reason
---in `stderr`.
---@param command string[]
---@param cwd string
---@param yield boolean
---@return vim.SystemCompleted
local function run(command, cwd, yield)
  ---@type vim.SystemCompleted?
  local done = nil
  local options = { cwd = cwd, text = true, timeout = PROCESS_TIMEOUT_MS }
  local started, problem = pcall(vim.system, command, options, function(result)
    done = result
  end)
  if not started then
    return { code = 127, signal = 0, stdout = '', stderr = tostring(problem) }
  end
  if yield then
    while not done do
      coroutine.yield()
    end
  else
    vim.wait(PROCESS_TIMEOUT_MS + 1000, function()
      return done ~= nil
    end, 20)
  end
  return done or { code = 124, signal = 0, stdout = '', stderr = 'did not finish in time' }
end

---The first line of what a finished command said on stderr, or its exit code when it said
---nothing.
---@param result vim.SystemCompleted
---@return string
local function why_it_failed(result)
  local line = vim.split(vim.trim(result.stderr or ''), '\n')[1]
  if line == nil or line == '' then
    return ('exit %d'):format(result.code)
  end
  return line
end

---The tag the plugin's checkout is exactly at (`git describe --tags --exact-match`), or `nil`
---when it is at no tag, or is no git checkout.
---@param root string
---@param yield boolean
---@return string? tag
local function installed_tag(root, yield)
  local result = run({ 'git', 'describe', '--tags', '--exact-match', 'HEAD' }, root, yield)
  local tag = vim.trim(result.stdout or '')
  if result.code ~= 0 or tag == '' then
    return nil
  end
  return tag
end

---The sha256 a `.sha256` file names, as `shasum -a 256` writes it (`<hex>  <file>`), in lower case;
---`nil` when the file holds none.
---@param path string
---@return string? sha256
local function expected_sha256(path)
  local file = io.open(path, 'r')
  if not file then
    return nil
  end
  local text = file:read('*a') or ''
  file:close()
  local hex = text:match('^(%x+)%s')
  if not hex or #hex ~= 64 then
    return nil
  end
  return hex:lower()
end

---The sha256 of the file at `path`, in lower case. Neovim's `sha256()` hashes its bytes; on Neovim
---0.11, whose `sha256()` refuses bytes with a NUL in them, `sha256sum` or `shasum -a 256` does.
---@param path string
---@param yield boolean
---@return string? sha256
---@return string? err
local function file_sha256(path, yield)
  local file = io.open(path, 'rb')
  if not file then
    return nil, 'cannot read ' .. path
  end
  local bytes = file:read('*a') or ''
  file:close()
  local hashed, hash = pcall(vim.fn.sha256, bytes)
  if hashed then
    return hash:lower()
  end

  local tools = { { 'sha256sum' }, { 'shasum', '-a', '256' } }
  for _, tool in ipairs(tools) do
    if vim.fn.executable(tool[1]) == 1 then
      local command = vim.list_extend(vim.deepcopy(tool), { path })
      local result = run(command, vim.fs.dirname(path), yield)
      local hex = (result.stdout or ''):match('^(%x+)%s')
      if result.code ~= 0 or not hex then
        return nil, why_it_failed(result)
      end
      return hex:lower()
    end
  end
  return nil, 'neither sha256sum nor shasum is installed'
end

---Downloads `url` to the file at `path` with curl; `nil` once done, else why it could not.
---@param url string
---@param path string
---@param yield boolean
---@return string? why
local function download(url, path, yield)
  local result = run({ 'curl', '-fsSL', '-o', path, url }, vim.fs.dirname(path), yield)
  if result.code ~= 0 then
    return ('could not download %s: %s'):format(url, why_it_failed(result))
  end
  return nil
end

---Downloads the asset at `url` into the empty directory `work`, verifies it against its `.sha256`,
---unpacks the binary and moves it to `<root>/bin/orca-term`. `nil` once installed, else why it was
---not.
---@param url string the asset's URL
---@param asset string the asset's file name
---@param work string an empty directory inside `<root>/bin`, so the last move stays on one disk
---@param root string
---@param yield boolean
---@return string? why
local function fetch_and_install(url, asset, work, root, yield)
  report('downloading ' .. url, levels.INFO, yield)
  local archive, checksum = work .. '/' .. asset, work .. '/' .. asset .. '.sha256'
  local failed = download(url, archive, yield) or download(url .. '.sha256', checksum, yield)
  if failed then
    return failed
  end

  local expected = expected_sha256(checksum)
  if not expected then
    return ('%s.sha256 holds no sha256'):format(url)
  end
  local actual, err = file_sha256(archive, yield)
  if not actual then
    return ('could not compute the sha256 of %s: %s'):format(asset, err)
  end
  if actual ~= expected then
    return ('the sha256 of %s is %s, not %s as its release says, so it was not installed'):format(
      asset,
      actual,
      expected
    )
  end

  local unpacked = run({ 'tar', '-xzf', asset, NAME }, work, yield)
  local binary = work .. '/' .. NAME
  if unpacked.code ~= 0 or vim.uv.fs_stat(binary) == nil then
    return ('could not unpack %s from %s: %s'):format(NAME, asset, why_it_failed(unpacked))
  end
  local installed = root .. '/bin/' .. NAME
  local moved, problem = vim.uv.fs_chmod(binary, tonumber('755', 8))
  if moved then
    moved, problem = vim.uv.fs_rename(binary, installed)
  end
  if not moved then
    return ('could not install %s: %s'):format(installed, problem)
  end
  return nil
end

---Installs the release binary of the plugin's tag at `<root>/bin/orca-term`. `nil` once
---installed, else why it was not: no tag, no binary for this machine, a failed download or a
---sha256 that does not match.
---@param root string
---@param base_url string
---@param yield boolean
---@return string? why
local function install_release(root, base_url, yield)
  local tag = installed_tag(root, yield)
  if not tag then
    return 'the plugin is not checked out at a release tag'
  end
  local uname = vim.uv.os_uname()
  local target = M.target(uname)
  if not target then
    return ('no release binary is made for %s %s'):format(uname.sysname, uname.machine)
  end

  local asset = M.asset_name(tag, target)
  local work = root .. '/bin/.download'
  vim.fn.delete(work, 'rf')
  vim.fn.mkdir(work, 'p')
  local why = fetch_and_install(('%s/%s/%s'):format(base_url, tag, asset), asset, work, root, yield)
  vim.fn.delete(work, 'rf')
  if why then
    return why
  end
  local message = ('installed %s from release %s (%s), sha256 verified'):format(
    root .. '/bin/' .. NAME,
    tag,
    target
  )
  report(message, levels.INFO, yield)
  return nil
end

---Builds the core with `cargo build --release` in `root`, which puts it in `target/release/`.
---@param root string
---@param yield boolean
---@return string status `built` or `failed`
local function cargo_build(root, yield)
  if vim.fn.executable('cargo') ~= 1 then
    local message = 'cargo is not on PATH: install Rust (https://rustup.rs), '
      .. 'or check out a release tag of the plugin'
    report(message, levels.ERROR, yield)
    return 'failed'
  end
  report('running `cargo build --release` in ' .. root, levels.INFO, yield)
  local result = run({ 'cargo', 'build', '--release' }, root, yield)
  if result.code ~= 0 then
    local lines = vim.split(vim.trim(result.stderr or ''), '\n')
    local message = ('`cargo build --release` failed (exit %d): %s'):format(
      result.code,
      lines[#lines]
    )
    report(message, levels.ERROR, yield)
    return 'failed'
  end
  report('built ' .. root .. '/target/release/' .. NAME, levels.INFO, yield)
  return 'built'
end

---The build hook: installs the release binary of the plugin's tag at `<root>/bin/orca-term`, or
---says why it cannot and builds the core with `cargo build --release` instead. Before that build,
---it removes `<root>/bin/orca-term`, an earlier release's binary, so `core.find_binary` finds the
---one cargo builds.
---@param opts orca_term.BuildOptions
---@return string status `installed`, `built` or `failed`
function M.build(opts)
  local root, yield = opts.root, opts.yield or false
  local why = install_release(root, opts.base_url or M.RELEASES_URL, yield)
  if not why then
    return 'installed'
  end
  report(why .. '; building it with `cargo build --release` instead', levels.WARN, yield)
  local stale = root .. '/bin/' .. NAME
  if vim.uv.fs_lstat(stale) then
    vim.uv.fs_unlink(stale)
  end
  return cargo_build(root, yield)
end

---Where `install_cli()` links the binary by default: `$XDG_BIN_HOME`, else `~/.local/bin`.
---@return string
function M.bin_home()
  local xdg = vim.env.XDG_BIN_HOME
  if xdg and xdg ~= '' then
    return vim.fs.normalize(xdg)
  end
  return vim.fs.normalize('~/.local/bin')
end

---Whether the directory `dir` is on PATH, symlinks resolved on both sides.
---@param dir string
---@return boolean
local function on_path(dir)
  local wanted = vim.uv.fs_realpath(dir) or vim.fs.normalize(dir)
  for _, entry in ipairs(vim.split(vim.env.PATH or '', ':', { plain = true })) do
    if entry ~= '' and (vim.uv.fs_realpath(entry) or vim.fs.normalize(entry)) == wanted then
      return true
    end
  end
  return false
end

---`:OrcaTerm install-cli`: links the binary `core.find_binary` returns (symlinks resolved) as
---`<dir>/orca-term`, creating `dir`, and says so; warns when `dir` is not on PATH. A link already
---there is replaced; anything else there is left alone, and refused with a message.
---@param dir? string where the link goes; `bin_home()` when `nil`
---@return string? link the link made; `nil` when it made none
function M.install_cli(dir)
  dir = vim.fs.normalize(dir or M.bin_home())
  local binary, err = require('orca-term.core').find_binary()
  if not binary then
    vim.notify(err and err.message or (NAME .. ': no core binary found'), levels.ERROR)
    return nil
  end
  binary = vim.uv.fs_realpath(binary) or binary

  local made, problem = pcall(vim.fn.mkdir, dir, 'p')
  if not made then
    vim.notify(('%s: could not create %s: %s'):format(NAME, dir, problem), levels.ERROR)
    return nil
  end
  local link = dir .. '/' .. NAME
  local existing = vim.uv.fs_lstat(link)
  if existing and existing.type ~= 'link' then
    local message = '%s: %s exists and is not a symlink; move it away, '
      .. 'then run `:OrcaTerm install-cli` again'
    vim.notify(message:format(NAME, link), levels.ERROR)
    return nil
  end
  if existing then
    vim.uv.fs_unlink(link)
  end
  local linked, why = vim.uv.fs_symlink(binary, link)
  if not linked then
    vim.notify(('%s: could not link %s -> %s: %s'):format(NAME, link, binary, why), levels.ERROR)
    return nil
  end

  vim.notify(('%s: linked %s -> %s'):format(NAME, link, binary), levels.INFO)
  if not on_path(dir) then
    local message = '%s: %s is not on PATH; add it to PATH to run %s from the shell'
    vim.notify(message:format(NAME, dir, NAME), levels.WARN)
  end
  return link
end

return M
