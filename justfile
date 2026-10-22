# The harness: `just check` runs everything CI runs. See docs/harness.md.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Every check, as CI runs it.
check: rust deny meta lua

# Rust: format, lint and test.
rust: fmt-check clippy test

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all --check

clippy:
    cargo clippy --workspace --all-targets --locked -- -D warnings

test:
    cargo test --workspace --locked

# The Neovim client: format, lint and type-check the Lua, then run its specs against the core.
lua: lua-fmt-check lua-lint lua-types lua-test

lua-fmt:
    stylua lua plugin spec build.lua

lua-fmt-check:
    stylua --check lua plugin spec build.lua

lua-lint:
    selene lua plugin spec build.lua

# LuaLS's type check, against the Lua API of the nvim on PATH; any warning fails it.
lua-types:
    #!/usr/bin/env bash
    set -euo pipefail
    # .luarc.json reads Neovim's Lua API from $VIMRUNTIME, the runtime of the nvim on PATH.
    VIMRUNTIME="$(nvim --clean --headless --cmd 'lua io.stdout:write(os.getenv("VIMRUNTIME"))' --cmd 'qa!')"
    export VIMRUNTIME
    logs="$(mktemp -d)"
    trap 'rm -rf "$logs"' EXIT
    lua-language-server --check . --checklevel=Warning --logpath "$logs"

# The specs: busted inside headless Neovim (through nlua), against the binary `cargo build` makes.
lua-test:
    #!/usr/bin/env bash
    set -euo pipefail
    rocks="$PWD/.rocks"
    if [ ! -x "$rocks/bin/busted" ]; then
        echo "lua-test: busted is not installed; run \`just lua-rocks\` once" >&2
        exit 1
    fi
    if [ ! -f "$rocks/lualine.nvim/lua/lualine.lua" ]; then
        echo "lua-test: lualine.nvim is not installed; run \`just lua-rocks\`" >&2
        exit 1
    fi
    if [ ! -f "$rocks/diffview.nvim/lua/diffview/init.lua" ]; then
        echo "lua-test: diffview.nvim is not installed; run \`just lua-rocks\`" >&2
        exit 1
    fi
    cargo build --locked
    export PATH="$rocks/bin:$PATH"
    export LUA_PATH="$rocks/share/lua/5.1/?.lua;$rocks/share/lua/5.1/?/init.lua;;"
    export LUA_CPATH="$rocks/lib/lua/5.1/?.so;;"
    # nlua reads Lua from stdin when stdin is not a terminal: /dev/null keeps it from waiting.
    busted </dev/null

# Installs busted and nlua, pinned, into .rocks/ for `just lua-test`; lualine.nvim at a pinned
# commit into .rocks/lualine.nvim, for the specs of the status strip's lualine component; and
# diffview.nvim at a pinned commit into .rocks/diffview.nvim, for the specs of the review. Run it
# once; running it again changes nothing.
lua-rocks:
    #!/usr/bin/env bash
    set -euo pipefail
    luarocks --lua-version=5.1 --tree .rocks install busted 2.3.0-1
    luarocks --lua-version=5.1 --tree .rocks install nlua 0.3.2-1
    # lualine.nvim (MIT), the commit of 2026-05-31. Only that commit is fetched, and checked out
    # again when the directory holds another.
    lualine_sha=221ce6b2d999187044529f49da6554a92f740a96
    lualine=.rocks/lualine.nvim
    if [ "$(git -C "$lualine" rev-parse HEAD 2>/dev/null || true)" != "$lualine_sha" ]; then
        rm -rf "$lualine"
        git init --quiet "$lualine"
        git -C "$lualine" fetch --quiet --depth 1 https://github.com/nvim-lualine/lualine.nvim "$lualine_sha"
        git -C "$lualine" checkout --quiet FETCH_HEAD
    fi
    # diffview.nvim (MIT), the commit of 2024-06-13, fetched the same way.
    diffview_sha=4516612fe98ff56ae0415a259ff6361a89419b0a
    diffview=.rocks/diffview.nvim
    if [ "$(git -C "$diffview" rev-parse HEAD 2>/dev/null || true)" != "$diffview_sha" ]; then
        rm -rf "$diffview"
        git init --quiet "$diffview"
        git -C "$diffview" fetch --quiet --depth 1 https://github.com/sindrets/diffview.nvim "$diffview_sha"
        git -C "$diffview" checkout --quiet FETCH_HEAD
    fi

