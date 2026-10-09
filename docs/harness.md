# Harness

Every rule this project adopts is enforced by a guard. `just check` runs every guard that CI runs,
and GitHub Actions is the authority. lefthook (`just hooks`) mirrors the fast subset locally, and
`.claude/settings.json` gives every coding agent the same guard against skipping hooks or signing.

A slice is done only when the guards for the behaviour it introduces are green in `just check` and
listed here. Rows marked *planned* are rules already ruled on the map, waiting for the slice that
introduces their behaviour. Decided in [the harness ticket](https://github.com/marivaldo/orca-term/issues/24).

## Running it

| Command | What it runs |
|---|---|
| `just check` | everything below, as CI runs it |
| `just rust` | rustfmt check, Clippy with `-D warnings`, the test suites |
| `just deny` | cargo-deny |
| `just meta` | typos, actionlint, zizmor, the vocabulary lint, the layers lint, the commit lint over `origin/main..HEAD`, the agent guard's tests |
| `just hooks` | installs the pre-commit and commit-msg hooks as lefthook shims (never touches `core.hooksPath`) |

## Rules and guards

| Rule | Guard | Where it runs | Slice | Status |
|---|---|---|---|---|
| Rust follows the Rust Style Guide (`style_edition` 2024) | rustfmt check | pre-commit, CI `rust` | [#27](https://github.com/marivaldo/orca-term/issues/27) | active |
| Clippy `all` + `pedantic` and the chosen restriction lints, no `#[allow]` without a reason | Clippy, Cargo `[workspace.lints]` | CI `rust` | #27 | active |
| No unsafe code | `unsafe_code = "forbid"` | compiler | #27 | active |
| Only stdout/stderr through the output module | `print_stdout`, `print_stderr` | CI `rust` | #27 | active |
| The core is a library (`src/lib.rs` declares every module) and `src/main.rs` only parses arguments, calls it and maps an error to an exit code (ADR 0010) | the crate layout: the black-box tests in `tests/` run the binary, the unit tests run in the library | CI `rust` | [#51](https://github.com/marivaldo/orca-term/issues/51) | active |
| Visibility is `pub(crate)` by default; `pub` only for what the binary and the tests use | rustc `unreachable_pub`, Cargo `[workspace.lints]` | CI `rust` | #51 | active |
| One file per module, `foo.rs` next to `foo/`, never `mod.rs` | Clippy `mod_module_files` | CI `rust` | #51 | active |
| Functions of at most 60 lines and at most 4 levels of nesting | Clippy `too_many_lines`, `excessive_nesting` (thresholds in `clippy.toml`) | CI `rust` | #51 | active |
| The core sits in five layers (main > edge > ops > adapters > domain), and a module uses only its own layer or a lower one (ADR 0010) | `cargo xtask layers` (every `crate::` reference against the table `MODULES` in `xtask/src/layers.rs`), with unit tests including an upward import | CI `meta` | [#52](https://github.com/marivaldo/orca-term/issues/52) | active |
| Only adapters touch git, files and the environment; the domain is pure | Clippy `disallowed-methods` and `disallowed-types` (`clippy.toml`), each adapter opting out with `#![expect(..., reason)]`; `cargo xtask layers` rejects the opt-out outside `src/adapters/` | CI `rust`, CI `meta` | #52 | active |
| Every file under `src/` starts with a `//!` line naming its layer (`//! Domain: ...`) | `cargo xtask layers` | CI `meta` | #52 | active |
| `ARCHITECTURE.md` is the code map: the layers, where each concept lives, the invariants; it changes with every change that moves a module | review, against the layer table `cargo xtask layers` enforces | PR review | #52 | active |
| Dependencies: no yanked, unmaintained or vulnerable crates; MIT, Apache-2.0 or Unicode-3.0 only; crates.io only; no wildcards | cargo-deny | CI `deny` | #27 | active |
| No typos | typos | pre-commit, CI `meta` | #27 | active |
| Workflows valid, actions pinned by SHA, least privilege | actionlint, zizmor | CI `meta` | #27 | active |
| Commit messages: one line, type prefix, no Jira id, no AI attribution | `cargo xtask commit-lint` | commit-msg hook, CI `meta` over the pushed range | #27 | active |
| PR titles read `#<issue> - <lowercase description>`, with no type prefix | `cargo xtask pr-title-lint` (`just pr-title`) | CI `pr-title`, on every PR including retitles | [#24](https://github.com/marivaldo/orca-term/issues/24) | active |
| The glossary's *Avoid* synonyms never appear in code or docs, and the config file is only ever called `orca-term.yaml` | `cargo xtask vocab` (waivers in `docs/vocabulary-waivers.md`) | CI `meta` | #27 | active |
| Coding agents never skip hooks or signing | `.claude/hooks/git-guard.sh`, tested by `just guard-test` | agent sessions, CI `meta` | #27 | active |
| main takes changes only through a PR with `check` green and signed commits; no force-push or deletion; PRs land as merge commits, so each commit keeps its own signature and linted message | ruleset `main` on the default branch (maintainer may bypass), merge commits only | GitHub | #27 | active |
| The fleet is `git worktree list` minus the primary checkout; a hand-made git worktree is listed as a worktree | `tests/worktree_ls.rs` | CI `rust` | #27 | active |
| The primary checkout is never a worktree | `tests/worktree_ls.rs`, unit tests in the fleet module | CI `rust` | #27 | active |
| Worktrees sharing a basename are told apart by path | `tests/worktree_ls.rs` | CI `rust` | #27 | active |
| Every `--json` output carries `contract` and `version` | `tests/worktree_ls.rs` | CI `rust` | #27 | active |
| The core never writes on a read path | `tests/worktree_ls.rs` (listing leaves status and refs untouched) | CI `rust` | #27 | active |
| State files carry `schema`; an older core refuses to write a newer schema | unit tests in the state module (`refuses_to_write_over_a_newer_schema`, `reads_any_schema_and_ignores_unknown_fields`), `tests/worktree_new.rs` (`the_worktree_state_is_written_to_the_admin_dir_and_lists_as_no_agent`) | CI `rust` | [#28](https://github.com/marivaldo/orca-term/issues/28) | active |
| Worktrees never share dependency trees | `tests/worktree_new.rs` (`the_copy_list_copies_ignored_files_and_never_shares_them`: plain byte copies, no shared inode, symlinks skipped) | CI `rust` | #28 | active |
| Config precedence is printed; unknown keys refused | `tests/worktree_new.rs`, unit tests in the config module | CI `rust` | #28 | active |
| `worktree rm` keeps an unmerged branch unless `--force`, and never deletes the default branch | `tests/worktree_rm.rs` (`removing_a_worktree_with_an_unmerged_commit_keeps_its_branch`, `removing_a_worktree_with_no_new_commits_deletes_its_branch`, `force_deletes_an_unmerged_branch`, `the_default_branch_is_never_deleted_even_with_force`, `a_branch_checked_out_in_another_worktree_is_kept`) | CI `rust` | #28 | active |
| Only `worktree prune` cleans a broken worktree; no command prunes implicitly | `tests/worktree_rm.rs` (`a_broken_worktree_is_refused_and_points_to_prune`, `prune_cleans_a_broken_worktree_and_reports_it`, `no_other_command_prunes_a_broken_worktree`), `tests/worktree_ls.rs` (`a_worktree_whose_directory_is_gone_lists_as_broken_and_is_not_pruned`) | CI `rust` | #28 | active |
| A worktree whose directory is gone, or whose state cannot be read, lists as broken without failing the fleet | `tests/worktree_ls.rs` (`a_worktree_whose_directory_is_gone_lists_as_broken_and_is_not_pruned`, `a_worktree_whose_state_cannot_be_parsed_lists_as_broken_without_failing_the_fleet`) | CI `rust` | #28 | active |
| Writes to a worktree's state hold a per-worktree lock | seam 1 concurrency test | CI `rust` | [#29](https://github.com/marivaldo/orca-term/issues/29) | planned |
| Lua formatted, linted and type-checked | stylua, selene, LuaLS `--check` | pre-commit, CI | [#30](https://github.com/marivaldo/orca-term/issues/30) | planned |
| The client refuses a core with another contract | seam 2 test | CI | #30 | planned |
| Sidebar: golden render of every state and mark, no row wider than the sidebar, primary checkout never a row, glyphs link to `Diagnostic*` | seam 2 golden tests | CI | #30 | planned |
| No global keymap is installed | seam 2 test | CI | #30 | planned |
| Removal needs confirmation; worktree state is independent of the tab layout | seam 2 tests | CI | [#31](https://github.com/marivaldo/orca-term/issues/31) | planned |
| No launch carries `--dangerously-skip-permissions`, `bypassPermissions` or `--auto` | source scan and seam 1 test | CI | [#32](https://github.com/marivaldo/orca-term/issues/32) | planned |
| The sandbox confines each agent's writes to its worktree and denies our state | boundary test per agent, real OS sandbox | CI (macOS, Linux) | #32, [#35](https://github.com/marivaldo/orca-term/issues/35) | planned |
| The holder survives its client and replays the scrollback | seam 1 and seam 2 tests | CI | #32, [#33](https://github.com/marivaldo/orca-term/issues/33) | planned |
| Nothing is written to an agent's global config | seam 1 test | CI | [#34](https://github.com/marivaldo/orca-term/issues/34) | planned |
| Turns are numbered per worktree and turn files are append-only; denials are never a state | seam 1 tests | CI | #34, [#42](https://github.com/marivaldo/orca-term/issues/42) | planned |
| Release binaries match their checksums | release workflow check | CI | [#43](https://github.com/marivaldo/orca-term/issues/43) | planned |
| Behaviour holds against the real agents | `just check-live` | maintainer, before each release | [#44](https://github.com/marivaldo/orca-term/issues/44) | planned |
