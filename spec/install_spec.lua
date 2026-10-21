-- Seam 2: lua/orca-term/install.lua, the build hook and `:OrcaTerm install-cli`.
--
-- The build hook runs against a fake release: a directory laid out as GitHub serves a release's
-- assets, `<tag>/<asset>` and `<tag>/<asset>.sha256`, packed by the release workflow's own script
-- (`.github/scripts/package-release.sh`), and reached through a `file://` base URL, so the specs
-- need no network. The workflow's verification script (`.github/scripts/verify-release.sh`) is
-- checked here too, on assets packed the same way. A fake `cargo` first on PATH records how it was run, so they never build a
-- release. `install_cli` links into temporary directories only, never the real home.

local support = require('support')
local orca_term = require('orca-term')
local install = require('orca-term.install')

local levels = vim.log.levels

---The tag the plugin checkouts are at.
local TAG = 'v9.9.9'

---The release target of the machine the specs run on.
local TARGET = assert(install.target(vim.uv.os_uname()), 'no release target for this machine')

---A plugin checkout: a git repository with one commit, at the tag `TAG` when `tagged`.
---@param tagged boolean
---@return string root
local function plugin_root(tagged)
  local root = support.isolate().tmp .. '/plugin'
  vim.fn.mkdir(root, 'p')
  support.run({ 'git', 'init', '--quiet' }, root)
  support.run({ 'git', 'commit', '--quiet', '--allow-empty', '-m', 'init' }, root)
  if tagged then
    support.run({ 'git', 'tag', TAG }, root)
  end
  return root
end

---The release workflow's scripts.
local PACK = support.root .. '/.github/scripts/package-release.sh'
local VERIFY = support.root .. '/.github/scripts/verify-release.sh'

---A fake release of `TAG` holding one asset, for `target`, packed by the release workflow's own
---script: a binary (a script printing `fake orca-term`) and LICENSE, and the asset's `.sha256`.
---@param target string
---@return string base_url the `file://` URL to hand `build()`
---@return string asset the asset's path
local function fake_release(target)
  local base = support.temp_dir()
  local binary = support.temp_dir() .. '/built'
  support.write_script(binary, { 'echo fake orca-term' })
  support.run({ 'bash', PACK, binary, TAG, target, base .. '/' .. TAG })
  return 'file://' .. base, ('%s/%s/%s'):format(base, TAG, install.asset_name(TAG, target))
end

---The sha256 of the file at `path`, by `shasum -a 256`.
---@param path string
---@return string
local function sha256(path)
  return (support.run({ 'shasum', '-a', '256', path }):match('^(%x+)'))
end

---Puts a fake `cargo` first on PATH, which appends its directory and arguments to a log, then
---makes `target/release/orca-term`, or prints `error: could not compile` and exits 101 when
---`fails`.
---@param fails? boolean
---@return string log the log's path
local function fake_cargo(fails)
  local bin = support.temp_dir()
  local log = bin .. '/cargo.log'
  local lines = { ('echo "$(pwd -P) $*" >> %s'):format(vim.fn.shellescape(log)) }
  if fails then
    vim.list_extend(lines, { 'echo "error: could not compile" >&2', 'exit 101' })
  else
    vim.list_extend(lines, {
      'mkdir -p target/release',
      "printf '#!/bin/sh\\n' > target/release/orca-term",
      'chmod +x target/release/orca-term',
    })
  end
  support.write_script(bin .. '/cargo', lines)
  support.set_env('PATH', bin .. ':' .. vim.env.PATH)
  return log
end

---The lines of the file at `path`; none when there is no such file.
---@param path string
---@return string[]
local function lines_of(path)
  return vim.fn.filereadable(path) == 1 and vim.fn.readfile(path) or {}
end

---The note, among those `stub_ui` captured, saying `message`.
---@param ui table what `support.stub_ui` returned
---@param message string
---@return { message: string, level: integer }?
local function note(ui, message)
  for _, shown in ipairs(ui.notes) do
    if shown.message == message then
      return shown
    end
  end
  return nil
