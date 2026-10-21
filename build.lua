-- orca-term's build hook: puts the core binary this checkout of the plugin needs in place.
--
-- lazy.nvim runs this file by itself after it installs or updates the plugin; other plugin managers
-- run `nvim -l build.lua` in the plugin's directory. It downloads and verifies the release binary
-- of the checkout's tag, or else builds the core with `cargo build --release`: see
-- lua/orca-term/install.lua, loaded here by its path since the plugin is not on the runtimepath
-- yet. It fails, so the plugin manager shows the build failed, only when no binary could be had.

local root = vim.fn.fnamemodify(debug.getinfo(1, 'S').source:sub(2), ':p:h')
local install = dofile(root .. '/lua/orca-term/install.lua')

-- lazy.nvim runs this file in a coroutine of its own, and resumes it on the next tick after each
-- yield: waiting by yielding there keeps Neovim responsive while cargo builds.
local thread, is_main = coroutine.running()
local in_coroutine = thread ~= nil and not is_main

if install.build({ root = root, yield = in_coroutine }) == 'failed' then
  error('orca-term: the build hook found no binary to install and could not build one', 0)
end
