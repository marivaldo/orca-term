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
<worktree>` run claude in one of them, inside claude's own sandbox, held by a detached process that
any terminal can attach to. `orca-term note
add|ls|edit|rm|send <worktree>` attach notes to a worktree's lines or files and paste them into its
agent as one prompt, in Orca's format. `orca-term turn ls|diff|blame <worktree>` list a worktree's
turns, show what one turn changed and say which turn wrote each line of a file. `orca-term doctor`
checks what orca-term needs on the machine.
`orca-term worktree rm <worktree>` keeps the worktree's branch unless it is merged, and refuses
modified or untracked files unless `--force`; an agent running there is stopped first (`stopped
the agent of worktree <name>`), once those checks pass, so a refused removal leaves it running.

In Neovim, `:OrcaTerm sidebar` shows the fleet in every tab. Each worktree gets a tab of its own,
pointed at it with `:tcd`: `<CR>` on a sidebar row goes there, `a` creates a worktree, `.` runs
its setup again, copies its path or removes it (always asking first, and saying when its running
agent will be stopped), `u` toggles its unseen
mark `•`, which a worktree gets when its agent's turn ends, `d` opens its review, and `K` shows
its detail in a float beside the sidebar: its state and agent, its branch and how far it is ahead
of and behind the local default branch (`↑2 ↓1 main`, never fetched), the whole error when it has
one, the tool calls claude's permission rules denied in its latest turn, and the end of its setup's
log. The float closes when the cursor moves, on `K` again, or on `q` once you enter it. At the
end of a row's second line, `⊘N` counts the tool calls denied in the worktree's latest turn and
`✎N` its notes waiting to be sent. These icons (`≠ ⊘N ✎N PR`) sit there, right-aligned, after what
the line says, which is cut with `…` to leave them room; a worktree with icons and nothing to say
gets a second line holding only them.
A row shows `≠` while its worktree is in the middle of a git merge, rebase, cherry-pick or revert,
and the float names it (`rebase in progress (2/5)`); `worktree ls` adds it to the state
(`idle, rebasing`). It is read from git's own files and is never a state.
The end of a row's first line names its agent, `cc` for Claude Code or `oc` for opencode, and how
long ago its current session was last active: `now`, `9m`, `3h` or `2d` (`cc 9m`), muted in the
group `OrcaTermAge`, linked by default to `Comment`. Within a section, the most recently active
worktree comes first, then the rest by name.
When a worktree has no question, error or setup failure to show and is not working, its row's
second line says how many files it changed against the default branch: `1 file changed`,
`4 files changed`. The count covers committed and uncommitted changes and untracked files that are
not ignored; `K` shows it under the branch too.
`:OrcaTerm jump` picks a worktree, `:OrcaTerm worktree N` goes to the Nth, `:OrcaTerm review`
opens the review of the worktree whose tab you are in, `:OrcaTerm notes` lists its notes and
`:OrcaTerm turns` lists its turns, to review one; bind keys to `<Plug>(OrcaTermSidebar)`,
`<Plug>(OrcaTermJump)`, `<Plug>(OrcaTermReview)`, `<Plug>(OrcaTermNotes)`, `<Plug>(OrcaTermTurns)`,
`<Plug>(OrcaTermSessions)` and `<Plug>(OrcaTermWorktree1)` to `<Plug>(OrcaTermWorktree9)`, since the client sets no global
key.

## Install

orca-term runs on macOS and Linux and needs Neovim 0.11 or later. With
[lazy.nvim](https://github.com/folke/lazy.nvim) it is one line:

```lua
{ 'marivaldo/orca-term', version = '*' }
```

`version = '*'` checks out the latest release tag. After installing or updating the plugin,
lazy.nvim runs its build hook, `build.lua`, by itself. The hook downloads that release's core
binary for your OS and architecture (macOS or Linux, arm64 or x86_64) from GitHub, checks its
sha256 against the checksum the release publishes beside it, and installs it in the plugin's
`bin/` directory. When the plugin is not at a release tag, the release has no binary for your
machine, or the download or the check fails, the hook says which and builds the core with
`cargo build --release` instead, which needs [Rust](https://rustup.rs). A binary whose checksum
does not match is never installed.

Other plugin managers run the same hook as a shell command in the plugin's directory,
`nvim -l build.lua`: `{ 'do': 'nvim -l build.lua' }` with vim-plug, `run = 'nvim -l build.lua'`
with packer. Pin the plugin to a release tag for the download; any other commit is built with
cargo.

The client looks for the core binary in this order: `binary` from
`require('orca-term').setup({ binary = '/path/to/orca-term' })`, the plugin's `bin/orca-term`,
the plugin's `target/release/orca-term` and `target/debug/orca-term` (from cargo), then
`orca-term` on `PATH`.

To run the core from the shell too, `:OrcaTerm install-cli` (or `<Plug>(OrcaTermInstallCli)`)
links the binary the client uses into `$XDG_BIN_HOME`, or `~/.local/bin` when that is not set,
creating the directory, and says where:
`orca-term: linked /home/you/.local/bin/orca-term -> /home/you/.local/share/nvim/lazy/orca-term/bin/orca-term`.
It warns when that directory is not on your `PATH`, and refuses to replace anything there that is
not a symlink.

## Setup

`worktree new` runs the setup script `scripts.setup` from `orca-term.yaml` in the new worktree, and
`worktree setup <worktree>` runs it again. A script from a committed `orca-term.yaml` was written
by whoever committed it, so it runs only once you have approved its exact text in this repository.
At a terminal, orca-term prints the script and the file it comes from, then asks
`run this setup script? [y/N]`; `y` approves it and runs it, anything else leaves the worktree
*not ready* until you run `orca-term worktree setup <worktree>` again. Without a terminal, or with
`--json`, nothing is asked: the command fails with the error `untrusted` (exit code 8) and the
worktree stays not ready. `--approve-setup` approves the script without asking, and the Neovim
client shows the script and asks `Run it` or `Cancel` before passing it. An approval is the sha256
of the script, kept in `.git/orca-term-trust.json`, outside every worktree and out of the agent's
reach; any change to the script, in the primary checkout or on a branch, asks again. A script from
`.git/orca-term.yaml` or from your global config file is yours and always runs.

The same goes for `sandbox.writable`, the extra paths claude's sandbox lets the agent write to: a
committed list widens the sandbox only once you have approved it. `agent start` prints the paths
and their file, then asks `widen the sandbox with these writable paths? [y/N]`; without a terminal
it fails with `untrusted` and starts nothing. `agent start --approve-sandbox` approves the list
without asking, and in Neovim `<CR>` shows it and asks `Allow` or `Cancel`. A changed list asks
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
- On macOS, claude's push over SSH fails inside its sandbox whatever the setting, because of
  claude's own network proxy.

## opencode

A worktree can run [opencode](https://opencode.ai) instead of claude:
`orca-term agent start <worktree> --agent opencode`. The worktree records its agent, so the next
`agent start` (and `<CR>` in Neovim) starts opencode again, resuming its session with
`opencode -s <id>`; a worktree that records no agent gets claude-code. `orca-term worktree ls`
shows `opencode (running)`. Asking for another agent while one runs is refused as `busy` (exit
code 3), naming the agent that runs.

`orca-term worktree new <name> --agent claude-code|opencode` creates the worktree and, once its
setup succeeded, records that agent and starts it, as `agent start --agent` would. A failed setup
leaves the worktree *not ready* and starts nothing. When the start is refused (no sandbox,
an unapproved `sandbox.writable` list, a missing or old agent), the worktree is still there and
ready, with the agent recorded, and the command exits with the refusal's error. In Neovim, `a`
asks for the name, then offers `claude-code`, `opencode` or `No agent`.

`orca-term agent swap <worktree> <agent>` swaps a worktree's agent: it stops the one running and
starts the other on the same worktree, which records it. It is refused as `busy` while the agent is
in a turn, working or waiting for you; stop it with `agent stop` first if you mean to. The turns,
the notes and the snapshots stay with the worktree. A session belongs to its agent: the new agent
resumes the session it recorded itself, if any, and starts a new one otherwise. In Neovim, `.` then
`Swap agent` asks which agent, swaps, and shows the new agent's terminal in the worktree's tab.

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
`no_sandbox` (exit code 5); opencode must be 1.18.0 or later (exit code 9 otherwise).

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
  start (`agent start`, `<CR>`); start a new one with `orca-term agent start <worktree> --fresh`.
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

In Neovim, `s` on a worktree in the sidebar lists its sessions, each as its agent, its turns, how
long ago it was last active and its first prompt, the current one marked `*`; choosing another one
resumes it and shows its agent's terminal in the worktree's tab. `.` then `New session` asks
first, then starts a new session. `:OrcaTerm sessions` (or `<Plug>(OrcaTermSessions)`) opens the
same list for the worktree under the cursor in the sidebar, or for the worktree whose tab (or
review) you are in.

Limits:

- A session is known from the turns it holds and from being current: one you leave before its
  first prompt is forgotten once another session is current.
- A claude session from before orca-term recorded each session's transcript (in the turn log) can
  be resumed only while it is the current one; any other refuses as `session_gone`, no transcript
  recorded.

## Review

The review shows what a worktree changed since it forked from the default branch: the commit it
forked from (`git merge-base <default branch> HEAD`) against its files on disk, so its commits and
its uncommitted changes show together, and the right-hand side is the worktree's real files. Files
git does not track yet are not listed. It needs
[diffview.nvim](https://github.com/sindrets/diffview.nvim); without it, `d` only says so.

`d` on a worktree in the sidebar (or `:OrcaTerm review` in its tab) goes to the worktree's tab and
opens the review from there, in a tab of its own where the sidebar is not shown; `d` again goes
back to that tab and refreshes it. Opening the review clears the worktree's unseen mark. In the
review, `gf` on a file (in the file list or in the diff) edits it in the worktree's tab, on the
same line when the cursor is in the right-hand side; when that tab shows only the agent's
terminal, the file opens beside it. Close the review with `:DiffviewClose`, or `:tabclose`.

## Notes

A note is a remark for the worktree's agent on a line, a range of lines or a whole file of its
review. Notes wait until you send them; sending types them into the agent as one prompt, in Orca's
format (`File:`, then `Line:`, `Lines:` or `Scope: file`, then `User comment:`), and consumes them.

In the review (and only there), on the right-hand side, the worktree's file:

- `gn` adds a note on the cursor's line, asking what it says (`Note for the agent: `); in visual
  mode, on the selected lines. `gn` on a file in the file list adds a note on the whole file. The
  left-hand side is the review's base: `gn` there only says notes go on the right. Write the file
  first: notes go on the file on disk, so a buffer with unsaved changes is refused.
- Each note shows as `✎` beside its line (a range's other lines get `┆`), its text below it, at its
  line in the file on disk now: when the agent adds lines above it, it moves down; when its line is
  changed or deleted, it becomes a note on the whole file. A note on the whole file shows below
  line 1, as `✎ file: <text>`. The file list shows `✎N` after each file with notes.
- `gN` (or `:OrcaTerm notes`, also from the worktree's tab) lists the notes: `Send all (N)` first,
  then each note, which you can send alone, edit or delete (deleting asks nothing, as in Orca).

Sending needs the worktree's agent running; when it is not, press `<CR>` on the worktree in the
sidebar to start it, then send again. The notes stay pending until then.

Limits:

- Notes show only in the review orca-term opens (`d`), not in a diffview opened by hand, nor when
  the file is edited in the worktree's tab.
- `gn` works on the files the review lists; a file git does not track is not listed, but a note
  added on it with `orca-term note add` shows in the list and is sent with the others.

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

In Neovim, `:OrcaTerm turns` in a worktree's tab (or its review) lists the worktree's turns, newest
first, each as its number, how it ended and its prompt; bind a key to `<Plug>(OrcaTermTurns)` for
it. Choosing one opens what that turn changed in diffview, its begin snapshot against its end
snapshot, in a tab of its own where the sidebar is not shown; choosing it again goes back to that
tab. An interrupted turn says that its end was not observed. An open turn, or one from before
orca-term took snapshots, cannot be reviewed: choosing it says why and opens nothing. In a turn
review, `gf` on a file edits it in the worktree's tab, as in the review. Notes are not added there:
they go on the files on disk, so `gn` says to add them from the worktree's review (`d`), which `d`
always opens, never a turn review.

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
- In a turn review, `gf` from the right-hand side (the turn's end) lands on the same line number in
  the file on disk, which later edits may have moved.

## Pull requests

`orca-term pr refresh [<worktree>...]` asks the GitHub CLI about the pull request of each
worktree's branch (every worktree when you name none) and its checks, and keeps the answer for the
sidebar. It prints one line per worktree:

```text
demo: PR #12 open, checks pending
docs: no PR
spike: PR unavailable: GitHub CLI is unavailable
```

A pull request is `open`, `draft`, `merged` or `closed`, and its checks are `failing` when any
check failed, errored, was cancelled or timed out, else `pending` while any is queued or running,
else `passing` when any succeeded, else `none`. It runs `gh pr view <branch>` in every worktree at
once and gives each 10 seconds. When gh is not installed, not logged in or the repository is not
on GitHub (`not a GitHub remote`), the worktree's pull request is unavailable, with the reason;
the command still succeeds. orca-term never runs gh anywhere else, and the Neovim client never
runs it at all.

`worktree ls --json` and `worktree detail --json` show what the last refresh found as each
worktree's `pr`: `null` when it never ran, else `fetched_at` (milliseconds since the Unix epoch)
and either the pull request (`number`, `title`, `state`, `url`, `checks`), `"none": true`, or
`unavailable` and the reason. They also show each worktree's `last_activity`: when its current
session was last active, in milliseconds since the Unix epoch, or `null` when it has no current
session, and `changed_files`: how many files it changed against its merge base with the default
branch, committed or not, untracked files that are not ignored included, or `null` when it cannot
be counted (no shared history, a broken worktree, one still setting up). The text
`worktree detail` adds a line such as `3 files changed` when there are some.

In the sidebar, a worktree whose branch has a pull request shows `PR` at the right of its row,
after `✎N`: green, yellow or red by its checks while it is open (green with no checks), purple once
merged, muted when closed or a draft. `K` adds its line to the detail float, such as `PR #12 open,
checks passing (2 min ago)`, with its address. `R` refreshes the worktree under the cursor, and
`:OrcaTerm pr-refresh` (`<Plug>(OrcaTermPrRefresh)`) every worktree. The client also refreshes on
its own, through the core and without blocking: when the sidebar opens or Neovim gains focus with
it open (each worktree at most once a minute), after a worktree's turn ends, and every minute while
the sidebar is open and some checks are pending. The colours are the groups `OrcaTermPrOpen`,
`OrcaTermPrMerged`, `OrcaTermPrClosed`, `OrcaTermPrDraft`, `OrcaTermPrPassing`, `OrcaTermPrFailing`
and `OrcaTermPrPending`, linked by default to `DiagnosticOk`, `Constant`, `Comment` and
`Diagnostic*`.

## Status strip

The status strip shows the fleet's health while the sidebar is closed: how many worktrees are in
each state, then how many are unseen, such as `?1 ◐2 ●3 •2`. orca-term never adds it to your
statusline, tabline or winbar: you place it, and pad it, where you want. It shows nothing when the
repository has no worktree, outside a git repository and before the fleet is first read. It
updates when the core says a worktree changed, as the sidebar does.

In a plain statusline, as an expression:

```lua
vim.o.statusline = "%f %= %{%v:lua.require'orca-term.strip'.statusline()%} %l:%c"
```

With [lualine](https://github.com/nvim-lualine/lualine.nvim), as the `orca-term` component (each
count keeps the section's background):

```lua
require('lualine').setup({ sections = { lualine_x = { 'orca-term', 'filetype' } } })
```

With [LazyVim](https://www.lazyvim.org), add it to the front of `lualine_x` (orca-term must be on
the runtimepath when lualine starts, so do not lazy-load it):

```lua
{
  'nvim-lualine/lualine.nvim',
  opts = function(_, opts)
    table.insert(opts.sections.lualine_x, 1, 'orca-term')
  end,
}
```

The glyphs are the sidebar's, in the order of its attention: `?` waiting for you, `●` failed (in
red), `●` interrupted (muted), `!` broken, `△` not ready, `◐` working, `◌` setting up, `●` finished
(in green), `○` idle, `·` no agent, then `•` unseen. Each takes the colour of its
`OrcaTermState*` group, and `•` of `OrcaTermUnseen`.

To build a strip of your own, `require('orca-term.strip').counts()` gives the counts of the fleet
last read: `{ states = { { state = 'waiting', count = 1 }, ... }, unseen = 2 }`, in the same order,
leaving out the states no worktree is in.

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
later. When git signs with an ssh key only an agent can use, an `info` line before the contract
names `sandbox.agent_socket` (see Signing). With `--json` it prints the same checks as one
document. It exits 0 unless something is missing or too old; then it prints the whole report and
exits 9.

Every command that needs a missing or outdated program refuses with the same line, exit 9: git for
every command, claude or opencode for `agent start`, such as
`orca-term: git 2.30.1: orca-term needs 2.38 or later; run orca-term doctor for the full report`.

In Neovim, `:checkhealth orca-term` shows the core binary the client uses, the doctor's checks,
whether the core speaks the client's contract, the Neovim version (0.11 or later) and whether
diffview.nvim is there for the review.

## Development

Needs Rust 1.97 and [just](https://github.com/casey/just). `just check` runs every check CI runs;
see `docs/harness.md` for the tools it expects and the rules it enforces. `just hooks` installs the
local git hooks (needs [lefthook](https://github.com/evilmartians/lefthook)).

### Tools for the Neovim client

`just lua`, part of `just check`, formats, lints and type-checks the Lua and runs its specs in
headless Neovim. It needs Neovim 0.11 or later, [StyLua](https://github.com/JohnnyMorganz/StyLua),
[selene](https://github.com/Kampfkarren/selene),
[lua-language-server](https://github.com/LuaLS/lua-language-server) and
[LuaRocks](https://luarocks.org) with LuaJIT. Set them up once; on macOS with Homebrew:

```sh
brew install neovim luajit stylua selene lua-language-server luarocks
just lua-rocks   # installs busted, nlua, lualine.nvim and diffview.nvim, pinned, into .rocks/ (ignored by git)
```

On Linux, CI shows the way: the tools come from their GitHub releases at the versions pinned in
`.github/workflows/check.yml`, LuaRocks and the Lua 5.1 headers from the distribution
(`luarocks liblua5.1-0-dev` on Ubuntu), then `just lua-rocks`.

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
output. A full run is 10 prompts (8 of them claude's) and takes a few minutes:

| Scenario | Agent | What the recording shows | Prompts |
|---|---|---|---|
| `p-hello` | claude | a plain prompt finishes | 1 |
| `tui-esc` | claude | Esc mid-turn interrupts it; the next prompt finishes | 2 |
| `tui-queuetool` | claude | a prompt queued while a command runs folds into that turn | 2 |
| `tui-perm` | claude | a permission ask waits; refused with Esc, then allowed | 2 |
| `tui-sandbox` | claude | the sandbox stops a write into the worktree's admin directory, and the file is checked missing | 1 |
| `opencode-hello` | opencode | a plain prompt finishes | 1 |
| `opencode-esc` | opencode | Esc twice mid-turn interrupts it | 1 |
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
`doctor` suite diverge until `CLAUDE_TESTED` in `src/domain/prerequisite.rs` names it: raise it and
commit it with the recordings.

A scenario is a small file in `tests/live/`: the agent, the recording's name, a timeout, the
outcome of each turn the recording must replay to, then the keys to type and the hooks to wait for
(`xtask/src/live/scenario.rs` documents the format). One that cannot be typed reliably, such as
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

## Scope

In: parallel agents in isolated worktrees, diff review that can feed comments back to the agent
that wrote the code, Neovim integration over a standalone core.

Out: mobile companion, cloud relay, screenshot-driven design mode, Linear integration,
worktrees over SSH.
