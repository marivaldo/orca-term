#!/bin/sh
# PreToolUse guard for every coding agent working in this repository: refuses git invocations that
# skip hooks or signing, so the rules hold without anyone's global setup. Exit 2 blocks the call.
input=$(cat)
pattern='--no-verify|--no-gpg-sign|--no-sign([^a-z-]|$)|gpgsign[" =]*false|core\.hooksPath|GIT_CONFIG_[A-Z]+='
if printf '%s' "$input" | grep -Eq -- "$pattern"; then
  echo "blocked by .claude/hooks/git-guard.sh: commits are always signed and hooks are never skipped (see CLAUDE.md)" >&2
  exit 2
fi
exit 0
