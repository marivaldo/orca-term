---
status: accepted
---

# One repo, release binaries fetched by the plugin, and contract and schema integers

The Rust core and the nvim client ship from **one repository**. `lua/` and `plugin/` sit at the
root, so a plugin manager installs it from the same URL, and the crate lives beside them. One tag
versions both halves.

CI publishes a binary for each tag, for macOS (arm64, x86_64) and Linux (x86_64, arm64), with
sha256 checksums. The plugin's build hook downloads the binary that matches the installed tag,
verifies it, and falls back to `cargo build --release`, as blink.cmp does. The binary stays at a
fixed path inside the plugin, and that path is what takeover hooks call. `:OrcaTerm install-cli`
symlinks it into `~/.local/bin` for use from the shell.

Two integers keep the halves in step:
- **`contract`**: every `--json` output carries it, and the client refuses to act on a contract it
  does not speak, naming the fix. It rises only when the contract breaks.
- **`schema`**: every state file carries it. A newer core migrates older files forward, only when
  it writes. A core older than a file's schema refuses to write that lane. Reading never migrates.

Takeover hooks are injected per launch: `claude --settings <generated>` and
`OPENCODE_CONFIG_DIR`. Nothing is ever written to an agent's global config.

Decided in [issue #23](https://github.com/marivaldo/orca-term/issues/23).

## Considered options

- **Two repos** (core and `orca-term.nvim`). Rejected: every contract change becomes two
  coordinated PRs and a version pairing across repos.
- **The binary on `PATH`, installed separately** (`cargo install`, Homebrew). Rejected as the
  default: plugin and binary drift apart on every update. Homebrew may come later as an extra
  channel.
- **Always build with cargo.** Rejected: it needs Rust on every machine and takes minutes per
  update.
- **Exact version match** between client and core. Rejected: every release would refuse to run
  until the binary is upgraded, even when nothing in the contract changed.
- **Unversioned, additive-only state.** Rejected: the first incompatible change could not be
  detected.
- **Installing hooks into `~/.claude/settings.json` and `~/.config/opencode`.** Rejected: it edits
  the person's own configuration and leaves traces on uninstall.

## Consequences

- Only macOS and Linux are supported. Windows has no sandbox that ADR 0007 accepts.
- Minimum versions: the agents' floor is the versions measured on this map (claude 2.1.295,
  opencode 1.18.0), and `doctor` warns, without refusing, when an agent is newer than the tested
  version. git and nvim have feature floors (git ≥ 2.38, nvim ≥ 0.11), and CI tests both the
  floor and the stable release.
- `orca-term doctor` (text or `--json`) and `:checkhealth orca-term` report missing
  prerequisites: the sandbox, git, the agents, the contract and diffview.nvim. Every command that
  needs one refuses with the same diagnosis. Nothing installs anything on the person's behalf.
- The harness checks the client's contract against the core's, runs schema-migration tests over
  fixtures of every schema, verifies release checksums, and asserts that no command writes an
  agent's global config.
