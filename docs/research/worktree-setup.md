# Making a fresh git worktree actually runnable

Research for orca-term's worktree provisioning design.

- **Date:** 2026-10-07
- **Branch:** `research/worktree-setup`
- **Machine all measurements were taken on:** macOS 26.5.2 (build 25F84), Darwin 25.5.0, arm64 (M1 Pro / T6000), APFS, `git version 2.55.0`, Claude Code 2.1.293.

## How to read this document

Every claim is tagged:

- **[measured]** — I ran it on this machine. Commands and numbers are reproducible from the Appendix.
- **[source]** — read from first-party source code or a shipped binary.
- **[docs]** — read from official documentation, URL cited.
- **[unverified]** — could not confirm from a primary source. Do not build on these.
- **[conflict]** — documentation and measurement disagree. Three of these exist; they are the most important findings in the document.

Where a claim came from a delegated reading pass rather than my own hands, it is tagged with its source type and URL, and I say so.

---

## Executive summary

The problem is real and nobody has fully solved it. The ecosystem has converged on **two mechanisms, not one**, and the interesting design question is not *what* but *when*.

1. **Prior art.** Every mature tool separates a *declarative copy/share list for gitignored files* (fast, deterministic) from a *setup shell script* (slow, imperative). `.worktreeinclude` at the repo root has become a de facto cross-tool convention — Claude Code, Orca, Conductor and Worktrunk all read it, and I confirmed the string is present in the Claude Code binary installed on this machine **[measured]**. The blocking question splits the field roughly in half, and only Worktrunk and Orca expose it as a setting.

2. **Git hooks.** `post-checkout` *does* fire on `git worktree add`, synchronously, with CWD set to the new worktree **[measured]**. But it is a weak foundation: it does not fire with `--no-checkout`, its exit code fails the command *without rolling back the worktree*, it cannot distinguish a worktree-add from a clone by its arguments, and it is silently disabled on any machine with a global `core.hooksPath` that does not chain — which is this machine's default configuration **[measured]**. There is no worktree-specific hook.

