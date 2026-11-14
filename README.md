# orca-term

Orchestrate a fleet of parallel CLI coding agents from the terminal, with Neovim as the editor.

Inspired by [Orca](https://github.com/stablyai/orca), but without the Electron app: each agent
runs in its own git worktree, and review happens in the tools already living in the terminal.

> **Not affiliated with Orca or Stably.** orca-term is an independent open-source project. It is
> not made, endorsed or supported by Stably or by the Orca team, and it shares no code with Orca.
> "Orca" is used here only to credit the project that inspired it.

## Status

Early. The route was charted as a [wayfinder](https://aihero.dev/skills-wayfinder) map in this
repo's issues (the issue labelled `wayfinder:map`) and synthesised into a spec. It is now being
built in small slices, each a GitHub issue under that spec.

Today: `orca-term worktree new|ls|setup|rm|prune` manage the worktrees of the current repository
(`worktree detail <worktree>` tells all about one), and `orca-term agent start|attach|stop
<worktree>` run the worktree's agent (claude or opencode) in one of them, inside a sandbox, held by
a detached process that any terminal can attach to. `orca-term note
add|ls|edit|rm|send <worktree>` attach notes to a worktree's lines or files and paste them into its
agent as one prompt, in Orca's format. `orca-term turn ls|diff|blame <worktree>` list a worktree's
turns, show what one turn changed and say which turn wrote each line of a file. `orca-term doctor`
checks what orca-term needs on the machine. `orca-term` with no arguments, or `orca-term app`,
opens the terminal application: every project's worktrees in a side menu, and the open
worktree's agent, Neovim and shell beside it ([Application](#application)).
The primary checkout, the repository you work in, is a worktree of its fleet too, as in Orca (ADR
0013): `worktree ls` lists it first, tagged `repo (primary)` (`"primary": true` in `--json`, `false`
for the others), and an agent can run there, with its turns, notes and unseen mark like any
worktree's. It is never removed: `worktree rm` refuses it (`primary_checkout`, exit code 24).
`orca-term worktree rm <worktree>` keeps the worktree's branch unless it is merged, and refuses
modified or untracked files unless `--force`; an agent running there is stopped first (`stopped
the agent of worktree <name>`), once those checks pass, so a refused removal leaves it running.
A branch also counts as merged when its pull request, as `pr refresh`, `pr create` or `pr merge`
last recorded it, is merged on GitHub with exactly the branch's tip as its head, as after a squash
or rebase merge: the removal then says `deleted branch <name> (merged as PR #12)`. It reads only
that record and never asks GitHub, so after merging on the web run `pr refresh` first. A branch
with a commit after the merged head is kept, and so is one whose record predates the head.
A merged branch is deleted only at the tip that was checked: if the agent commits while it is
being stopped, the branch is kept with that commit, saying `kept branch <name>: it moved while the
agent stopped`. `--force` still deletes the branch whatever its tip.

## Install

orca-term runs on macOS and Linux. It is one binary, `orca-term`; it ships no Neovim plugin
(ADR 0014), and your own Neovim, whatever its config, is the editor beside each worktree.

From a release: download the asset for your OS and architecture from the
[releases](https://github.com/marivaldo/orca-term/releases) (the targets `aarch64-apple-darwin`,
`x86_64-apple-darwin`, `x86_64-unknown-linux-musl` and `aarch64-unknown-linux-musl`), check it
against the `.sha256` beside it, and put the binary on your `PATH`:

```fish
set version 0.1.0
set target aarch64-apple-darwin
set asset orca-term-$version-$target.tar.gz
curl -fsSLO https://github.com/marivaldo/orca-term/releases/download/v$version/$asset
curl -fsSLO https://github.com/marivaldo/orca-term/releases/download/v$version/$asset.sha256
shasum -a 256 --check $asset.sha256
tar -xzf $asset orca-term
mkdir -p ~/.local/bin
mv orca-term ~/.local/bin/
```

From the source, with [Rust](https://rustup.rs): `cargo install --path .` in a clone of this
repository puts `orca-term` in `~/.cargo/bin`.

Then `orca-term doctor` says whether the rest is there: git, the agents and their sandbox
([Doctor](#doctor)). Neovim is needed only for its tab in the application, 0.11 or later.

## Start

Run `orca-term` with no arguments in a repository, from a terminal, and it opens the terminal
application there, as `orca-term app` does (see [Application](#application)). Like
`orca-term app`, it needs git, and refuses with the error `prerequisite` (exit code 9) without
it.

`orca-term` with no arguments prints the usage instead, as before, when it runs inside Neovim
(`$NVIM` is set, as in a Neovim terminal) or when its stdin or stdout is not a terminal (a pipe,
a script).

As it opens, the application adds the repository you are in to your
[project list](#projects), by its primary checkout, as `orca-term project add` does, and says
nothing, whether you opened it with no arguments or with `orca-term app`. A repository already on
the list is left as it is, and outside a repository nothing is added. A project list that cannot
be changed (a broken `projects.json`, one written by a newer orca-term, another command still
changing it after 2 seconds) never stops the application from opening: nothing is added then, and
the application shows the problem. Nothing is added either when the application does not open:
the usage, `orca-term app` without a terminal, git missing.

`orca-term open`, which opened your own Neovim with the plugin's sidebar, is gone, and
`orca-term` with no arguments no longer opens Neovim: the application runs your Neovim beside
each worktree instead.

## Application

`orca-term app`, or `orca-term` with no arguments, opens the terminal application in the
terminal you run it in (ADR 0012). It is being built in slices; it shows the side menu, each
worktree's agent, Neovim and shell, and the open worktree's changed files and pull request.

- **Side menu**, on the left at 100 columns and more: every project on your
  [project list](#projects), each with its worktrees, its primary checkout first with the
  ` primary ` tag after its name. A worktree shows its state's glyph (`◐`
  working, `?` waiting for you, `●` finished, `✗` failed, `○` idle, `·` no agent), its name and
  its unseen mark `•`; then its branch and its pull request's number, as `pr refresh` last found
  it; then its agent, what the agent said last and how long ago. Under 40 rows each worktree takes
  two lines. A project with no worktrees says `no worktrees yet`, and one whose directory is gone
  shows why, muted. The side menu reads every project again each second, so it follows what
  happens anywhere else.
- **Centre**: the open worktree's panels, under the tabs `1 agent  2 nvim  3 shell`. The
  agent's terminal is attached to its holder as
  `orca-term agent attach` is: its screen is replayed, every key goes to it, and it takes the
  panel's size. A worktree with no agent running says so, and `enter` starts it, as
  `orca-term agent start` does (a start that needs your approval fails, saying why: run
  `orca-term agent start` in a shell then). The application opens the worktree you start it in
  (the deepest, when one sits inside the primary checkout), else the first one.
- **Neovim and shell**: each worktree has its own Neovim (`nvim` on your PATH with your own
  config, or the program `ORCA_TERM_NVIM` names) and its own shell (`$SHELL`), both started in
  the worktree the first time their tab shows, and kept while the application runs. Neovim
  listens on a socket of its own, under `$XDG_RUNTIME_DIR/orca-term/` or `/tmp/orca-term-<uid>/`,
  and is registered as `orca-term nvim register` does, so the core can ask it to `:checktime`
  and to open a diff.
  Quitting the application ends its Neovims (unsaved buffers are kept in Neovim's swap files) and
  its shells; agents keep running in their holders.
- **Right column**, on the right at 150 columns and more: `changes` with how many files changed
  against the default branch and the lines added and deleted, each file with its letter (`M`, `A`,
  `D`, `R`), name, directory and counts, and a bar of the lines added against deleted; `pull
  request` with its number, title, state, how many checks fail and run and each check, as
  `pr refresh` last found it (`○ none yet` and `^a P open a draft` without one); `branch` with how far it is ahead of and
  behind the local default branch. `^a f` hides or shows it. Under 150 columns the centre's tab row
  shows the summary `4 files +55 −21` instead, and `^a f` lays the right column over the centre.
  With the focus on it, `j` and `k` move over the files and `enter` opens the file's diff in the
  worktree's Neovim and shows Neovim (zoomed, from the overlay): see [Diff](#diff). A Neovim you
  run yourself in the worktree, in a window of its own and registered, opens the diff too; the
  application cannot raise that window. The changes are read again when the
  worktree's agent works and every 5 s.
- **`:checktime`**: when an agent ends a turn, every Neovim registered in the repository is asked
  to `:checktime`, so the buffers of the files it wrote show what it wrote, with or without the
  application.
- **Split and zoom**: `^a |` shows the agent and Neovim (or the shell) side by side, each with its
  own tab row; `^a z` fills the screen with the focused panel and puts everything back as it was,
  the split included.
- **Picker**: `^a w` floats every worktree over the screen; type to filter by name, branch or
  project, move with the arrows (or `Ctrl-j`/`Ctrl-k`), `enter` opens, `esc` closes. Under 100
  columns, where the side menu does not show, it is how you reach the other worktrees.
- **A failed agent**: when a worktree's agent ends with an error you did not ask for (`agent stop`
  is not one), its panel shows the end of its last output and, under it, a card:
  `✗ agent stopped · exit 1 after 41m · 12m ago`, that your changes are kept, and `r` resume the
  session, `R` restart clean on a new session, `l` show the log (the agent's last output as its
  holder kept it, which `j`/`k`, `ctrl-u`/`ctrl-d` and `g`/`G` scroll and `esc` closes), `x`
  dismiss. The holder records the end in `agent-end.json` and the output in `agent.log`, in the
  worktree's admin directory, so the card shows whenever you open the worktree.
- **New worktrees and a filter**: in the focused side menu, `n` asks a name and creates the
  worktree in the project under the cursor, as `orca-term worktree new` does (its setup's output
  goes to its log; a committed setup script that needs your approval does not run), then opens
  it; `/` filters the worktrees as you type (`enter` keeps the filter, `esc` clears it).
- **Draft pull request**: `^a P` opens a draft pull request for the open worktree, as
  `orca-term pr create --draft` does, titled and described as `orca-term pr compose` says; what
  went wrong (the primary checkout on the default branch, no commits) shows in the status bar.
- **Help**: `^a ?` lists the keys; any key closes it.
- **Status bar**: the open worktree, how many worktrees need you, failed, are running or are
  quiet, and the keys.

Every key goes to the part with the focus, a panel, the side menu or the right column (or the
picker or help while one floats). `Ctrl-a` arms
the application's own keys for the next key (the status bar lights up for about 1.5 s; `esc`
cancels):

| Key | What it does |
|---|---|
| `^a h`, `^a l` | move the focus: the side menu, the centre, between the split's halves, and the right column (zoomed, between the split's halves only, staying zoomed) |
| `^a j`, `^a k` | open the next, or the previous, worktree |
| `^a 1`, `^a 2`, `^a 3` | show the agent, Neovim or the shell |
| `^a \|` | split the agent and Neovim (or the shell) side by side, or back |
| `^a z` | zoom the focused panel, or restore |
| `^a f` | hide or show the right column; under 150 columns, lay it over the centre or close it |
| `^a w` | the picker |
| `^a P` | open a draft pull request |
| `^a ?` | help |
| `^a a` | send a literal `Ctrl-a` to the focused panel |
| `^a q` | quit; every agent keeps running, the Neovims and shells end |
| `j`, `k`, arrows, `enter` | in the focused side menu: move, and open the worktree |
| `n`, `/` | in the focused side menu: a new worktree, and the filter |
| `j`, `k`, arrows, `enter`, `esc` | in the focused right column: move, open the file's diff in Neovim, and leave |
| `enter` | in a panel whose agent, Neovim or shell is not running: start it |
| `r`, `R`, `l`, `x` | on a failed agent's card: resume, restart clean, the log, dismiss |

The application restores your terminal when it quits, when it is ended by a signal (`SIGTERM`,
`SIGHUP` when its terminal closes) and on a crash. Colours follow `COLORTERM` (`truecolor` or
`24bit` for 24-bit colour, else 256 colours), and `NO_COLOR` draws without colour: where the keys
go reversed and bold, titles bold and underlined, the open worktree between two rules, secondary
text dim. The 16 colours a panel's program uses are drawn with the theme's (red failed, green done,
yellow waiting, blue and cyan working, magenta the accent).

An agent attached from several places at once (the application, and an `agent attach` elsewhere)
takes the size of the one you typed in last. A terminal attaching late also gets the agent's
modes (its alternate screen, mouse, bracketed paste, keyboard protocol) put back before its
screen. Keys reach the agent in the classic encoding, so Shift+Enter arrives as Enter, and the
mouse stays with your terminal.

## Setup

`worktree new` runs the setup script `scripts.setup` from `orca-term.yaml` in the new worktree, and
`worktree setup <worktree>` runs it again. A script from a committed `orca-term.yaml` was written
by whoever committed it, so it runs only once you have approved its exact text in this repository.
At a terminal, orca-term prints the script and the file it comes from, then asks
`run this setup script? [y/N]`; `y` approves it and runs it, anything else leaves the worktree
*not ready* until you run `orca-term worktree setup <worktree>` again. Without a terminal, or with
`--json`, nothing is asked: the command fails with the error `untrusted` (exit code 8) and the
worktree stays not ready. `--approve-setup` approves the script without asking. An approval is the sha256
of the script, kept in `.git/orca-term-trust.json`, outside every worktree and out of the agent's
reach; any change to the script, in the primary checkout or on a branch, asks again. A script from
`.git/orca-term.yaml` or from your global config file is yours and always runs.

The same goes for `sandbox.writable`, the extra paths claude's sandbox lets the agent write to: a
committed list widens the sandbox only once you have approved it. `agent start` prints the paths
and their file, then asks `widen the sandbox with these writable paths? [y/N]`; without a terminal
it fails with `untrusted` and starts nothing. `agent start --approve-sandbox` approves the list
without asking. A changed list asks
again; a list from `.git/orca-term.yaml` or the global config file needs no approval.

## Signing

A commit the agent makes is signed as yours are when git can reach your key from inside the
sandbox. A key in a file signs inside both sandboxes, since they let the agent read everything. A
key only an agent holds (ssh-agent, 1Password, Secretive, a passphrase-protected or hardware key,
gpg-agent) does not: both sandboxes keep the agent away from unix sockets, your signing agent's
among them. To let the agents of a repository's worktrees sign with it, turn on
`sandbox.agent_socket` in `.git/orca-term.yaml`, or in your global config file for every
repository:

```yaml
sandbox:
  agent_socket: true
```

`agent start` then lets the agent connect to exactly the socket `SSH_AUTH_SOCK` names when it
starts, by its real path, and, when git signs with openpgp (`gpg.format` is `openpgp` or unset),
to gpg-agent's socket as `gpgconf --list-dirs agent-socket` names it: claude's settings list it in
`allowUnixSockets`, and opencode's macOS profile allows that one path. The agent can then sign,
and with ssh-agent authenticate, with your agent's keys without ever reading them. Nothing else
changes: the holders' and Neovim's sockets stay out of its reach. When `SSH_AUTH_SOCK` is unset or
names no socket, nothing changes either. It is off by default. A committed `orca-term.yaml` that
sets it is refused: `agent start` starts nothing and says why, and no approval lets it through,
since it would hand your agent's keys to whoever wrote the repository.

With the setting on, `agent start` says what it allowed, one line per kind of socket, and never
the socket's path:

```
signing: the sandbox reaches your ssh agent
signing: the sandbox reaches gpg-agent
```

When it found none, it still starts the agent and warns, so you know before a signed commit
fails (the gpgconf part appears only when git signs with openpgp):

```
signing: sandbox.agent_socket is on, but no signing agent socket was found (SSH_AUTH_SOCK is unset or not a socket; gpgconf names no gpg-agent socket)
```

`agent start --json` carries the same as `signing_sockets`: a list of `ssh-agent` and
`gpg-agent`, possibly empty, with the setting on, and `null` with it off or when the agent was
already running. A socket inside the repository's git directory, where orca-term keeps its own,
is never allowed, even when `SSH_AUTH_SOCK` names it.

`orca-term doctor` says when git signs with an ssh key whose private half is not a readable file
(or with a key written in git's config), so that only an agent can sign with it, and names the
setting.

Limits:

- With the setting on, every command the agent runs can use your agent's keys, to sign and to
  authenticate, for as long as the agent runs.
- On Linux, opencode's sandbox reaches pathname sockets already, so opencode signs through your
  agent with or without the setting. claude's sandbox there allows a unix socket only through
  `allowAllUnixSockets`, which opens every socket, so orca-term never turns it on and claude
  cannot sign through an agent on Linux.
- On macOS (measured with claude 2.1.318), claude's `git push` over SSH works inside its sandbox
  once ssh can reach your key, but a push over HTTPS cannot read its credentials from the
  Keychain, and `gh` cannot verify GitHub's TLS certificate (`x509: OSStatus -26276`), so
  `gh pr create` fails too. Listing `gh *` in claude's own `sandbox.excludedCommands` does not
  change that. Open the pull request from your own terminal.

## Claude and Neovim

claude can see what you do in the worktree's Neovim through Claude Code's own editor protocol:
the file you are in and the lines you have selected go along with each prompt, and
`:ClaudeCodeSend` sends a selection as an at-mention. orca-term starts claude with `--ide`, which
links it, as it starts, to the one editor whose lock file under `~/.claude/ide/` is valid for the
worktree. Neovim serves that protocol through
[coder/claudecode.nvim](https://github.com/coder/claudecode.nvim), which you install in your own
Neovim; orca-term ships no part of it. With lazy.nvim:

```lua
{
  'coder/claudecode.nvim',
  lazy = false,
  opts = { terminal = { provider = 'none' } },
}
```

The terminal provider `none` keeps the plugin from opening a claude of its own: orca-term starts
the worktree's claude. `lazy = false` loads it when Neovim starts, so its lock file is there when
claude starts; loaded only on a key or a command, it writes none until then. Open that Neovim in
the worktree (`cd` into it first): the lock file names Neovim's working directory, and claude
links only to an editor open on its own worktree. A Neovim started after claude is not picked up
by `--ide`: type `/ide` in claude to link it. In the [application](#application), the worktree's
Neovim already runs in the worktree, but it starts the first time its tab shows, often after the
agent: show it (`^a 2`), then type `/ide` in the agent. Without claudecode.nvim, claude starts as
before.

`orca-term doctor` asks the Neovims registered with orca-term in the repository (the application
registers each Neovim it starts; one of your own registers with `orca-term nvim register`, see
[Diff](#diff)) whether they have claudecode.nvim loaded: `ok` when they have, a `warn` line when
one has not, and an `info` line saying it was not checked when none is registered or none
answered. It never fails because of it.

Limits:

- Every launch runs under `acceptEdits`, so edits in the worktree are not asked about, and the
  protocol's proposed diffs in Neovim mostly do not open; the file and the selection do go along.
- Two Neovims open on the same worktree are two valid editors: `--ide` links to neither, and
  `/ide` in claude lets you pick one.
- An orca-term started from an editor's own terminal does not pass that editor's port
  (`CLAUDE_CODE_SSE_PORT`, `ENABLE_IDE_INTEGRATION`) on to the agent, so claude never links to
  that editor instead of the worktree's Neovim.
- opencode is not linked: it has no such protocol for Neovim (`opencode.nvim` goes through
  opencode's own server, left for later).
- CI never runs a real claude, so the link itself is checked by hand, before each release, with
  the steps below; the doctor's question is tested against real Neovims.

To check the link by hand, in a repository with orca-term, with claudecode.nvim installed as above
(each block pasted into fish):

```fish
orca-term worktree new ide-check
```

In a second terminal, open your Neovim in that worktree:

```fish
cd (orca-term worktree ls --json | jq -r '.worktrees[] | select(.name == "ide-check") | .path')
nvim README.md
```

`:lua print(package.loaded.claudecode ~= nil)` there prints `true`, and
`grep -l ide-check ~/.claude/ide/*.lock` in the first terminal names one lock file. Then, in the
first terminal, start claude after that Neovim, so `--ide` links them:

```fish
orca-term agent start ide-check
orca-term agent attach ide-check
```

Type `/ide`: it shows Neovim connected. Select a few lines in Neovim (`V`, then `j`), then ask
claude `which lines do I have selected in the editor? quote them`: it quotes them. Detach with
`Ctrl-\` and clean up:

```fish
orca-term agent stop ide-check
orca-term worktree rm ide-check --force
```

## opencode

A worktree can run [opencode](https://opencode.ai) instead of claude:
`orca-term agent start <worktree> --agent opencode`. The worktree records its agent, so the next
`agent start` starts opencode again, resuming its session with
`opencode -s <id>`; a worktree that records no agent gets claude-code. `orca-term worktree ls`
shows `opencode (running)`. Asking for another agent while one runs is refused as `busy` (exit
code 3), naming the agent that runs.

`orca-term worktree new <name> --agent claude-code|opencode` creates the worktree and, once its
setup succeeded, starts that agent, as `agent start --agent` would. A failed setup leaves the
worktree *not ready* and starts nothing, but the agent stays recorded: once `orca-term worktree
setup` succeeds (it starts nothing itself), `agent start` starts the agent
chosen at creation. When the start is refused (no sandbox,
an unapproved `sandbox.writable` list, a missing or old agent), the worktree is still there and
ready, with the agent recorded, and the command exits with the refusal's error.

`orca-term agent swap <worktree> <agent>` swaps a worktree's agent: it stops the one running and
starts the other on the same worktree, which records it. It is refused as `busy` while the agent is
in a turn, working or waiting for you; stop it with `agent stop` first if you mean to. The turns,
the notes and the snapshots stay with the worktree. A session belongs to its agent: the new agent
resumes the session it recorded itself, if any, and starts a new one otherwise.

opencode runs inside a sandbox orca-term generates for each launch, `sandbox-exec` on macOS and
bubblewrap (`bwrap`) on Linux, with everything allowed inside it, as claude is. It reads anything
and reaches the network, and writes only to the worktree, the git common dir, its own directories
(`~/.local/share/opencode`, `~/.cache/opencode`, `~/.config/opencode` and `~/.local/state/opencode`,
or the same under your `XDG_*` directories, and `~/.opencode`), the temporary directories and the
approved `sandbox.writable` paths. orca-term creates opencode's own directories before the first
launch, since the sandbox would keep opencode from creating the directories above them. orca-term's
state stays out of its reach (every worktree's `orca-term` directory, `.git/orca-term.yaml`,
`.git/orca-term-trust.json`, `refs/orca-term`), and so do the other agents: on macOS it may connect
to no unix socket but the system's name resolver (and your signing agent's, when you turn on
`sandbox.agent_socket`, see Signing), and on Linux it runs in a user namespace of its
own, whose clients the holders refuse. Without the sandbox tool, `agent start` refuses with
`no_sandbox` (exit code 5); opencode must be 1.18.0 or later (exit code 9 otherwise). orca-term
launches it with `OPENCODE_DISABLE_AUTOUPDATE=1`: an update could only fail inside the sandbox,
and the dialog opencode opens to offer one would take the first keys you type.

The status comes from a plugin orca-term ships: one small JavaScript file, written before each
launch into the worktree's own opencode config directory (`orca-term/opencode` in its admin
directory, which `OPENCODE_CONFIG_DIR` names); nothing goes into opencode's global config. It
reports the message you send (a turn starts), the tool opencode runs, a permission or a question
(waiting), your answer, the session going idle (the turn finishes, with opencode's last words) and
an error (the turn fails), with the session's id. Esc reads as interrupted, whether opencode was
answering or waiting to retry an unreachable provider; while it retries, the turn stays working. It
writes them to a pipe
the holder hands opencode, and the holder, outside the sandbox, records them as claude's hooks are
recorded: the states, the turn log and the snapshots are the same for both agents.

Limits:

- The plugin follows opencode 1.18.0's plugin API; a newer opencode may report its events
  differently.
- opencode's config directory is writable, since opencode writes there, so an agent could add a
  plugin that runs unsandboxed the next time you run opencode outside orca-term, as it could add a
  git hook in `.git/hooks`.
- A command opencode runs may inherit the events pipe and write to it: it can then change what
  orca-term shows for that worktree's turns, and nothing else.
- A subagent session that opencode continues, rather than creates, reports its first message as
  yours, which interrupts the turn; a message opencode adds on its own (a background subagent's
  result) starts no turn.
- A recorded session opencode no longer has makes it exit at once with its own message on a cold
  start (`agent start`); start a new one with `orca-term agent start <worktree> --fresh`.
  `session resume` asks opencode first (see Sessions).
- On macOS, signing commits and pushing over SSH with a key only an agent holds (ssh-agent,
  gpg-agent) fail inside the sandbox unless `sandbox.agent_socket` is on (see Signing); a key in a
  file signs either way. On Linux, the sandbox covers the admin
  directories that exist at launch, and a missing `.git/orca-term.yaml` is covered by an empty file,
  which stays.
- Your own `OPENCODE_CONFIG_DIR` is replaced by orca-term's for opencode in a worktree.

## Sessions

A session is an agent's own record of your prompts and its answers, under its id for it. A
worktree has one current session, which its agent resumes when it starts again, and keeps every
session it held:
`orca-term session ls <worktree>` lists them, the most recently active first, the current one
marked:

```
   SESSION                               AGENT        TURNS  LAST ACTIVITY  FIRST PROMPT
*  5d1c2e9f-8a7b-4c3d-9e2f-1a0b9c8d7e6f  claude-code  1      just now       write the docs
   ses_2Ab9                              opencode     3      2 h ago        add the tests
   0b983a87-4074-466e-b864-422ce6f063c8  claude-code  2      1 day ago      fix the login
```

`orca-term session resume <worktree> <id>` makes that session current and restarts the agent on
it (`claude --resume <id>`, `opencode -s <id>`), starting it if it was not running. A session of
the other agent swaps the worktree's agent, as `agent swap` does. Both are refused as `busy` (exit
code 3) while the agent is in a turn. A claude session whose transcript claude has since deleted,
or an opencode session opencode no longer lists (`opencode session list`, which only reads), is
refused as `session_gone` (exit code 4), with the reason, and nothing starts. Resuming the current
session while its agent runs only says so.

`orca-term session new <worktree>` discards the current session: the agent's next start begins a
new one, and a running agent is restarted on a new session at once. The discarded session keeps
its turns, and `session ls` still lists it, for `session resume` to bring back. Each command takes
`--json`.

A worktree moved with `git worktree move` keeps its sessions, as it keeps its turns and notes:
the next `agent start` resumes the current one from the new path (measured with claude 2.1.318).

Limits:

- A session is known from the turns it holds and from being current: one you leave before its
  first prompt is forgotten once another session is current.
- A claude session from before orca-term recorded each session's transcript (in the turn log) can
  be resumed only while it is the current one; any other refuses as `session_gone`, no transcript
  recorded.

## Diff

`enter` on a changed file in the application's right column opens its diff in the worktree's
Neovim: a new tab with the file's version at the merge base on the left and the file on disk on the
right, both in Neovim's own diff mode (`:diffthis`). The merge base is the commit the worktree forked
from (`git merge-base <default branch> HEAD`), the same the right column counts the changes
against, so its commits and its uncommitted changes show together. A file new in the worktree has
an empty left side. The left side is a scratch buffer named `orca-term://<base>/<file>`, never
written anywhere; the right side is the file itself, which you can edit. Asking again for the same
file replaces its earlier tab; close one with `:tabclose`.

It needs no plugin. The core runs git, then sends every Neovim registered in the repository one
msgpack-RPC request, `nvim_exec_lua` with a little Lua of its own and the file's content at the
base; a Neovim with a tab whose directory is the worktree opens the diff, any other does nothing.
The command behind it is `orca-term nvim diff <worktree> <file>`. A Neovim you start yourself
takes part once it is registered:

```fish
nvim --listen /tmp/my-nvim.sock
# in another terminal, in the repository:
orca-term nvim register /tmp/my-nvim.sock
orca-term nvim diff <worktree> src/main.rs
```

`orca-term nvim unregister <address>` takes it off; a Neovim that is gone is dropped from the
registry the next time the core sends it something.

`orca-term worktree review <worktree>` prints the commit a review compares against and clears the
worktree's unseen mark.

## Notes

A note is a remark for the worktree's agent on a line, a range of lines or a whole file. Notes wait
until you send them; sending types them into the agent as one prompt, in Orca's format (`File:`,
then `Line:`, `Lines:` or `Scope: file`, then `User comment:`), and consumes them. Until the
application has notes of its own, they are added from the command line:

```fish
orca-term note add <worktree> src/main.rs --line 12 --body 'rename this'
orca-term note add <worktree> src/main.rs --lines 3-5 --body 'one function'
orca-term note add <worktree> README.md --body 'explain the install'
orca-term note ls <worktree>
orca-term note send <worktree>
```

A note follows its line in the file on disk: when the agent adds lines above it, it moves down
(`note ls` says `(now line 15)`); when its line is changed or deleted, it becomes a note on the
whole file. `note edit <worktree> <id> --body <text>` changes one, `note rm <worktree> <id>`
deletes it, and `note send <worktree> <id>...` sends only those. Sending needs the worktree's agent
running (`no_holder`, exit code 7, otherwise); the notes stay pending until then.

## Turns

Every turn of a worktree's agent is bounded by two snapshots of the worktree's files, taken by the
hooks when the turn starts and when it ends: its tracked and untracked files, `.gitignore`
respected, as a commit outside every branch. A worktree's snapshots form one chain, each the parent
of the next, under `refs/orca-term/worktrees/<id>` (`<id>` is the worktree's directory in
`.git/worktrees/`): see them with `git log refs/orca-term/worktrees/<id>`. They are not pushed
unless you push that ref yourself. What you edit between two turns is a snapshot of its own, the
next turn's begin, and belongs to no turn.

`orca-term turn ls <worktree>` lists the turns, oldest first, the open one last:

```
TURN  OUTCOME      AGENT   BEGIN    END      PROMPT
1     finished     claude  1a2b3c4  5d6e7f8  add a readme
2     interrupted  claude  5d6e7f8  0f1e2d3  fix the test
3     working      claude  0f1e2d3  -        rename the module
```

`orca-term turn diff <worktree> 1` prints what turn 1 changed: `git diff` from its begin snapshot
to its end snapshot, as a patch. A turn from before orca-term took snapshots has none, and an open
turn has no diff yet.

`orca-term turn blame <worktree> src/main.rs` says which turn wrote each line of the file as it is
on disk now: the turn's number, `-` for a line written between turns (or there before the first
turn), `·` for a line changed since the latest snapshot (during an open turn, those lines will be
the turn's once it ends):

```
   -     1  # demo
   2     2  fn main() {
   -     3      // added by hand between turns
   ·     4  }
```

Each command takes `--json`. A turn whose end the agent never reported (you pressed Esc and sent
another prompt, or the session ended) is *interrupted*, its end *not observed*: it ends on the
snapshot taken by whatever told orca-term it was over, so its diff may include edits made after the
interruption, and `turn diff` says so.

Limits:

- A snapshot stores every untracked file `.gitignore` does not exclude in the repository's objects,
  even one you never meant to commit; it stays local unless you push the chain's ref by hand.
- An edit made during a turn belongs to it, whoever made it: nothing tells your saves apart from
  the agent's while a turn is open.
- Each hook that starts or ends a turn runs git to take its snapshot, which adds a little time on
  a large tree.
- Once `git gc` has packed the chain's ref into `.git/packed-refs`, the agent's sandbox no longer
  keeps it from rewriting the chain.
- If orca-term fails to record a snapshot after taking it, the next snapshot starts the chain again,
  and blame gives the lines of the older turns to no turn.

## Pull requests

`orca-term pr refresh [<worktree>...]` asks the GitHub CLI about the pull request of each
worktree's branch (every worktree when you name none) and its checks, and keeps the answer for
`worktree ls` and the application. It prints one line per worktree:

```text
repo: no PR
demo: PR #12 open, checks pending
docs: no PR
spike: PR unavailable: GitHub CLI is unavailable
```

A worktree on the repository's default branch, the primary checkout most of the time, has no pull
request, and gh is not asked about it; on any other branch the primary checkout has one like any
worktree.

A pull request is `open`, `draft`, `merged` or `closed`, and its checks are `failing` when any
check failed, errored, was cancelled or timed out, else `pending` while any is queued or running,
else `passing` when any succeeded, else `none`. It runs `gh pr view <branch>` in every worktree at
once and gives each 10 seconds. When gh is not installed, not logged in or the repository is not
on GitHub (`not a GitHub remote`), the worktree's pull request is unavailable, with the reason;
the command still succeeds.

orca-term runs gh only in `pr refresh`, `pr create` and `pr merge`, `git push` only in `pr
create` and `worktree push` ([Push](#push)), and `git fetch` only in `worktree rebase`
([Rebase](#rebase)). No other command touches
the network, `pr compose` included.

`orca-term pr compose <worktree>` prints the message a new pull request of the worktree starts
from, without writing anything: the title, a blank line, then the body. The title is the subject
of the branch's only commit ahead of the default branch, or the branch name when there are none or
several, as `gh pr create --fill` makes it. The body is the repository's pull request template when
it has one (`pull_request_template.md` in `.github/`, the root or `docs/`, in any case), else the
only commit's body, else the list of the commits' subjects (`- First`, `- Second`). With `--json`
it prints `title`, `body`, `base` (the default branch), `branch`, `commits` (how many are ahead of
the base) and `uncommitted` (how many files have changes not committed).

`orca-term pr create <worktree> --title <title> [--body-file <file>] [--base <branch>] [--draft]`
creates the worktree's pull request from the core, outside the agent's sandbox (on macOS claude's
sandbox can run neither gh nor a push over HTTPS). It first refuses, pushing nothing:

- a worktree on the repository's default branch, the primary checkout most of the time, which
  has no pull request (`default_branch`, exit code 25; `pr compose` refuses it too);
- a branch with no commits ahead of the base (`no_commits`, exit code 10);
- a branch that already has an open pull request, which it names (`pr_exists`, exit code 11);
- gh missing or not logged in (`prerequisite`, exit code 9), in the words `pr refresh` uses;
- a `--body-file` that does not exist.

Then it pushes the branch with `git push -u origin <branch>`, never forced (a push that fails is
`push_failed`, exit code 12, with git's own words), and runs `gh pr create --base <base> --head
<branch> --title <title> --body-file <file> [--draft]`. The base is the default branch unless you
pass `--base`, and no body file, or an empty one, means an empty body. git never prompts for
credentials there: a push that needs ones it cannot find fails instead. Last it asks gh about the
pull request as `pr refresh` does and keeps the answer, so `worktree ls` shows it at once:

```text
demo: created PR #12 (draft): https://github.com/o/r/pull/12
1 file with uncommitted changes was left out of the pull request
```

Files with uncommitted changes do not stop it: the second line says how many were left out. With
`--json` it prints the worktree's `name` and `path`, its `pr` as `pr refresh` shows it, `pushed`
(`true`) and `uncommitted`.

`orca-term pr merge <worktree> [--squash|--merge|--rebase]` merges the worktree's open pull request
on GitHub, squashing its commits unless you pass `--merge` (a merge commit) or `--rebase`. It first
asks gh about the pull request and refuses, merging nothing:

- no open pull request: none, or one already merged or closed, and always on the default branch
  (`no_open_pr`, exit code 13);
- a draft, which you mark ready for review first (`pr_draft`, exit code 14);
- a pull request GitHub reports as conflicting with its base, whose branch you update first
  (`pr_conflicting`, exit code 15);
- a review that requested changes or still needs an approval (`not_approved`, exit code 16);
- gh missing or not logged in (`prerequisite`, exit code 9), in the words `pr create` uses.

Then it runs `gh pr merge <number> --squash` (or `--merge`, `--rebase`). Anything else GitHub
refuses comes back as gh's own first line. It never passes `--delete-branch`: the worktree and its
local branch stay as they are, as in Orca, and you remove the worktree when you are done with it.
Last it asks gh again and keeps the answer as `pr refresh` would, so `worktree ls` shows it merged
at once:

```text
demo: merged PR #12 (squash): https://github.com/o/r/pull/12
```

A merge queue, auto-merge or a required check can make gh accept the merge while the pull request
stays open. orca-term then never says it is merged: it keeps it open and prints `demo: gh accepted
the merge of PR #12 (squash), but GitHub still shows it open: a merge queue or a required check
may be holding it; run orca-term pr refresh demo later`. When gh cannot say at all, the pull
request is recorded merged, since GitHub may only be slow to show it.

Removing the worktree after the merge deletes its branch too, saying `merged as PR #12`, as long as
the branch has no commit after the one GitHub merged.

With `--json` it prints the worktree's `name` and `path`, its `pr` as `pr refresh` shows it, the
`method` (`squash`, `merge` or `rebase`) and `merged` (`false` when GitHub still shows it open).

`worktree ls --json` and `worktree detail --json` show what the last refresh found as each
worktree's `pr`: `null` when it never ran, else `fetched_at` (milliseconds since the Unix epoch)
and either the pull request (`number`, `title`, `state`, `url`, `checks`), `"none": true`, or
`unavailable` and the reason. They also show each worktree's `last_activity`: when its current
session was last active, in milliseconds since the Unix epoch, or `null` when it has no current
session, and `changed_files`: how many files it changed against its merge base with the default
branch, committed or not, untracked files that are not ignored included, or `null` when it cannot
be counted (no shared history, a broken worktree, one still setting up). The text
`worktree detail` adds a line such as `3 files changed` when there are some.

## Rebase

`orca-term worktree rebase <worktree>` rebases the worktree's branch onto the default branch on
`origin`, as Orca's **Rebase from `<base>`** does. The base is always `origin/main` (or whatever
the default branch is), never a local branch. It first refuses, changing nothing:

- an agent in a turn, working or waiting for you (`busy`, exit code 3);
- a rebase, merge, cherry-pick or revert already in progress (`operation_in_progress`, exit code
  17);
- files with changes not committed, untracked ones included, which it names: commit or stash them
  first, since it never stashes them for you (`dirty`, exit code 6);
- a repository with no `origin` remote holding the default branch (`no_remote_base`, exit code
  18).

Then it fetches the base into a private temporary ref, `refs/orca-term/rebase/<id>`, so a fetch
running at the same time cannot move it, and rebases onto it: from the fork point git finds with
`merge-base --fork-point`, else a plain `git rebase`. The temporary ref is always deleted
afterwards. git never prompts for credentials or opens an editor there.

```text
demo: rebased onto origin/main (3 commits replayed)
```

When the branch already holds every commit of the base, it says `demo: already up to date with
origin/main` and does nothing. With `--json` it prints the worktree's `name` and `path`, the
`base`, the `outcome` (`rebased` or `up_to_date`) and how many commits were `replayed`.

A rebase that stops on conflicts stays in progress, as in Orca: the command fails as
`rebase_conflict` (exit code 19), naming the files in conflict (`files` in the `--json` error),
`worktree ls` shows `idle, rebasing` and `worktree detail` shows `rebase in progress (2/5)`. From
there:

- `orca-term agent resolve <worktree>` types one prompt into the worktree's running agent, as
  `note send` does. It lists the files in conflict, tells the agent to resolve them, `git add`
  them and run `git rebase --continue`, and forbids `git rebase --abort`, `reset`, `stash` and
  `push`. It is refused with no rebase in progress (`no_rebase`, exit code 20) and when the agent
  is not running (`no_holder`, exit code 7). With `--json` it prints the `prompt` and the `files`.
- `orca-term worktree rebase <worktree> --abort` runs `git rebase --abort`, which puts the branch
  back where it was. It is refused with no rebase in progress (`no_rebase`, exit code 20).

Fetching the base is the only network use of `worktree rebase`, and no other command fetches.
`worktree ls --json` names the base as `remote_base` (`origin/main`, or `null` when there is none),
from the refs git already has.

## Push

On macOS a worktree's agent cannot push from inside claude's sandbox, so orca-term pushes for it,
from the core, as Orca's **Push** and its separate **Force push with lease** do.
`orca-term worktree push <worktree>` runs `git push -u origin <branch>`, which sets the upstream
the first time and never forces. git never prompts for credentials there.

```text
demo: pushed to origin/demo
```

When `origin` already has the branch as it is, it says `demo: origin/demo is already up to date`.
Files with changes not committed do not stop it: they are left out, and a second line says how
many. After a push it reads the worktree's pull request again, as `pr refresh` does, so
`worktree ls` and the application show the new run's checks. With `--json` it prints the worktree's `name` and `path`, its `branch`,
the `remote` (`origin/demo`), the `outcome` (`pushed` or `up_to_date`), `with_lease`,
`uncommitted` and the `pr`.

When git rejects the push because the branch diverged from `origin/<branch>` (after a
`worktree rebase`, for example), it fails as `push_diverged` (exit code 21), says so, and pushes
nothing. Only `--force-with-lease` replaces the remote branch:
`orca-term worktree push <worktree> --force-with-lease` runs `git push
--force-with-lease=<branch>:<sha> -u origin <branch>`, where `<sha>` is what
`refs/remotes/origin/<branch>` holds, the commit git last saw there. git refuses when someone else
pushed since; that refusal, like any other failure, is `push_failed` (exit code 12), with git's
own words. There is never an automatic force.

It refuses, pushing nothing, a worktree on no branch (detached, or in the middle of a rebase) and
a repository with no `origin` remote. `worktree ls --json` says whether there is one in
`has_origin`.

orca-term never pushes the repository's default branch: `worktree push` of the primary checkout
on `main` fails as `default_branch` (exit code 25), as `pr create` does there, and pushes
nothing, with or without `--force-with-lease`. On any other branch the primary checkout pushes
like any worktree. `worktree rebase` still updates the local default branch onto `origin/main`.

## Projects

As in Orca, orca-term keeps a list of the repositories you work on, its projects (ADR 0011). A
project is a repository's primary checkout, and its name is that directory's basename. The list
keeps the order projects were added in, and lives outside every repository, in
`$XDG_STATE_HOME/orca-term/projects.json` (or `~/.local/state/orca-term/projects.json`):

```json
{"schema":1,"projects":[{"path":"/home/me/src/app"},{"path":"/home/me/src/api"}]}
```

- `orca-term project add [<path>]` adds the repository containing `<path>`, or the current
  directory. From a linked worktree or a subdirectory it adds the primary checkout. Adding a
  project already on the list changes nothing and says `project app at /home/me/src/app is already
  on the list`. A directory outside every git repository is refused (`not_a_repository`, exit code
  22). With `--json` it prints the project's `name` and `path` and whether it was `added`.
  The application (`orca-term` with no arguments, or `orca-term app`) adds the repository of the
  current directory the same way, silently, as it opens (see [Start](#start)), so a repository
  you start orca-term in is listed without `project add`.
- `orca-term project ls [--json]` lists the projects in order. A project whose directory is gone
  stays on the list, marked `gone` (`"gone": true` with `--json`).
- `orca-term project rm <path or name>` takes a project off the list. It never touches the disk,
  the worktrees or the branches. A path inside the project, such as one of its worktrees or `.`,
  names it too. A name several projects share is refused, naming them (`ambiguous_project`, exit
  code 23, the paths in `candidates` in the `--json` error): remove one by its path.
- `orca-term fleets [--json]` reads every project's fleet in one call. With `--json`, each entry of
  `projects` carries the project's `name`, `path` and `gone`, then `fleet`, exactly what
  `worktree ls --json` prints for it without `contract` and `version`, and `error`, as any
  command's error prints it. A gone or broken project has a `null` fleet and its error, and the
  others are still read.

The application's side menu shows every project on the list, in its order, each with its
worktrees ([Application](#application)).

## Doctor

`orca-term doctor` checks everything orca-term needs and says how to fix what is missing. It
installs nothing, and works outside a repository:

```
ok       git 2.55.0
ok       sandbox sandbox-exec
warn     claude 2.1.310: newer than the tested 2.1.306; it may still work
ok       opencode not installed (optional)
info     contract 1, orca-term 0.1.0
```

It needs git 2.38 or later, claude's sandbox tools (`sandbox-exec` on macOS, `bwrap` and `socat`
on Linux) and claude 2.1.295 or later. A claude newer than the one orca-term was tested with is
only a warning. opencode is optional, since you may use only claude; starting it needs 1.18.0 or
later, and one newer than the tested 1.18.0 is only a warning too. Inside a repository, a
`claudecode` line says whether the Neovims registered there have claudecode.nvim loaded, a
warning at worst (see Claude and Neovim). When git signs with an ssh key only an agent can use, an `info` line before the contract
names `sandbox.agent_socket` (see Signing). With `--json` it prints the same checks as one
document. It exits 0 unless something is missing or too old; then it prints the whole report and
exits 9.

Every command that needs a missing or outdated program refuses with the same line, exit 9: git for
every command, claude or opencode for `agent start`, such as
`orca-term: git 2.30.1: orca-term needs 2.38 or later; run orca-term doctor for the full report`.

## Development

Needs Rust 1.97 and [just](https://github.com/casey/just). `just check` runs every check CI runs;
see `docs/harness.md` for the tools it expects and the rules it enforces. `just hooks` installs the
local git hooks (needs [lefthook](https://github.com/evilmartians/lefthook)).

`tests/nvim_diff.rs` opens a diff in a real `nvim --clean --headless`, so it needs Neovim 0.11 or
later on `PATH`; without one it says so and skips (CI's Linux job installs a pinned Neovim and sets
`ORCA_TERM_REQUIRE_NVIM`, which makes it fail instead).

### Live checks

The suites replay recordings of what claude and opencode sent orca-term, so CI needs neither agent.
`just check-live`, which CI never runs, checks that those recordings still hold: it drives the
installed claude and opencode through the real orca-term, records what they send, and runs the
suites that replay the recordings. Run it before each release and whenever an agent's version
floor rises.

```sh
just check-live                  # every scenario
just check-live p-hello tui-esc  # only the scenarios named (these need only claude)
```

It first checks the agents the scenarios need as `orca-term doctor` does, opencode included, and
refuses when one is missing or too old. Each scenario then runs in a throwaway repository under the
system's temporary directory: `worktree new`, `agent start`, and an attach on a pseudo-terminal
that types the scenario's keys and waits for the hooks it names. orca-term writes nothing to the
agents' config, but the agents run with your own login and settings, so claude keeps its
transcripts and remembers the temporary folders it was told to trust, as it always does.

It costs real prompts on your account, each asking for a one-word answer or a few seconds of
output. A full run is 11 prompts (8 of them claude's) and takes a few minutes:

| Scenario | Agent | What the recording shows | Prompts |
|---|---|---|---|
| `p-hello` | claude | a plain prompt finishes | 1 |
| `tui-esc` | claude | Esc mid-turn interrupts it; the next prompt finishes | 2 |
| `tui-queuetool` | claude | a prompt queued while a command runs folds into that turn | 2 |
| `tui-perm` | claude | a permission ask waits; refused with Esc, then allowed | 2 |
| `tui-sandbox` | claude | the sandbox stops a write into the worktree's admin directory, and the file is checked missing | 1 |
| `opencode-hello` | opencode | a plain prompt finishes | 1 |
| `opencode-esc` | opencode | Esc twice mid-turn interrupts it | 1 |
| `opencode-deny` | opencode | a deny rule in the worktree's `opencode.json` refuses a command: the turn finishes with one denial, and the file is checked missing | 1 |
| `tui-ask` | claude | a question waits for an answer; recorded by hand, `just record-live tui-ask` | 2 |

Each recording is written in place, named after the agent's version:
`tests/fixtures/claude-hooks/<recording>-<version>.log` for claude,
`tests/fixtures/opencode-events/<recording>-<version>.log` for opencode, its local paths replaced by
`{{worktree}}`, `{{transcripts}}` and `{{denied}}`. A new version lands beside the older ones, so
`git diff --no-index` between the two files shows what the agent now sends differently, and
recording the same version again shows in `git diff`. The summary lists each scenario as recorded
or failed (a failed one writes nothing and keeps its directory, with the attach's screen in
`screen.log`), each suite as passed or diverged with its failing tests, and `git status --short` of
the fixtures; it exits 1 when a suite diverged or a scenario failed. A newer claude makes the
`doctor` suite diverge until `CLAUDE_TESTED` in `src/domain/prerequisite.rs` names it, and a newer
opencode until `OPENCODE_TESTED` does: raise it and commit it with the recordings.

A scenario is a small file in `tests/live/`: the agent, the recording's name, a timeout, the
outcome of each turn the recording must replay to, then the keys to type and the hooks to wait for
(`xtask/src/live/scenario.rs` documents the format). It can also write files into the worktree
before the agent starts, such as the `opencode.json` holding the deny rule of `opencode-deny`, and
say how many denials each turn must count. One that cannot be typed reliably, such as
picking an answer in a menu, is marked `manual`: its comments are the procedure, and
`just record-live <scenario>` attaches your terminal to the agent and writes the recording once you
detach. The older recordings no scenario reproduces stay as they were made by hand: an API error
(`p-stopfailure`), print mode, which orca-term never launches (`p-sandbox-denied-bash`), a hang-up
or `/exit` (`tui-kill`, `tui-exit`, `tui-web`, `tui-esctool`), timing-dependent queues (`tui-queue`,
`tui-queueesc`) and deny rules with their menus (`tui-deny-*`).

### Releases

Pushing a tag `v*` runs `.github/workflows/release.yml`: it builds the core for the four targets,
packs each binary with LICENSE as `orca-term-<tag>-<target>.tar.gz` beside its `.sha256`, checks
every checksum, and only then creates the GitHub release with them. A pull request that touches
the build runs the same build and check as a dry run that publishes nothing, and so does
`gh workflow run release.yml --ref <branch>`.

Before tagging, set `version` in `Cargo.toml` to the release's version and tag it as `v<version>`, for
example `v0.1.0` for `0.1.0`: the release fails before building anything when the tag is not
exactly that, which `cargo xtask release-version <tag>` checks locally.

## Scope

In: parallel agents in isolated worktrees, diff review that can feed comments back to the agent
that wrote the code, the person's own Neovim beside each agent, over a standalone core.

Out: mobile companion, cloud relay, screenshot-driven design mode, Linear integration,
worktrees over SSH.