# Advisories, bans, licenses and sources.
deny:
    cargo deny check

# Spelling, workflows, vocabulary, layers, the client boundary, the launch guard, the gh guard,
# commit messages and the agent guard.
meta range="origin/main..HEAD": typos workflows vocab layers client-boundary launch-guard gh-guard (commit-lint range) guard-test

# Spelling over the given files, or the whole tree; files the config excludes stay excluded when named.
typos *files:
    typos --force-exclude {{ files }}

workflows:
    actionlint
    zizmor --quiet .github/workflows

vocab:
    cargo xtask vocab

# The core's layers: imports point down, each file names its layer, I/O opt-outs only in adapters.
layers:
    cargo xtask layers

# The Neovim client: only core.lua and install.lua start processes, only sidebar.lua, tabs.lua,
# terminal.lua and review.lua touch windows, tabs and buffers, fleet.lua and render.lua are pure.
client-boundary:
    cargo xtask client-boundary

# No launch of an agent bypasses permissions: the forbidden flags and modes are named only in the
# run-time guard, src/domain/launch.rs.
launch-guard:
    cargo xtask launch-guard

# The GitHub CLI runs only from `pr refresh`: only src/ops/pr_refresh.rs names gh, and the Neovim
# client never does.
gh-guard:
    cargo xtask gh-guard

# Lints every commit in the range; skipped when its base does not exist (no remote yet).
commit-lint range="origin/main..HEAD":
    #!/usr/bin/env bash
    set -euo pipefail
    base="{{ range }}"; base="${base%%..*}"
    if git rev-parse --quiet --verify "${base}^{commit}" >/dev/null; then
        cargo xtask commit-lint --range "{{ range }}"
    else
        echo "commit-lint: ${base} not found, skipped"
    fi

# A PR title: `#<issue> - <lowercase description>`. CI runs it on every pull request. The title is
# passed as a positional argument, never interpolated, because anyone can write a PR title.
[positional-arguments]
pr-title title:
    cargo xtask pr-title-lint "$1"

# The commit-msg hook.
commit-msg file:
    cargo xtask commit-lint --file "{{ file }}"

# The agent guard refuses what skips hooks or signing, and lets ordinary git through.
guard-test:
    #!/usr/bin/env bash
    set -uo pipefail
    guard=.claude/hooks/git-guard.sh
    fail=0
    expect() {
        printf '{"tool_input":{"command":"%s"}}' "$2" | "$guard" >/dev/null 2>&1
        local got=$?
        if [ "$got" -ne "$1" ]; then echo "guard: exit $got, wanted $1, for: $2"; fail=1; fi
    }
    expect 0 'git commit -m \"feat: x\"'
    expect 0 'git push origin feat/x'
    expect 2 'git commit --no-verify -m x'
    expect 2 'git commit -m x --no-gpg-sign'
    expect 2 'git -c commit.gpgsign=false commit -m x'
    expect 2 'git config core.hooksPath /tmp/h'
    expect 2 'GIT_CONFIG_GLOBAL=/dev/null git commit -m x'
    exit $fail

# Installs the git hooks as shims calling lefthook. Never touches core.hooksPath.
hooks:
    #!/usr/bin/env bash
    set -euo pipefail
    dir="$(git rev-parse --git-common-dir)/hooks"
    mkdir -p "$dir"
    for hook in pre-commit commit-msg; do
        printf '#!/bin/sh\nexec lefthook run %s "$@"\n' "$hook" > "$dir/$hook"
        chmod +x "$dir/$hook"
    done
    echo "installed pre-commit and commit-msg in $dir"

# The live checks (#44), which CI never runs: every scenario of tests/live/ against the installed
# claude and opencode, through the real orca-term, re-recording what they send into
# tests/fixtures/, then the suites that replay the recordings, and a summary. It spends real
# prompts (README.md, "Live checks"). `just check-live p-hello` runs only the scenarios named.
check-live *scenarios:
    cargo xtask check-live {{ scenarios }}

# Records one live scenario by hand, this terminal attached to the agent, for what cannot be typed
# reliably; the scenario's file says what to do.
record-live scenario:
    cargo xtask record-live {{ scenario }}