3. **Cheap copies.** APFS cloning is excellent and cheap — a 309 MB / 40k-file tree clones in 6.1s consuming 12 MB instead of 317 MB **[measured]**. But **"clone `node_modules` and it just works" is false for pnpm and catastrophically false for Python.** A cloned pnpm tree silently resolves modules out of the *source* worktree; a cloned CPython venv silently runs the *source* venv's interpreter and installs into the *source* venv's site-packages. Both fail open, not closed. See [The three things that contradict the premise](#the-three-things-that-contradict-the-premise).

---

## The three things that contradict the premise

Stated up front because they are the reason this document exists.

### 1. A cloned pnpm `node_modules` leaks module resolution into the source worktree

pnpm's `node_modules/.bin/*` entries are **not symlinks**. They are generated `/bin/sh` shims, and they bake **absolute** `NODE_PATH` entries. In a 3-dependency project, 6 of 162 files contain the project's absolute path **[measured]**:

```
.bin/tsc
.bin/tsserver
.bin/esbuild
.pnpm/typescript@5.7.3/node_modules/typescript/node_modules/.bin/tsc
.pnpm/typescript@5.7.3/node_modules/typescript/node_modules/.bin/tsserver
.pnpm/esbuild@0.25.0/node_modules/esbuild/node_modules/.bin/esbuild
```

The shim's *exec target* is relative (`basedir=$(dirname "$0")`), so the binary **still runs** after a clone and reports the right version. What is wrong is invisible:

```sh
# inside .bin/tsc, in the NEW worktree, after cp -Rc
export NODE_PATH=".../fs-lab/src-pnpm/node_modules/.pnpm/typescript@5.7.3/node_modules/typescript/node_modules:..."
#                       ^^^^^^^^^ the ORIGINAL worktree
```

I proved the leak is live, not theoretical. I planted a package resolvable only in worktree A, then resolved it from a process launched with worktree B's shim `NODE_PATH` **[measured]**:

```
resolved leakprobe -> .../fs-lab/src-pnpm/node_modules/.pnpm/node_modules/leakprobe/index.js
```

Two agents on branches with different lockfiles will silently resolve each other's dependencies. This fails open: no error, wrong code.

**Partial repair exists, and is itself a trap.** `pnpm install --frozen-lockfile` in the relocated worktree rewrites the top-level shims in **0.50s** — while printing only `Already up to date`, never mentioning the repair **[measured]**. But it **does not repair the 3 nested shims under `.pnpm/<pkg>/node_modules/.bin/`**, which remain stale **[measured]**. Those are what a package's own lifecycle script or a nested bin invocation uses.

**And the clone buys nothing here anyway**: a warm `pnpm install` into an *empty* `node_modules` also takes 0.5s **[measured]**. For pnpm, just run the install.

### 2. A cloned CPython venv runs the source venv's interpreter

This is the worst case found. **486 of 1078 files** in a `python3 -m venv` tree contain the venv's own absolute path, plus one absolute symlink **[measured]**.

- `./.venv/bin/python` in the clone **works correctly** and reports the new `sys.prefix` (Python derives prefix from `argv[0]`). This is the misleading part — a smoke test passes.
- `./.venv/bin/pip` has shebang `#!/<original abs path>/.venv/bin/python3.14`, so it executes the **original** interpreter. In the clone, `pip --version` reported the **source** venv's site-packages **[measured]**. `pip install` in worktree B installs into worktree A.
- `bin/activate` hardcodes `VIRTUAL_ENV=<original path>`. After sourcing the *clone's* activate, `which python`, `which pip` and `sys.prefix` all pointed entirely at the **source** venv **[measured]**. The agent in worktree B is operating in worktree A's environment, silently.
- 5 console scripts carried absolute shebangs in a one-dependency project (`pip`, `pip3`, `pip3.14`, `idna`, `normalizer`) — the count scales with installed packages.

CPython says so explicitly **[docs]** (https://docs.python.org/3/library/venv.html):

> "Because scripts installed in environments should not expect the environment to be activated, their shebang lines contain the absolute paths to their environment's interpreters. **Because of this, environments are inherently non-portable, in the general case.** […] If for any reason you need to move the environment to a new location, you should recreate it at the desired location and delete the one at the old location."

and in its summary bullets: *"Not considered as movable or copyable – you just recreate the same environment in the target location."*

**`uv venv --relocatable` genuinely fixes it.** 0 of 143 files contain the source path; `activate` derives `VIRTUAL_ENV` from `realpath` of the script; console scripts get `#!/bin/sh` shims. A cloned relocatable venv reported the correct new `sys.prefix` and `VIRTUAL_ENV` **[measured]**. Caveat from uv's own source **[source]**: relocatability "can only be guaranteed for standard `console_scripts` and `gui_scripts`" — raw binaries in `bin/` are left as-is.

### 3. Symlinking the whole directory is destroyed by the first install, and hardlinking shares writes

- **Symlink** `node_modules` → the first `npm install` in the worktree **deletes the symlink** and does a full install: `npm warn reify Removing non-directory .../node_modules` **[measured]**. Not corruption, but the strategy silently evaporates exactly when the agent needs it.
- **Symlink**, before that: `require.resolve("typescript")` returns a path inside the **source** worktree, because Node realpaths symlinks by default **[measured]**. Every dependency's `__dirname` escapes the worktree, so tooling that derives a project root or cache location from a dependency's path lands in the wrong worktree.
- **Hardlink** (`cp -Rl`) → source and worktree share inodes (`links=2`, same inode number). Appending to the worktree's copy of `typescript/package.json` mutated the source file **[measured]**. Anything that writes in place — `patch-package`, `node_modules/.cache`, native rebuild artifacts — contaminates every worktree.
- A worktree-local `node_modules` **symlink** also breaks `git worktree remove`: a trailing-slash `.gitignore` pattern like `node_modules/` does not match a symlink, so git reports it untracked and refuses removal **[docs]** (https://github.com/k1LoW/git-wt). Drop the trailing slash or use `.git/info/exclude`.

---

## Part 1 — Prior art

### 1.1 Summary table

| Tool | Copy/share mechanism | Config file | Setup script | Blocks the agent? |
|---|---|---|---|---|
| **Claude Code** | `.worktreeinclude` copy list (gitignore syntax, intersected with actually-ignored files) | `.worktreeinclude` (repo root) | none for the default git path | copy is synchronous; `WorktreeCreate` hook *replaces* creation and blocks |
| **Orca** (stablyai/orca) | 3-way: Settings shared paths (APFS clone or symlink), `worktree.sharedDirectories` (always link), `.worktreeinclude` (always private copy) | `orca.yaml` + `.worktreeinclude` | `scripts.setup` | **configurable**: `setupAgentStartupPolicy: wait-for-setup \| start-immediately`, default `start-immediately` |
| **Conductor** | `file_include_globs` or `.worktreeinclude`; default `.env*` | `.conductor/settings.toml` (legacy `conductor.json`) | `scripts.setup` | **[unverified]** — never stated on any of 8 doc pages checked |
| **Crystal** (stravu/crystal) | **none at all** | SQLite column, GUI only | `build_script` | **yes, hard-coded** |
| **vibe-kanban** | `copy_files` (comma-separated globs, plain `fs::copy`) | SQLite column, GUI only | `setup_script` | **configurable**: `parallel_setup_script`, default 0 = sequential |
| **Sculptor** (imbue-ai) | only its own `.sculptor/.env` | DB field `workspace_setup_command` | same | **no** — async thread + a system reminder carrying the PID |
| **Worktrunk** (`wt`) | `wt step copy-ignored` — **reflink/CoW**, narrowed by `.worktreeinclude` | `.config/wt.toml` + `.worktreeinclude` | `[[pre-start]]` / `[[post-start]]` | **both offered**: `pre-start` blocks, `post-start` backgrounds |
| **gtr** (coderabbitai) | `copy.include/exclude/includeDirs/excludeDirs` | `.gtrconfig` (gitconfig format) | `hooks.postCreate` | yes; requires `git gtr trust` |
| **wtp** | `type: copy` / `type: symlink` steps | `.wtp.yml` | `type: command` steps | yes |
| **workmux** | `files.copy`, `files.symlink` | `.workmux.yaml` | `post_create` | yes — "before tmux window opens" |
| **gwq** | `copy_files` globs | TOML `[[repository_settings]]` | `setup_commands` | yes, sequential; failures are warnings |
| **git-wt** (k1LoW) | `wt.copy`, `wt.copyignored`, `wt.symlink` | `git config` keys only | `wt.hook` | yes, fail-fast |
| **wtman** | `copy`, `link`, `mkdir`, `remove` actions | `.wtman/config.yaml` (3-layer) | `run` action | yes |
| **Graphite** (`gt`) | **none** — tolerates worktrees, does not create them | — | — | n/a |
| **Jujutsu** (`jj workspace add`) | **none** | — | — | n/a; no hook mechanism exists |

Not found at suggested paths: `johnalanwoods/wt` (404), `taecontrol/worktree` (404) **[measured]**. `ThePrimeagen/git-worktree.nvim` has an in-process Lua `on_tree_change` callback and no copy feature **[source]**.

### 1.2 Claude Code — verified against the binary on this machine

The docs **[docs]** (https://code.claude.com/docs/en/worktrees.md) say, verbatim:

> "A worktree is a fresh checkout, so initialize your development environment there: ask Claude to install dependencies, or run your project's setup yourself in the worktree directory under `.claude/worktrees/`. To carry gitignored files such as `.env` into every new worktree automatically, add a `.worktreeinclude` file."

That is the passage worth noting: **dependency installation is explicitly not automated.** The copy list handles secrets and local config only.

I confirmed the implementation against the shipped binary at `~/.local/share/claude/versions/2.1.293` **[measured]** — `worktreeinclude` appears 6 times, `WorktreeCreate` 45 times. Extracting the surrounding code gives the real algorithm **[source]**:

```js
async function iCo(e, n) {
  let r;
  try { r = await gue(Cd(e, ".worktreeinclude"), "utf-8") } catch { return [] }
  let s = r.split(/\r?\n/).map(he => he.trim())
            .filter(he => he.length > 0 && !he.startsWith("#"));
  if (s.length === 0) return [];
  let g = await at(Ot(), [...wn, "ls-files", "--others", "--ignored", "--exclude-stand…
```

So: read the file, strip blanks and `#` comments, then intersect the patterns with `git ls-files --others --ignored --exclude-standard`. Tracked files are never copied. The guard rails, from the binary's own warning strings **[source]**:

```
Skipping .worktreeinclude copy: realpath(…) failed: …
Skipping symlink in .worktreeinclude: …
Skipping .worktreeinclude entry: destination escapes worktree via committed symlink: …
```

Worth stealing: **symlinks in the include list are skipped outright**, and destinations that escape the worktree through a committed symlink are refused.

`WorktreeCreate` is a **replacement, not a post-create hook** **[docs]** (https://code.claude.com/docs/en/hooks.md):

> "Configuring a WorktreeCreate hook **replaces that default git behavior**… Because the hook replaces the default behavior entirely, `.worktreeinclude` is **not processed**."

Contract: stdin gets `{"hook_event_name":"WorktreeCreate","name":"<slug>",…}`; the hook prints the worktree path as the last non-empty line of stdout; **any non-zero exit fails worktree creation**. A hook-created directory *inside* a git repo is refused. There is no "worktree was created" event that merely augments the default path — `SessionStart` has no worktree matcher, and `DirectoryAdded` ("Use this to prepare a newly added repository, for example by installing its dependencies") explicitly does not fire for worktrees and is not awaited.

Also relevant and easy to miss **[docs]**: Claude Code deliberately neutralises repo-local git filter drivers during worktree creation, so **Git LFS files arrive as pointer files**; the fix is `git lfs pull` in the worktree. "The worktree doesn't run" has causes beyond missing `node_modules`.

### 1.3 Orca — the sharpest thinking on copy semantics

Real home is **https://github.com/stablyai/orca**, site **https://onorca.dev** **[measured]**. (`orcaworks.ai` is an unrelated business-process product; `zaridan/orca` is a 1-star fork.)

Orca's docs open with exactly our problem statement **[docs]** (https://www.onorca.dev/docs/model/worktrees):

> "A brand-new worktree is a clean checkout. Dependencies, caches, and local secrets that live in gitignored paths are missing until you recreate them."

Its answer is a deliberate three-way split by copy semantics:

| Mechanism | Result in a new worktree |
|---|---|
| Settings → Worktree Shared Paths | "APFS clone-copy on macOS when available; otherwise a link." |
| `worktree.sharedDirectories` (in `orca.yaml`) | "Always a link to the primary directory, **including on APFS**. … Edits affect the shared source." |
| `.worktreeinclude` | "Private copy, using APFS clone-copy when available… **It never falls back to a shared link.**" |

So Orca's documented answer to the `node_modules` question is **symlink `node_modules`, clone-copy `.env`** — and "Large dependency trees usually belong in setup or deliberate sharing rather than `.worktreeinclude`."

It shells out to plain `/bin/cp` **[source]** (`src/main/ipc/worktree-apfs-clone.ts`):

```ts
await deps.execFileAsync('/bin/cp', ['-c', source, tempTarget])                       // file
await deps.execFileAsync('/bin/cp', ['-n', '-c', '-R', `${source}${sep}.`, target])   // directory
```

…gated by a `df -P` + `diskutil info -plist` same-APFS-volume probe, 5s timeout, cached per `stat().dev`.

Two details worth copying wholesale **[docs]**:
- **Admission budget**, measured *before* copying: 2 GiB total file bytes and 50,000 filesystem entries per worktree. "APFS clones do not consume the byte budget, but still consume the entry budget."
- **Ordering**: settings paths → YAML shared directories → include copies; "a path already present through sharing is not copied again" **[source]** (`runtime-local-worktree-materialization.ts`).

Its sync gate is a setting, `setupAgentStartupPolicy: wait-for-setup | start-immediately`, default `start-immediately`, implemented as a `Promise.race` against a timeout that swallows errors — "Setup completion is evidence, not a reason to strand a launch when the PTY disappears" **[source]**.

Orca also exports `CONDUCTOR_ROOT_PATH` and `GHOSTX_ROOT_PATH` as aliases for `ORCA_ROOT_PATH` **[docs]** — deliberate cross-compatibility with Conductor's contract.

### 1.4 Conductor

Config is `.conductor/settings.toml` (TOML); `conductor.json` is legacy and ignored when the TOML exists **[docs]** (https://www.conductor.build/docs/configure-your-project).

```toml
[scripts]
setup = """
pnpm install
cp "$CONDUCTOR_ROOT_PATH/.env" .env
pnpm run build
"""
```

Copy list is `file_include_globs` (a TOML multiline string) **or** `.worktreeinclude`, with precedence `.worktreeinclude` > `file_include_globs` > the built-in default `.env*`; adding either one *replaces* the default **[docs]** (https://www.conductor.build/docs/reference/files-to-copy). Unlike Orca, Conductor accepts full gitignore glob syntax.

On `node_modules` it takes the opposite position from Orca: the docs advise **against** copying `node_modules`, `.next`, `dist`, `target` — "large, machine-specific, and easy to regenerate", and copying them "can slow workspace creation and carry stale state". Use a setup script.

**Whether the agent blocks on setup is [unverified]** — the delegated reading pass checked 8 doc pages and none states it. Nor is failure behaviour documented. Nice touch nobody else has: `CONDUCTOR_PORT`..`+9`, ten ports allocated per workspace.

### 1.5 Crystal — the naive design, and the clearest evidence of blocking

No repo config file at all; `build_script` and `run_script` are SQLite columns edited in the GUI **[source]** (`main/src/database/models.ts`). The UI tooltip is the de facto spec: *"Commands that run once when creating a new worktree. Use for setup tasks like installing dependencies."*

It has **no copy list, no symlink list, no clone-copy** — a grep across `main/src` for `copyFile|fs.cp|cpSync|cp -R|clonefile|reflink|symlink` found only unrelated hits **[source]**. So Crystal's answer is "run `npm install` in every worktree, and you're on your own for `.env`."

It is, however, the clearest blocking implementation **[source]** (`main/src/services/taskQueue.ts`):

```ts
sessionManager.emitSessionCreated(session);   // row appears in the UI immediately
if (targetProject.build_script) {
  sessionManager.updateSessionStatus(session.id, 'initializing', 'Running build script...');
  // "⏳ Waiting for build script to complete..."
  const buildResult = await sessionManager.runBuildScript(session.id, buildCommands, worktreePath);
}
// …only then start the agent panel
```

The UX trick is worth noting: the session appears instantly with a waiting status, while the *agent* is gated. Build failure is logged and the agent starts anyway.

Last pushed 2026-02-26 — the least active of the group.

### 1.6 vibe-kanban — the cleanest sync/async implementation

Columns on `repos` **[source]** (`crates/db/migrations/20260107000000_move_scripts_to_repos.sql`): `setup_script`, `cleanup_script`, `copy_files`, `parallel_setup_script`, `dev_server_script`.

Documented ordering **[docs]** (https://vibekanban.com/docs/core-features/creating-projects): *"These files will be copied after the worktree is created but before the setup script runs."* And: *"Setup scripts will be run before the coding agent is executed."*

Copy is plain `fs::copy`, glob-walked, **no reflink** — wrapped in `spawn_blocking` with a **30-second timeout** **[source]** (`crates/local-deployment/src/copy.rs`, `container.rs`). It canonicalizes both paths and rejects anything resolving outside the source root, with a regression test for `../secret.txt`.

The sync toggle is two genuinely different execution models, not a flag **[source]** (`crates/services/src/services/container.rs`): parallel = fire-and-forget processes alongside the agent; sequential = a *linked list of executor actions* built back-to-front with the coding agent as the tail. Mixed config degrades to sequential (`all_parallel` requires every repo's flag). Default is 0 = sequential. The flag is **not in the published docs** — **[unverified]** as documented, verified in source and UI.

### 1.7 Sculptor — the one genuinely different answer to "when"

The premise that Sculptor is container-per-agent is **out of date**. Its hosted docs are gone (`docs.imbue.com` now 302-redirects to the GitHub repo) and the current repo is worktree-first **[measured]**:

> "By default a workspace is a **git worktree** off your repo."

And it says why it dropped per-agent containers **[docs]** (`docs/history.md`): isolation cost flexibility ("very powerful to allow agents to inspect each other's work"), users found it confusing, "easier — and equivalent — to run the entire application in a container or VM instead of each agent", and "we made the wrong choice relying on Docker Desktop (on macOS)".

Setup is a DB field `workspace_setup_command`, default `"git fetch origin 2>/dev/null || true"`, with tri-state semantics (`null` = use default, `""` = run nothing, string = as-is) **[source]**.

It runs **asynchronously** and pushes the synchronisation decision onto the agent, via an injected reminder on the agent's first message **[source]** (`process_manager_utils.py`):

> A workspace setup command is currently running. Command: `{command}` / Bash PID: `{pid}` / Log file: `{log_path}` — "The setup command may modify workspace state — files, dependencies, git, locks — concurrently with your work. Before proceeding, consider whether your task depends on that state being settled… If so, wait on this PID; otherwise, continue."

They cap even the PID handshake at 0.5s: *"we'd rather drop the reminder than block the agent's first message"* **[source]**.

It copies exactly one gitignored file, its own `.sculptor/.env`, with the comment *"Gitignored files don't follow the worktree either"* **[source]**. `node_modules` is the setup command's problem.

All claims about `user_setup.sh` / `.devcontainer` support are **[unverified]** — they come from a docs site that no longer exists, and `grep -rn user_setup` over the current repo returns zero hits.

### 1.8 Worktrunk — the strongest prior art for the copy mechanism

Self-described as "designed for running AI agents in parallel… including hooks to automate local workflows & copy-on-write build caches" **[docs]** (https://github.com/max-sixty/worktrunk).

It ships a `.worktreeinclude` in its own repo and dogfoods the copy in `.config/wt.toml` **[source]**:

```toml
[[post-start]]
# Copy target/ from main worktree using CoW (~3s vs ~68s full rebuild)
deps = "wt step copy-ignored"
assets = "task fetch-assets"

[[post-start]]
docs-install = "npm --prefix docs install --prefer-offline --no-audit --no-fund"
```

`wt step copy-ignored` semantics **[docs]** (https://worktrunk.dev/step/):

> "Files are reflinked where the filesystem supports it, including APFS, btrfs, XFS, and ReFS. A reflinked copy shares disk blocks with the source until one side is written. On ext4 and NTFS, files are fully copied. **Reflinks work per file, so copy time scales with file count.**"

Quoted benchmark: **14 GB `target/` ≈ 20s with reflinks vs ≈ 2 min with `cp -R`**. Re-running is safe (existing destinations skipped); `.worktreeinclude` narrows the set; built-in excludes always apply plus `[step.copy-ignored] exclude`.

And it is the only tool that exposes *both* timing policies and documents the choice rule **[docs]** (https://worktrunk.dev/hook/):

> "`pre-*` hooks block — failure aborts the operation." / "`post-*` hooks run in the background with output logged." / "Use `post-start` so the copy runs in the background. Use `pre-start` if later hooks or `--execute` commands need the files immediately."

### 1.9 Patterns worth stealing from the long tail

- **Trust gating.** `gtr` requires `git gtr trust` before `.gtrconfig` commands execute; `gwq` has a trust store; Worktrunk prompts once for project hooks **[docs]**. A checked-in config that runs shell on worktree creation is a code-execution vector — and in orca-term the configs may be *written by agents*.
- **`from` vs `to` resolution.** `wtp` is explicit **[docs]**: "`from`: path is always resolved relative to the **main worktree**. `to`: path is resolved relative to the **newly created worktree**… regardless of where you run `wtp add` from." This is the kind of ambiguity that produces bug reports.
- **Fail-soft is universal.** Crystal logs and starts anyway; vibe-kanban warns and continues; `gwq` prints `[gwq] setup command error:` and does not abort; Orca's gate has a timeout. **Nobody hard-fails worktree creation on setup failure.**
- **Worktrees inside the repo cause double config loading.** `git-wt` warns that tools walking parent directories — naming Claude Code reading `CLAUDE.md` explicitly — load config from both the worktree and the main repo, and recommends `.git/wt` **[docs]**.

### 1.10 The two tools with no answer at all

- **Graphite** has no worktree command. The full `gt` command reference contains none **[measured]**. It only *tolerates* worktrees: "If a command would need to rewrite or check out a branch in another worktree, Graphite exits with an informative error" **[docs]**. No hook, no setup, no mention of dependencies. (Search results associating "WorktreeCreate hooks" with Graphite are describing a third-party *Claude Code* plugin — do not conflate.)
- **Jujutsu.** `jj workspace add` has no option that copies ignored files and none that runs a command. `--sparse-patterns copy` copies *pattern settings*, not files — easy to misread **[docs]**. The working-copy docs never mention carrying ignored files into a new workspace, and **jj has no generalized hook mechanism**: https://github.com/jj-vcs/jj/issues/3577 ("FR: Generalized hook support") is still **open**, filed 2024-04-26 **[docs]**. A fresh `jj workspace add` has our problem with no built-in remedy.

### 1.11 The pnpm official answer, which is neither clone nor symlink

pnpm ships a page titled **"pnpm + Git Worktrees for Multi-Agent Development"** describing this exact architecture **[docs]** (https://pnpm.io/git-worktrees):

> "When multiple AI agents need to work on the same monorepo simultaneously, they each need an isolated working copy with fully functional `node_modules`. […] each worktree gets its own checkout and its own `node_modules`, but dependencies are shared across all of them through a single content-addressable store on disk."

> "**Near-zero per-worktree overhead** — the local `node_modules` contains only symlinks to the shared global virtual store. Unlike pnpm's default behavior, which hardlinks files from the content-addressable store into a local `node_modules/.pnpm` directory, the global virtual store means no files are copied or hardlinked into the worktree at all."

> "**No conflicts** — each worktree has its own `node_modules` tree, so agents can install different dependency versions on different branches without interference."

Mechanism: bare clone → `git worktree add` per branch → `virtualStoreType: global` in `pnpm-workspace.yaml` → ordinary `pnpm install` in each worktree, which is "nearly instant because they only create symlinks to the same store."

Caveats pnpm states itself **[docs]** (https://pnpm.io/global-virtual-store):
- "currently **disabled by default** for project installs and **marked as experimental**, as some tools may not work correctly with symlinked `node_modules`."
- "**ESM hoisting**: pnpm uses the `NODE_PATH` environment variable… and **Node.js does not respect `NODE_PATH` for ESM imports.**" Since v11.23.0 pnpm injects `NODE_OPTIONS --import` hooks into processes *it* spawns — "**A `node` process started outside pnpm does not get that environment.**" That matters directly: orca-term spawns agents, and agents spawn `node`.
- "**Do not use one writable pnpm store for mutually untrusted agents or users.**"

Yarn Berry **zero-installs** dissolves the problem entirely: `.yarn/cache` and `.pnp.cjs` are *tracked files*, so `git worktree add` materialises a working install with no mechanism at all **[docs]** (https://yarnpkg.com/features/caching). With the default `enableGlobalCache: true`, `.pnp.cjs` paths are `path.relative(project.cwd, …)` unconditionally, so a global cache is reached via `../../../…` — making the manifest **depth-sensitive** rather than path-portable **[source]**, `packages/plugin-pnp/sources/PnpLinker.ts`. That inference is from source, not a doc claim.

---

## Part 2 — Git's own hooks on worktree creation

### 2.1 The documentation

`githooks(5)` as shipped with git 2.55.0 on this machine, verbatim **[docs]**:

> **post-checkout** — "This hook is invoked when a `git-checkout(1)` or `git-switch(1)` is run after having updated the worktree. The hook is given three parameters: the ref of the previous HEAD, the ref of the new HEAD (which may or may not have changed), and a flag indicating whether the checkout was a branch checkout (changing branches, flag=1) or a file checkout (retrieving a file from the index, flag=0). This hook cannot affect the outcome of `git switch` or `git checkout`, other than that the hook's exit status becomes the exit status of these two commands."
>
> "It is also run after `git-clone(1)`, unless the `--no-checkout` (`-n`) option is used. The first parameter given to the hook is the null-ref, the second the ref of the new HEAD and the flag is always 1. **Likewise for `git worktree add` unless `--no-checkout` is used.**"

That last sentence is the version-accurate answer. Everything below is my verification of it.

### 2.2 What actually fires — measured

| Command | `post-checkout` fires? |
|---|---|
| `git worktree add ../wt -b new-branch` | **yes** |
| `git worktree add ../wt existing-branch` | **yes** |
| `git worktree add --detach ../wt HEAD` | **yes** |
| `git worktree add --no-checkout ../wt -b b` | **no** |
| `git worktree repair <path>` | **no** |
| `git worktree move <src> <dst>` | **no** |
| `post-merge` on `git worktree add` | **no** |

**[measured]** — all six in one run against a scratch repo; full transcript in the Appendix.

### 2.3 Arguments and environment — measured

```
argc=3  a1=[0000000000000000000000000000000000000000]  a2=[bd15e276…]  a3=[1]
PWD=.../wt-lab/wt-new                       <- the NEW worktree root
GIT_DIR=<unset>   GIT_WORK_TREE=<unset>     <- not exported into the hook's env
toplevel       = .../wt-lab/wt-new
git-dir        = .../wt-lab/main-repo/.git/worktrees/wt-new
git-common-dir = .../wt-lab/main-repo/.git
```

- `$1` is the **null ref**, not the previous HEAD — identical to what `git clone` passes. **A `post-checkout` hook cannot tell a worktree-add from a clone by its arguments.** The reliable discriminator is `git rev-parse --git-dir` containing `/worktrees/` — in a clone, `--git-dir` and `--git-common-dir` are both `.git` **[measured]**.
- CWD is the new worktree root, which is convenient — a setup script needs no argument parsing to know where it is.

### 2.4 Synchronous and blocking — measured

A hook that `sleep 3` then writes a marker:

```
elapsed: 3.40s
../wt-sync/HOOK_MARKER present immediately after add returns
```

So `git worktree add` waits. Setup can be hung off it synchronously.

### 2.5 But the failure semantics are wrong for setup — measured

A hook exiting 42:

```
Preparing worktree (new branch 'feat-fail')
HEAD is now at bd15e27 init
hook says no
git worktree add exit code: 42
worktree dir exists? yes
registered in git worktree list? yes
HEAD in failed worktree: feat-fail
```

**The exit code propagates but the worktree is fully created and registered.** There is no rollback. A failed setup leaves a half-provisioned worktree behind plus a nonzero exit — the caller must clean up itself. (This matches the doc's "the hook's exit status becomes the exit status of these two commands" and its explicit "This hook cannot affect the outcome.")

### 2.6 Hooks live in the common dir, not per worktree — measured

I placed a hook **only** in `.git/worktrees/wt-new/hooks/post-checkout` and ran `git checkout` inside that worktree. It did **not** fire; only the common-dir hook did. So all worktrees of a repo share one hook set, and a hook cannot be installed for one worktree only.

Corroborated by `git rev-parse --git-path hooks/<name>`, which resolves to the hooks directory git will actually consult **[measured]**.

### 2.7 There is no "worktree created" hook — and `post-checkout` is fragile in practice

`githooks(5)` on 2.55.0 enumerates 28 hooks **[measured]**: `applypatch-msg`, `pre-applypatch`, `post-applypatch`, `pre-commit`, `pre-merge-commit`, `prepare-commit-msg`, `commit-msg`, `post-commit`, `pre-rebase`, `post-checkout`, `post-merge`, `pre-push`, `pre-receive`, `update`, `proc-receive`, `post-receive`, `post-update`, `reference-transaction`, `push-to-checkout`, `pre-auto-gc`, `post-rewrite`, `sendemail-validate`, `fsmonitor-watchman`, `p4-*` (4), `post-index-change`. **None is worktree-specific.** `post-checkout` is the only one that fires.

**The practical fragility, found by accident on this machine [measured].** This machine has a global `core.hooksPath` set to `~/.config/git/hooks`. Git consults *only* the configured directory:

```
post-checkout        -> /Users/…/.config/git/hooks/post-checkout
reference-transaction -> /Users/…/.config/git/hooks/reference-transaction
post-index-change    -> /Users/…/.config/git/hooks/post-index-change
```

A repo-local `.git/hooks/post-checkout` worked here **only because** that directory happens to contain a hand-written dispatcher that chains to `$(git rev-parse --git-common-dir)/hooks/<name>` — and whose own comment flags the subtlety: *"`--git-common-dir`, not `--git-dir`: in a worktree `--git-dir` is `.git/worktrees/<name>`, which has no `hooks/`."*

**Consequence for orca-term: a repo-local `post-checkout` is silently dead on any machine with a global hooks path that does not chain — which includes the common husky/lefthook setups.** Do not make it the only mechanism. If used at all, verify with `git rev-parse --git-path hooks/post-checkout` and warn when it does not resolve into the repo.

Caveat on my own method: because of this, my negative results for `reference-transaction` and `post-index-change` on `git worktree add` are **not isolable on this machine** — the global directory lacks those two filenames, so repo-local copies are never consulted. The sandbox forbade overriding the hooks path to isolate it. `post-checkout` and `post-merge` results are valid, since those names *are* present and chained.

### 2.8 The ordering constraint that rules out pre-staging — measured

```
$ mkdir -p prefilled/node_modules && echo x > prefilled/node_modules/a
$ git worktree add prefilled -b feat-pre
Preparing worktree (new branch 'feat-pre')
fatal: '…/prefilled' already exists
```

A directory containing **anything** — even a single `.keep` — is refused. An **empty** existing directory is accepted **[measured]**. So you cannot populate `node_modules` first and then run `git worktree add` over it. The order is forced: `worktree add` → then copy.

---

## Part 3 — Cheap physical copy on macOS and Linux

### 3.1 The APIs

**macOS `cp -c`**, verbatim from `man cp` on this machine **[docs]**:

> "-c    copy files using `clonefile(2)`. Note that if the source and target are on different filesystems, or the target filesystem does not support cloning, **cp will fallback to using `copyfile(2)` instead to ensure the copy still succeeds.**"

So `cp -c` **never fails for lack of cloning** — it silently degrades to a full copy. If you need to know which happened, you must probe the filesystem yourself (as Orca does with `df -P` + `diskutil`).

`man 2 clonefile` has a warning implementers should read **[docs]**:

> LIMITATIONS — "**Cloning directories with these functions is strongly discouraged.** Use `copyfile(3)` to clone directories instead."

So do not call `clonefile()` on `node_modules` directly; drive `/bin/cp -c -R` or `copyfile(3)`.

**Linux GNU `cp --reflink`**, verbatim from GNU coreutils 9.11's man page **[docs]**:

> "By default or with `--reflink=auto`, cp will try a lightweight copy, where the data blocks are copied only when modified, **falling back to a standard copy if this is not possible**. With `--reflink[=always]` cp will **fail if CoW is not supported**, while `--reflink=never` ensures a standard copy is performed."

Note the asymmetry: BSD `-c` always falls back; GNU `--reflink=always` fails loudly, `=auto` falls back. `=auto` is the right default; `=always` is the right *probe*.

### 3.2 Measured numbers on this machine (APFS)

**Small tree — 41 MB, 152 files (a real `npm install` of esbuild + typescript + chalk):**

| method | elapsed | disk consumed |
|---|---|---|
| `cp -Rc` (clonefile) | **0.03s** | **184 KiB** |
| `cp -R` (full copy) | 0.13s | 41,988 KiB |
| `cp -Rc` → HFS+ image | 0.29s | full copy (silent fallback, all 152 files present) |

**Large tree — 309 MB, 40,006 files (synthetic, 400 packages × 100 small files + 6 × 12 MB binaries):**

| method | elapsed | disk consumed |
|---|---|---|
| `cp -Rc` (clonefile) | **6.12s** | **12,596 KiB** |
| `cp -R` (full copy) | 13.65s | 316,780 KiB |
| GNU `cp -a --reflink=always` | 5.45s | (see below) |

**220 MB, 5,001 files**, clean space measurement:

| method | disk consumed |
|---|---|
| GNU `cp -a --reflink=always` | 1,544 KiB |
| BSD `cp -Rc` | 1,764 KiB |

All **[measured]**. File counts verified identical across source and all copies.

Two observations:

- **Clone cost scales with file count, not bytes.** 41 MB / 152 files → 0.03s; 309 MB / 40k files → 6.12s. That matches Worktrunk's documented "reflinks work per file, so copy time scales with file count." A real `node_modules` is tens of thousands of tiny files, so budget seconds, not milliseconds. The win over a full copy is ~2.2x in time and ~25x in space at this scale.
- **GNU coreutils `--reflink` works on macOS/APFS** — `rc=0` for both `auto` and `always`, consuming clone-like space **[measured]**. This contradicts the common belief that `--reflink` is a Linux-only `FICLONE` ioctl. Useful if you prefer one code path, though BSD `cp -c` is already present everywhere on macOS.

**On a first attempt the 309 MB GNU measurement returned a *negative* space delta (−1,036,416 KiB)** — free space grew during the copy, because APFS reclaims purgeable space in the background. I re-ran smaller and with `sync` + settling delays to get the clean numbers above. Any `df`-based space measurement on APFS needs that treatment; treat single-shot deltas with suspicion.

### 3.3 When cloning is not available — measured

A C probe calling `clonefile(2)` directly:

| case | result |
|---|---|
| APFS → APFS, same volume | `clonefile OK` |
| HFS+ → HFS+ (200 MB disk image) | `clonefile FAILED errno=45 (Operation not supported)` |
| APFS → HFS+ (cross-device) | `clonefile FAILED errno=18 (Cross-device link)` |

`cp -Rc` across the same boundaries succeeded in 0.29s with all files intact — i.e. the fallback is real and silent **[measured]**.

Practical mapping: **APFS, btrfs, XFS (with reflink=1), ReFS → reflinks; HFS+, ext4, NTFS, FAT, most network mounts → full copy** (first two **[measured]**, the rest **[docs]**, Worktrunk). Cross-volume is always a full copy regardless of filesystem.

### 3.4 Is a cloned dependency directory valid at a different absolute path?

I audited each tree byte-exactly with a Python walker (not `grep -r`, see the method note below), counting files whose contents include the source project's absolute path, plus absolute symlinks **[measured]**:

| tree | files w/ source abs path | absolute symlinks | verdict |
|---|---|---|---|
| **npm** `node_modules` | **0 / 153** | 0 | relocatable |
| **yarn classic** `node_modules` | **0 / 152** | 0 | relocatable |
| **pnpm 11.13.1** `node_modules` | **6 / 162** | 0 | **broken** — all 6 are `.bin` shims |
| **CPython 3.14 venv** | **486 / 1078** | 1 | **broken** |
| **uv venv `--relocatable`** | **0 / 143** | 1 (to the brew interpreter) | relocatable |
| **bundler `vendor/bundle`** | **0 / 63** | 0 | relocatable |

Then the functional tests.

**npm — works.** `.bin` entries are **relative symlinks** (`tsc -> ../typescript/bin/tsc`), preserved as symlinks by `cp -Rc`. After cloning to a new path: `tsc --version` → 5.7.3, `esbuild --version` → 0.25.0, `npm ls --depth=0` clean **[measured]**. Corroborated in source **[source]** (`npm/bin-links/lib/link-bins.js`): `const from = relative(dirname(to), absFrom)`. `node_modules/.package-lock.json` keys paths relative to the project root (`"node_modules/@esbuild/darwin-arm64"`), no absolute paths **[measured]**.

One thing I did **not** verify: npm's hidden-lockfile fast path requires the lockfile's mtime to be "at least as recent as every package folder it references" **[docs]** (https://docs.npmjs.com/cli/v11/configuring-npm/package-lock-json). Whether `clonefile` preserves mtimes such that the fast path survives a clone is **[unverified]** — it decides whether the next `npm install` is instant or re-scans the whole tree. Worth a one-line test before shipping.

**yarn classic — works**, same argument. Source comment is explicit **[source]** (`yarn/src/util/fs.js`): *"use relative paths otherwise which will be retained if the directory is moved."*

**pnpm — broken.** See [finding 1](#1-a-cloned-pnpm-node_modules-leaks-module-resolution-into-the-source-worktree).

**CPython venv — broken.** See [finding 2](#2-a-cloned-cpython-venv-runs-the-source-venvs-interpreter).

**bundler — works.** `.bundle/config` holds `BUNDLE_PATH: "vendor/bundle"`, a **relative** value; the gem binstub shebang is the portable `#!/usr/bin/env ruby`; `Gemfile.lock` contains no paths. After cloning `.bundle` + `vendor` to a new path: `bundle exec rake --version` → `rake, version 13.2.1`, `bundle check` → "The Gemfile's dependencies are satisfied" **[measured]**. Corroborated in source **[source]** (`lib/bundler/settings.rb`): the configured value is stored verbatim and expanded against `Bundler.root` at use time.

Two Ruby caveats: my test gem (`rake`) is pure Ruby, so **native-extension gems are [unverified]**; and `.bundle/` is conventionally gitignored, so a fresh worktree has **no `.bundle/config`** and Bundler silently falls back to system gems even with `vendor/bundle` cloned in. Copy `.bundle/config` too, or set `BUNDLE_PATH`.

**bun — [unverified], not installed on this machine.** From docs **[docs]** (https://bun.com/docs/pm/cli/install): `--backend` defaults to **`clonefile` on macOS**, `hardlink` on Linux/Windows, with `copyfile` as fallback. So a default bun `node_modules` on macOS is already real CoW-cloned files with relative `.bin` links, and cloning it gets the same economics bun itself uses. Isolated installs put relative symlinks under `node_modules/.bun` **[docs]**; with `install.globalStore` the docs show an absolute-looking `~/.bun/install/cache/links/…` target but never state the format — **[unverified]**, worth a `readlink`. bun has no relocatability statement and no worktree documentation.

**Native `.node` addons — safe.** node-gyp sets `DYLIB_INSTALL_NAME_BASE: '@rpath'` and `-undefined dynamic_lookup`, so an addon does not link against the `node` binary at all **[source]** (`node-gyp/addon.gypi`). Absolute paths that *do* appear are the build machine's and are inert: a delegated `otool` pass found `next-swc.darwin-arm64.node` carrying `LC_ID_DYLIB name /Users/runner/work/next.js/…` — a GitHub Actions path that has never existed locally — and it loads fine, because `dlopen` of a bundle by explicit path ignores `LC_ID_DYLIB` **[measured, delegated]**. Platform binaries (esbuild/sharp style) are selected by npm's `os`/`cpu`/`libc` fields: **platform-locked, not path-locked** **[docs]**. Within one machine and arch a clone is safe.

### 3.5 Clone vs symlink vs shared store

| | clone (reflink) | symlink the directory | shared store |
|---|---|---|---|
| speed | seconds, scales with file count | instant | fast after first install (pnpm: 0.5s **[measured]**) |
| disk | ~metadata only (184 KiB for 41 MB) **[measured]** | zero | one copy total |
| isolation | **full** — diverges on write | **none** — all worktrees share one mutable tree | per-worktree `node_modules`, shared immutable store |
| different lockfiles per branch | fine | **corrupts every worktree** | fine (pnpm's documented "No conflicts") |
| survives the agent running the package manager | yes | **no** — `npm install` deletes the symlink **[measured]** | yes |
| correctness | **ecosystem-dependent** — see the table above | dependency `__dirname` escapes the worktree **[measured]** | the sanctioned answer for pnpm; mandatory for Python |
| non-CoW filesystem | silently becomes a full copy | unaffected | unaffected |

**Mapping to orca-term:**

- **npm, yarn classic, bun (hoisted, macOS)** → clone. Fast, isolated, correct.
- **pnpm** → do **not** clone. Run `pnpm install` (0.5s warm **[measured]**), ideally with `virtualStoreType: global`. If you clone anyway, you must repair *both* the top-level and the nested `.pnpm/**/.bin` shims yourself — pnpm's own install only fixes the former.
- **yarn Berry zero-installs** → do nothing; the deps are tracked files.
- **Python** → never clone or symlink a venv. Create a fresh one per worktree: `uv venv` with a warm `UV_CACHE_DIR` on the same filesystem. If a venv must be moved, `uv venv --relocatable`, accepting that only `console_scripts`/`gui_scripts` are covered.
- **Ruby** → clone `vendor/bundle` **and** `.bundle/config`, or set `BUNDLE_PATH`/`BUNDLE_APP_CONFIG`.
- **Never symlink a dependency directory** that the agent may run a package manager against.

### 3.6 A doc-vs-measurement conflict on pnpm

**[conflict]** pnpm's own source carries a doc comment implying its shims *are* relocatable **[source]** (`pnpm/crates/cmd-shim/src/shim/sh.rs`, read in a delegated pass):

> "A shim inside `relocatable_root` names its target marker and every `NODE_PATH` entry inside that root **relative to its own directory, so the tree keeps working after the root moves**. `None`, or any root on Windows, writes the absolute paths `@zkochan/cmd-shim` writes."

**I measured the opposite.** pnpm **11.13.1** on this machine wrote fully **absolute** `NODE_PATH` entries into every `.bin` shim **[measured]** — see the excerpt in finding 1. The reconciliation is presumably that `relocatable_root` was `None` for a plain project install, so the code took the absolute `@zkochan/cmd-shim` path. I could not confirm which, so: **trust the measurement for pnpm 11.13.1 and treat shim relocatability as version- and configuration-dependent.** Any implementation must *verify* the shims after copying rather than assume them relative.

Secondary **[conflict]**, minor: pnpm docs describe `node_modules/.pnpm` files as **hardlinks** into the content-addressable store — "These are the only 'real' files in `node_modules`" **[docs]** (https://pnpm.io/symlinked-node-modules-structure). I measured `links=1` and a **different inode** from the store file **[measured]**, i.e. not hardlinked. That is consistent with pnpm's `packageImportMethod: auto`, documented as "On macOS and Windows it tries clone, then hardlink, then copy" **[docs]** — so on APFS the files are CoW clones. The docs' "hardlink" framing is Linux-accurate and macOS-inaccurate. It does not change the relocation conclusion, but it does mean per-worktree pnpm disk cost on APFS is already near-zero without any help from us.

### 3.7 Method note on `grep -r`

`grep -rl "<abs path>" .venv` returned **0** while `grep -c "<abs path>" .venv/bin/pip` returned **1** on the same file **[measured]**. BSD `grep -r` under-reported here and I did not isolate why. **Every absolute-path count in §3.4 was therefore re-derived with an explicit Python `os.walk` + byte-substring scan**, which is what the table reports. Flagging it because the npm and bundler "0 references" conclusions would have been false negatives from `grep -r` alone — I caught that only because the Python scan confirmed them independently and the venv case exposed the discrepancy. If you reproduce this, do not trust `grep -r` for the audit.

---

## Open questions

- npm hidden-lockfile mtime survival across `clonefile` **[unverified]** — one test away, and it decides whether post-clone `npm install` is instant.
- Conductor's setup-script blocking semantics and failure behaviour **[unverified]** — undocumented.
- bun: everything here is from docs; nothing measured (not installed).
- Native-extension Ruby gems and relocatability of `vendor/bundle` **[unverified]**.
- Whether `reference-transaction` / `post-index-change` fire on `git worktree add` — **not isolable on this machine** (§2.7).
- pnpm `relocatable_root`: under what configuration does pnpm write relative shims? (§3.6)

## Appendix — reproducing the measurements

Scratch labs, all under the session scratchpad (removed with the session):

- `…/scratchpad/wt-lab/` — git hook lab. `main-repo/` with hooks in `.git/hooks/`, worktrees `wt-new`, `wt-existing`, `wt-nc`, `wt-detach`, `wt-sync`, `wt-fail`.
- `…/scratchpad/fs-lab/` — filesystem lab. `src-npm/`, `src-pnpm/`, `src-yarn/`, `py/{src,uvsrc}/.venv`, `rb/src/`, plus `scan.py` (the `os.walk` auditor) and `probe.c` (the raw `clonefile(2)` probe).

Key invocations:

```sh
# hook firing matrix
git worktree add ../wt-new -b feat-new          # fires
git worktree add --no-checkout ../wt-nc -b b    # does not fire
git worktree repair ../moved                     # does not fire

# where git looks for hooks in your environment
git rev-parse --git-path hooks/post-checkout

# clone vs copy, with settling for APFS purgeable space
sync; df -k /private/tmp; cp -Rc node_modules dst; sync; sleep 2; df -k /private/tmp

# absolute-path audit (do NOT use grep -r, see §3.7)
python3 -I scan.py <tree> <source-abs-path>

# raw clonefile probe
cc -o probe probe.c && ./probe src dst
```

pnpm runs used Homebrew node 26.5.0 (`/opt/homebrew/Cellar/node/26.5.0/bin`), since pnpm 11 requires Node ≥ 22.13 and the default `node` on PATH here is v22.5.1.

## Sources

**Git** — `githooks(5)` and `git-worktree(1)` as shipped with git 2.55.0 · https://git-scm.com/docs/git-worktree

**Claude Code** — https://code.claude.com/docs/en/worktrees.md · https://code.claude.com/docs/en/hooks.md · binary at `~/.local/share/claude/versions/2.1.293`

**Parallel-agent tools** — https://github.com/stablyai/orca · https://www.onorca.dev/docs/model/worktrees · https://www.conductor.build/docs/{core/scripts,reference/files-to-copy,configure-your-project,reference/conductor-json} · https://github.com/stravu/crystal · https://github.com/BloopAI/vibe-kanban · https://vibekanban.com/docs/core-features/creating-projects · https://github.com/imbue-ai/sculptor (+ its `docs/help`, `docs/history.md`)

**Worktree managers** — https://github.com/max-sixty/worktrunk · https://worktrunk.dev/{step,hook}/ · https://github.com/coderabbitai/git-worktree-runner · https://github.com/satococoa/wtp · https://github.com/raine/workmux · https://github.com/d-kuro/gwq · https://github.com/k1LoW/git-wt · https://github.com/eetann/wtman · https://github.com/kdcokenny/opencode-worktree · https://github.com/ThePrimeagen/git-worktree.nvim

**Graphite / jj** — https://graphite.com/docs/multiple-worktrees · https://graphite.com/docs/command-reference · https://docs.jj-vcs.dev/latest/{working-copy,cli-reference}/ · https://github.com/jj-vcs/jj/issues/3577

**Package managers** — https://pnpm.io/git-worktrees · https://pnpm.io/global-virtual-store · https://pnpm.io/{settings/node-modules,settings/store,symlinked-node-modules-structure,limitations,faq} · pnpm source `crates/{fs/src/symlink_dir.rs,modules-yaml/src/lib.rs,cmd-shim/src/shim/sh.rs}` · https://docs.npmjs.com/cli/v11/configuring-npm/{package-lock-json,folders,package-json} · https://github.com/npm/bin-links/blob/main/lib/link-bins.js · https://yarnpkg.com/{features/pnp,advanced/pnp-spec,features/caching,configuration/yarnrc} · https://github.com/yarnpkg/berry/blob/master/packages/plugin-pnp/sources/PnpLinker.ts · https://github.com/yarnpkg/yarn/blob/master/src/util/fs.js · https://bun.com/docs/pm/{cli/install,isolated-installs,global-store}

**Python / Ruby / native** — https://docs.python.org/3/library/venv.html · https://docs.python.org/3/library/site.html · CPython `Lib/venv/{__init__.py,scripts/common/activate}` · https://github.com/astral-sh/uv/blob/main/crates/uv-cli/src/lib.rs · https://docs.astral.sh/uv/{reference/cli,concepts/cache}/ · https://python-poetry.org/docs/configuration/ · https://bundler.io/man/bundle-config.1.html · https://github.com/rubygems/rubygems/blob/master/lib/bundler/settings.rb · https://github.com/nodejs/node-gyp/blob/main/addon.gypi · https://sharp.pixelplumbing.com/install/

**macOS / GNU** — `man cp`, `man 2 clonefile` (macOS 26.5.2) · `man cp` from GNU coreutils 9.11