end

local FALLBACK = '; building it with `cargo build --release` instead'

describe('the build hook', function()
  after_each(support.cleanup)

  it('installs the release binary of the tag when its sha256 matches', function()
    local root = plugin_root(true)
    local base = fake_release(TARGET)
    local log = fake_cargo()
    local ui = support.stub_ui({})

    local status = install.build({ root = root, base_url = base })

    assert.are.equal('installed', status)
    local installed = root .. '/bin/orca-term'
    assert.are.equal('fake orca-term\n', support.run({ installed }))
    assert.is_nil(vim.uv.fs_stat(root .. '/bin/.download'))
    assert.are.same({}, lines_of(log))
    local url = ('%s/%s/%s'):format(base, TAG, install.asset_name(TAG, TARGET))
    assert.are.same({
      { message = 'orca-term: downloading ' .. url, level = levels.INFO },
      {
        message = ('orca-term: installed %s from release %s (%s), sha256 verified'):format(
          installed,
          TAG,
          TARGET
        ),
        level = levels.INFO,
      },
    }, ui.notes)
  end)

  it('never installs a binary whose sha256 does not match, and builds with cargo', function()
    local root = plugin_root(true)
    local base, asset = fake_release(TARGET)
    local name = install.asset_name(TAG, TARGET)
    local wrong = ('0'):rep(64)
    vim.fn.writefile({ wrong .. '  ' .. name }, asset .. '.sha256')
    -- An earlier release's binary, which would hide the one cargo builds.
    vim.fn.mkdir(root .. '/bin', 'p')
    support.write_script(root .. '/bin/orca-term', { 'echo earlier release' })
    local log = fake_cargo()
    local ui = support.stub_ui({})

    local status = install.build({ root = root, base_url = base })

    assert.are.equal('built', status)
    assert.is_nil(vim.uv.fs_lstat(root .. '/bin/orca-term'))
    assert.are.same({ root .. ' build --release' }, lines_of(log))
    local mismatch = ('orca-term: the sha256 of %s is %s, not %s as its release says, '):format(
      name,
      sha256(asset),
      wrong
    ) .. 'so it was not installed' .. FALLBACK
    assert.are.equal(levels.WARN, assert(note(ui, mismatch), mismatch).level)
    assert(note(ui, 'orca-term: running `cargo build --release` in ' .. root))
    assert(note(ui, 'orca-term: built ' .. root .. '/target/release/orca-term'))
    assert.are.equal(1, vim.fn.executable(root .. '/target/release/orca-term'))
  end)

  it('builds with cargo when the release has no binary for this machine', function()
    local root = plugin_root(true)
    local base = fake_release('riscv64gc-unknown-linux-gnu')
    local log = fake_cargo()
    local ui = support.stub_ui({})

    local status = install.build({ root = root, base_url = base })

    assert.are.equal('built', status)
    assert.is_nil(vim.uv.fs_stat(root .. '/bin/orca-term'))
    assert.are.same({ root .. ' build --release' }, lines_of(log))
    local url = ('%s/%s/%s'):format(base, TAG, install.asset_name(TAG, TARGET))
    local failed = ui.notes[2]
    assert.are.equal(levels.WARN, failed.level)
    assert(vim.startswith(failed.message, 'orca-term: could not download ' .. url .. ': '))
    assert(vim.endswith(failed.message, FALLBACK), failed.message)
  end)

  it('builds with cargo when the plugin is not checked out at a release tag', function()
    local root = plugin_root(false)
    local base = fake_release(TARGET)
    local log = fake_cargo()
    local ui = support.stub_ui({})

    local status = install.build({ root = root, base_url = base })

    assert.are.equal('built', status)
    assert.is_nil(vim.uv.fs_stat(root .. '/bin/orca-term'))
    assert.are.same({ root .. ' build --release' }, lines_of(log))
    assert.are.same({
      message = 'orca-term: the plugin is not checked out at a release tag' .. FALLBACK,
      level = levels.WARN,
    }, ui.notes[1])
  end)

  it('fails, saying why, when cargo cannot build the core either', function()
    local root = plugin_root(false)
    fake_cargo(true)
    local ui = support.stub_ui({})

    local status = install.build({ root = root })

    assert.are.equal('failed', status)
    assert.are.same({
      message = 'orca-term: `cargo build --release` failed (exit 101): error: could not compile',
      level = levels.ERROR,
    }, ui.notes[#ui.notes])
  end)

  it('names the release target of each machine a binary is made for', function()
    local cases = {
      { 'Darwin', 'arm64', 'aarch64-apple-darwin' },
      { 'Darwin', 'x86_64', 'x86_64-apple-darwin' },
      { 'Linux', 'aarch64', 'aarch64-unknown-linux-musl' },
      { 'Linux', 'x86_64', 'x86_64-unknown-linux-musl' },
    }
    for _, case in ipairs(cases) do
      assert.are.equal(case[3], install.target({ sysname = case[1], machine = case[2] }))
    end
    assert.is_nil(install.target({ sysname = 'Linux', machine = 'riscv64' }))
    assert.is_nil(install.target({ sysname = 'Windows_NT', machine = 'x86_64' }))
    assert.are.equal(
      'orca-term-v0.1.0-x86_64-apple-darwin.tar.gz',
      install.asset_name('v0.1.0', 'x86_64-apple-darwin')
    )
  end)
end)

describe("the release workflow's verification", function()
  after_each(support.cleanup)

  local TARGETS = { 'aarch64-apple-darwin', 'x86_64-unknown-linux-musl' }

  ---A directory holding the assets of `TARGETS`, packed by the workflow's script, as the release
  ---job downloads them.
  ---@return string dir
  local function packed()
    local dir = support.temp_dir()
    local binary = support.temp_dir() .. '/built'
    support.write_script(binary, { 'echo fake orca-term' })
    for _, target in ipairs(TARGETS) do
      support.run({ 'bash', PACK, binary, TAG, target, dir })
    end
    return dir
  end

  ---Runs the verification over `dir` for `TARGETS`: its exit code and the last line of its stderr.
  ---@param dir string
  ---@return integer code
  ---@return string said
  local function verify(dir)
    local command = vim.list_extend({ 'bash', VERIFY, dir, TAG }, TARGETS)
    local result = vim.system(command, { text = true }):wait()
    local lines = vim.split(vim.trim(result.stderr or ''), '\n')
    return result.code, lines[#lines]
  end

  it('passes the assets the workflow packed', function()
    assert.are.equal(0, (verify(packed())))
  end)

  it('fails an asset that does not match its checksum', function()
    local dir = packed()
    local asset = io.open(dir .. '/' .. install.asset_name(TAG, TARGETS[1]), 'ab')
    assert(asset):write('tampered')
    assert(asset):close()
    assert.are_not.equal(0, (verify(dir)))
  end)

  it('fails when an asset is missing', function()
    local dir = packed()
    local asset = install.asset_name(TAG, TARGETS[2])
    assert(vim.uv.fs_unlink(dir .. '/' .. asset))
    local code, said = verify(dir)
    assert.are.equal(1, code)
    assert.are.equal(('verify: %s is missing'):format(asset), said)
  end)

  it('fails a checksum file that names another asset', function()
    local dir = packed()
    local first, second = install.asset_name(TAG, TARGETS[1]), install.asset_name(TAG, TARGETS[2])
    assert(vim.uv.fs_copyfile(dir .. '/' .. second .. '.sha256', dir .. '/' .. first .. '.sha256'))
    local code, said = verify(dir)
    assert.are.equal(1, code)
    assert.are.equal(('verify: %s.sha256 names %s'):format(first, second), said)
  end)

  it('fails a file that is no asset of a target', function()
    local dir = packed()
    vim.fn.writefile({ 'extra' }, dir .. '/extra')
    local code, said = verify(dir)
    assert.are.equal(1, code)
    assert.are.equal('verify: 5 files, expected 4', said)
  end)
end)

describe(':OrcaTerm install-cli', function()
  after_each(support.cleanup)

  ---A fake core binary, set as `binary` in setup().
  ---@return string
  local function fake_binary()
    local binary = support.temp_dir() .. '/orca-term'
    support.write_script(binary, { 'exit 0' })
    orca_term.setup({ binary = binary })
    return binary
  end

  it('links the binary into the directory, creating it, and says where', function()
    local binary = fake_binary()
    local dir = support.temp_dir() .. '/new/bin'
    support.set_env('PATH', dir .. ':' .. vim.env.PATH)
    local ui = support.stub_ui({})

    local link = install.install_cli(dir)

    assert.are.equal(dir .. '/orca-term', link)
    assert.are.equal(binary, vim.uv.fs_readlink(dir .. '/orca-term'))
    assert.are.same({
      {
        message = ('orca-term: linked %s/orca-term -> %s'):format(dir, binary),
        level = levels.INFO,
      },
    }, ui.notes)
  end)

  it('warns when the directory is not on PATH', function()
    local binary = fake_binary()
    local dir = support.temp_dir()
    local ui = support.stub_ui({})

    install.install_cli(dir)

    assert.are.same({
      {
        message = ('orca-term: linked %s/orca-term -> %s'):format(dir, binary),
        level = levels.INFO,
      },
      {
        message = ('orca-term: %s is not on PATH; add it to PATH to run orca-term from the shell'):format(
          dir
        ),
        level = levels.WARN,
      },
    }, ui.notes)
  end)

  it('replaces a link already there', function()
    local binary = fake_binary()
    local dir = support.temp_dir()
    assert(vim.uv.fs_symlink('/nowhere/orca-term', dir .. '/orca-term'))
    support.stub_ui({})

    assert.are.equal(dir .. '/orca-term', install.install_cli(dir))

    assert.are.equal(binary, vim.uv.fs_readlink(dir .. '/orca-term'))
  end)

  it('refuses to replace anything there that is not a link', function()
    fake_binary()
    local dir = support.temp_dir()
    vim.fn.writefile({ 'mine' }, dir .. '/orca-term')
    local ui = support.stub_ui({})

    assert.is_nil(install.install_cli(dir))

    assert.are.same({ 'mine' }, vim.fn.readfile(dir .. '/orca-term'))
    assert.are.same({
      {
        message = ('orca-term: %s/orca-term exists and is not a symlink; move it away, '):format(
          dir
        ) .. 'then run `:OrcaTerm install-cli` again',
        level = levels.ERROR,
      },
    }, ui.notes)
  end)

  it('links into $XDG_BIN_HOME when no directory is given', function()
    local binary = fake_binary()
    local dir = support.temp_dir()
    support.set_env('XDG_BIN_HOME', dir)
    support.stub_ui({})

    assert.are.equal(dir .. '/orca-term', install.install_cli())

    assert.are.equal(binary, vim.uv.fs_readlink(dir .. '/orca-term'))
  end)

  it('links nothing when there is no core binary, naming the fix', function()
    local missing = support.temp_dir() .. '/orca-term'
    orca_term.setup({ binary = missing })
    local dir = support.temp_dir()
    local ui = support.stub_ui({})

    assert.is_nil(install.install_cli(dir))

    assert.is_nil(vim.uv.fs_lstat(dir .. '/orca-term'))
    assert.are.equal(levels.ERROR, ui.notes[1].level)
    assert(vim.startswith(ui.notes[1].message, 'orca-term: the core binary set in setup()'))
  end)
end)
