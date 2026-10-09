# orca-term

## Harness

Run `just check` before every commit: it runs everything CI runs (rustfmt, Clippy, tests,
cargo-deny, typos, actionlint, zizmor, the vocabulary, layers and commit lints). Install the local
hooks once with `just hooks`. Every rule is enforced by a guard listed in `docs/harness.md`; a
slice is done only when the guards for its behaviour are green there.

`ARCHITECTURE.md` maps the code: its five layers, where each concept lives and the invariants.

Commits are always signed and hooks are never skipped: `.claude/hooks/git-guard.sh` refuses
`--no-verify`, `--no-gpg-sign`, signing overrides, `core.hooksPath` and `GIT_CONFIG_*` overrides.
Commit messages are one line with a type prefix (`feat: ...`), with no Jira id and no AI
attribution. PR titles read `#<issue> - <lowercase description>` (the issue number stands where
a Jira id would), with no type prefix; PR bodies are in English and say `Closes #<issue>`.

## Agent skills

### Issue tracker

Issues e specs vivem nas GitHub Issues de `marivaldo/orca-term`, operadas via `gh` CLI.
See `docs/agents/issue-tracker.md`.

### Triage labels

Vocabulário canônico sem overrides: `needs-triage`, `needs-info`, `ready-for-agent`,
`ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: um `CONTEXT.md` na raiz e ADRs em `docs/adr/`. See `docs/agents/domain.md`.
