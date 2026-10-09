# The harness: `just check` runs everything CI runs. See docs/harness.md.

set shell := ["bash", "-euo", "pipefail", "-c"]

# Every check, as CI runs it.
check: rust deny meta

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

# Advisories, bans, licenses and sources.
deny:
    cargo deny check

# Spelling, workflows, vocabulary, commit messages and the agent guard.
meta range="origin/main..HEAD": typos workflows vocab (commit-lint range) guard-test

# Spelling over the given files, or the whole tree.
typos *files:
    typos {{ files }}

workflows:
    actionlint
    zizmor --quiet .github/workflows

vocab:
    cargo xtask vocab

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
