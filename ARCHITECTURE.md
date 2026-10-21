# Architecture

This file is the map of `orca-term`, its core and its Neovim client: where each concept lives and the
rules the code keeps.
Read it before changing the code; update it when a module moves. The words follow the glossary in
`CONTEXT.md`, and the decision behind the shape is ADR 0010 (`docs/adr/`).

## Bird's-eye view

`orca-term` gives a person a fleet of git worktrees, each on its own branch, for parallel coding
agents. The core is a Rust command-line program: each command reads the repository through git and
the file system, decides what to do with pure rules, does it, and prints the result as text for a
person or as JSON for the Neovim client. A worktree's agent (claude, for now) runs inside a
detached process of the core, its holder, which keeps the agent's terminal and screen so that any
terminal can attach to it and detach again (ADR 0009), under claude's native sandbox (ADR 0007).

The core is a library (`src/lib.rs`) with a thin binary (`src/main.rs`). Inside the library, every
module sits in one of five layers, and code may use only its own layer and the layers below it:

```text
main      src/main.rs, src/lib.rs     parse the arguments, run, turn an error into an exit code
edge      src/cli.rs, src/output*     the command-line surface and everything printed
ops       src/ops*                    one module per command
adapters  src/adapters*               the only code that touches git, files, processes and the environment
domain    src/domain*                 pure rules: data in, data or a decision out
```

A command goes down and comes back up: `cli` parses it and calls its ops module, the ops module
asks the adapters for facts, hands them to the domain for a decision, acts on the decision through
the adapters, and returns a domain value that `output` prints.

## Code map

### main

- `main.rs`: parses the arguments with clap, calls `orca_term::run` and exits with the code it
  returns, or hands the error to `output::error`, which prints it and returns its exit code.
- `lib.rs`: declares the layer modules; exposes `Cli`, `run`, `output::error` and
  `output::Format` to the binary.

### edge

- `cli.rs`: the clap types for every command and flag (`worktree ls|new|setup|rm|prune`, the
  person's marks `worktree seen|unseen`, `worktree review`, which the sidebar runs before it
  opens a review, `worktree detail`, what the detail float shows,
  `agent start|attach|stop`, `note add|ls|edit|rm|send`, `turn ls|diff|blame`, the hidden `agent holder`, the holder process itself, and the
  hidden `hook claude`, which claude runs for every hook event, and the hidden
  `nvim register|unregister <address>`, which a running Neovim runs), the
  dispatch to `ops`, and the exit code of a command that did its job (1 when a setup failed).
  `doctor [--json]` needs no repository; every other command but the hidden `hook` and `agent
  holder` first checks git once (`runs_git`, then `ops::check_git`), refusing a missing or too old
  git as the typed `prerequisite` before anything else runs.
- `output.rs`: the only writer to stdout and stderr; renders the fleet table, the reports of
  `worktree new`, `setup`, `rm`, `prune`, `seen|unseen`, `review` and `detail` (a block of lines:
  the state, the whole detail indented, the denials, the setup and the end of its log, the
  drift), of `agent start` and `stop` (and, on stderr, the
  sandbox's extra writable paths with the config file that set them, and how an attach ended),
  of the note commands (a note's lines as made and, when they moved, `(now line 5)`; the prompt a
  send pasted), of the turn commands (`turn ls`'s table, `turn diff`'s patch written to stdout
  byte for byte with, on stderr, the warning of an end not observed, and `turn blame`'s lines:
  the turn right-aligned in 4 columns, `-` for no turn, `·` for since the latest snapshot, then
  the line number and the text),
  and errors (as text on stderr, or as JSON on stdout with `--json`). `Format` is text or JSON. Before a command that may run a setup starts, `cli`
  has `output` say on stderr what it starts (`creating worktree fix...`); `live_echo` is the
  writer the setup's output is then echoed to while it runs (stderr, or nowhere with `--json`),
  handed by `cli` to the ops module. `doctor`'s report is one line per check, its status padded
  to 8 columns (`ok`, `warn`, `missing`, `too old`, `info`) then its message; when a check is
  missing or too old, the whole report comes first, then the `prerequisite` error as any command
  prints it, and its exit code.
- `output/json.rs`: the JSON documents of `--json` (the fleet, `worktree new`, `worktree setup`,
  `worktree rm`, `worktree review`, `agent start`, `agent stop` and errors), built as views from domain values, with the `contract`
  and `version` envelope. Each worktree carries `agent` (`"claude-code"` or null) and `holder`
  (whether a holder is alive) and `unseen` (whether it carries the unseen mark); `worktree seen`
  and `unseen` print the worktree and its mark; `worktree review` prints the worktree object
  (now seen), `base` (the review's base, a full commit id) and `against` (the default branch it
  was taken from, `main` or `origin/main`). The text table adds `(unseen)` to a marked row.
  Each worktree also carries `notes`, how many notes are pending on it, and the text table adds a
  `NOTES` column when some worktree has any. `note add` and `note edit` print the worktree's
  `name` and `path` and the `note` (`id`, `path`, `anchor`, `current`, `body`, `rev`, `created`;
  an anchor is `{"scope":"line","line":N}`, `{"scope":"lines","start":S,"end":E}` or
  `{"scope":"file"}`); `note ls` the same with `notes`, in id order; `note rm` the `id` removed;
  `note send` the `prompt` pasted, the ids `sent` and those `kept` (edited while being sent).
  Each worktree also carries `denials`, how many tool calls its agent was denied in its latest
  turn (open or ended, whatever its outcome). `worktree detail` prints the worktree object,
  `detail` (why it is in its state, whole, or null), `denials` (`[{"tool", "text"}]`, in order),
  `setup` (`{"status": "running"|"interrupted"|"succeeded"|"failed", "log", "tail"}`, the last 20
  lines of the log, or null when it never ran one) and `drift` (`{"against", "ahead", "behind"}`,
  or null). `turn ls` prints the worktree's `name` and `path` and `turns`, each `{"number",
  "agent", "session", "prompt", "started_at", "ended_at", "outcome", "phase", "end_observed",
  "begin", "end", "denials"}` (`outcome` null while open, `phase` `working`/`waiting` only while
  open; nulls for what a turn from before the snapshots lacks); `turn diff` the `turn`, its
  `begin` and `end`, `end_observed` and the `patch`; `turn blame` the `file` (relative to the
  worktree) and its `lines`, each `{"line", "source": "turn"|"between_turns"|"since_snapshot",
  "turn", "agent", "snapshot", "text"}`. `doctor` prints `checks`, each `{"name", "status",
  "found", "need", "message"}` (`status` `ok`, `warn`, `missing`, `too_old` or `info`; `found` and
  `need` null when there is nothing to say), and `error` (kind `prerequisite`) only when a check
  is missing or too old.

### ops

- `ops.rs`: what several commands share: reading the fleet and deriving each worktree's state,
  agent and holder, finding the worktree a command names, taking a worktree's lock, refusing a
  change under a live holder, writing `worktree.json` (the setup or the agent, keeping the other),
  reading the config files, running a worktree's setup, the registry of running Neovims
  (`nvim_registry`), the notes' shared steps (`find_noted`, `read_notes`, `change_notes`, which
  holds `notes.lock` from the read to the write and then removes the kept contents no note refers
  to, `keep_copy` and `pending`, which translates a note's anchor to the file now through
  `git diff --no-index` of its kept content), and telling them a worktree changed: `Changed` finds the common dir and the
  worktree's path from git's `commondir` and `gitdir` files beside the admin dir (no git run),
  and `notify_changed`, which every command calls after it writes a worktree's state. The turn
  commands share `find_turned` (refusing a worktree whose directory is gone) and `read_history`
  (the whole `turns.jsonl` and `status.json`, read with no lock: the hook only appends to the one
  and replaces the other whole). The prerequisites: `check_program` finds a program on PATH as a
  shell would and asks its `--version`; `check_git` (called once per command by `cli`) and
  `find_claude` (by `agent start`) refuse a missing or too old one as the typed `prerequisite`.
- `ops/doctor.rs`: `doctor`: check git, claude's sandbox tools, each agent and the contract, in
  that order. It only looks, needs no repository and never fails.
- `ops/worktree_new.rs`: `worktree new`: check the name, resolve the config, add git's worktree,
  lock it, copy the copy list, run the setup.
- `ops/worktree_setup.rs`: `worktree setup`: find the target, lock it (or fail as `busy`), run
  the setup again.
- Both run the setup through `ops::set_up`, which, for a script from a committed
  `orca-term.yaml`, first checks its approval (#58): the sha256 of its text in
  `<git common dir>/orca-term-trust.json`, else the person's `Approval` (`Given` by
  `--approve-setup`, `Ask` at a terminal through the function `cli` hands down, `Unavailable`
  otherwise). Approving records it under `<git common dir>/orca-term/trust.lock`; refusing writes
  `not_approved` to `worktree.json`, notifies, and returns the typed `untrusted`.
- `ops/worktree_ls.rs`: `worktree ls`: the fleet of the current repository.
- `ops/worktree_seen.rs`: `worktree seen` and `worktree unseen`: find the target (refusing the
  primary checkout), read the latest ended turn as `worktree ls` does, replace `seen.json` whole
  (no lock, so never `busy`; never over a newer schema), notify. Doing either twice changes
  nothing more. `mark` writes the mark and notifies; `worktree review` marks through it too.
- `ops/worktree_review.rs`: `worktree review`: find the target (refusing the primary checkout
  and a worktree whose directory is gone), resolve the default branch as `worktree new` resolves
  its start point, find the review's base with `git merge-base <default branch> HEAD` in the
  worktree, then mark it seen as `worktree seen` does. The base comes first, so a failure (no
  default branch, unrelated histories) marks nothing.
- `ops/worktree_detail.rs`: `worktree detail`: find the target (refusing the primary checkout and
  an unknown worktree), read the hooks' live status for the whole detail and the latest turn's
  denials, `worktree.json` and the end of the setup log (64 KiB at most), and the drift: the
  default branch only when it exists locally (`refs/heads/<default>`, never `origin/`), `git
  merge-base` against it (none: unrelated histories, no drift), then `git rev-list --left-right
  --count`. A worktree whose directory is gone gives only its state and reason. Only reads.
- `ops/worktree_rm.rs`: `worktree rm`: find the target, lock it (or fail as `busy`, even with
  `--force`), refuse modified or untracked files without `--force` (the typed `dirty`), decide
  the branch's fate, remove, delete its chain of snapshots (`refs/orca-term/worktrees/<id>`, the
  id learnt before git removes the admin directory), delete or keep the branch.
- `ops/worktree_prune.rs`: `worktree prune`: `git worktree prune`, then delete every chain of
  snapshots whose `.git/worktrees/<id>` is gone (a worktree removed by hand), and report both.
- `ops/agent_start.rs`: `agent start`: find the target (ready, not the primary checkout), lock it
  (or fail as `busy`), report a holder already alive, probe the sandbox (or fail as
  `no_sandbox`), find claude on PATH and check its version (or fail as `prerequisite`), resolve
  the extra writable paths (those of a committed `orca-term.yaml` approved, else asked for or
  refused as `untrusted`),
  pre-create the admin directories, pick the session (none with `--fresh` or without a recorded
  one; else the current session `status.json` records, once its id is checked and its transcript
  found on disk, or the typed `session_gone`), build and check the settings and the command line
  (`--resume <id>` on a cold start), write `claude-settings.json`, start the holder and wait for
  its first word, record the agent. The report says which session was `resumed`, if any.
- `ops/agent_attach.rs`: `agent attach`: find the target, refuse when no holder is alive (naming
  `agent start`), connect, hand the terminal to the holder until detach or the agent's end.
- `ops/agent_stop.rs`: `agent stop`: find the target, nothing to do when no holder is alive, else
  ask the holder to stop and wait for `agent.lock` to be released (at most 10 s).
- `ops/agent_holder.rs`: the hidden `agent holder`: the holder process, `adapters/holder/process.rs`.
- `ops/hook.rs`: the hidden `hook claude --admin <dir> <event>`: read the payload from stdin,
  take `hook.lock` (waiting at most a second, never the worktree's lock), apply the event, take
  the snapshot when the event opened or closed a turn, fill its id into the records, append to
  `turns.jsonl`, replace `status.json`, and exit 0 without printing whatever happens; what went
  wrong is appended to `hook.log`, emptied first once past 256 KiB. The snapshot: copy the real
  index (`git rev-parse --git-path index`) to `snapshot.index` in the admin directory, `git add -A`
  and `git write-tree` through the copy (`GIT_INDEX_FILE`), `git commit-tree` with the identities
  and dates in the environment, `git update-ref` as a compare-and-swap, remove the copy. A failed
  snapshot is logged and the turn recorded without its id. Every other event runs no git, to stay
  fast.
- `ops/note_add.rs`: `note add`: check the body and the path, find the target (refusing the
  primary checkout and a worktree whose directory is gone), refuse a path that leaves the worktree
  through a symlink, read the file for a note on lines and check the lines fit, then, under
  `notes.lock`, keep the file's content under its blob id and record the note; notify.
- `ops/note_ls.rs`: `note ls`: the pending notes, each with its anchor translated to the file now.
  Only reads.
- `ops/note_edit.rs`, `ops/note_rm.rs`: `note edit` (a new body, the revision bumped) and `note rm`,
  each under `notes.lock`; notify.
- `ops/note_send.rs`: `note send`: read the notes to send (all, or the ids given), refuse with no
  pending notes and, as the typed `no_holder`, with no live holder; translate each anchor, build
  Orca's prompt, type it into the agent through the holder (the paste, 50 ms, then Enter), and only
  then, under `notes.lock`, consume the notes sent whose revision did not change; notify.
- `ops/turn_ls.rs`: `turn ls`: the worktree's turns from its turn log. Only reads.
- `ops/turn_diff.rs`: `turn diff`: the turn's begin and end snapshots from the turn log (refusing
  a turn that does not exist, one not ended, one without snapshots), then `git diff <begin>
  <end>`. Only reads.
- `ops/turn_blame.rs`: `turn blame`: resolve the path (relative to the worktree, or absolute
  inside it; refused outside it, also through a symlink, and when it is not a file on disk), then
  `git blame --porcelain --contents <file on disk> <tip> -- <path>` against the chain's tip
  `status.json` records, mapped through the turn log. No tip, a tip git no longer has, or a file
  the tip does not hold: every line changed since the latest snapshot. Only reads.
- `ops/nvim_register.rs`: the hidden `nvim register <address>`: write the address's file in the
  registry (idempotent; creating the file is the only write, so no lock).
- `ops/nvim_unregister.rs`: the hidden `nvim unregister <address>`: remove the address's file;
  one not recorded is not an error.

### adapters

- `adapters/git.rs`: every git process, one function per question (`worktree_list`,
  `default_branch`, `local_default_branch`, `is_merged`, `merge_base`, `left_right_counts` (the
  drift's `rev-list --left-right --count`), `blob_id` of some content, `diff_files` between two
  files, `index_path`, `config_value`, `read_ref`, `refs_under`, `commit_exists`, `has_file`, the
  turn commands' `patch` (`git diff` between two snapshots) and `blame_porcelain`, ...) or change (`worktree_add`,
  `delete_branch`, the snapshot's `write_tree_through` a given index, `commit_tree` and
  `update_ref`, `delete_ref`, ...).
- `adapters/fs.rs`: reading and atomically writing files, plain byte copies, creating and listing
  directories (`list_file_names`: the non-directory entries, sorted, empty when the directory is
  missing), resolving paths through symlinks, reading a symlink's text (`link_text`, Linux only,
  for the holder's peer check), finding an executable in the PATH directories, and locks: a worktree's `WorktreeLock`, taken without waiting, the holder's `FileLock` on
  `agent.lock`, and the read-only probes `ls` uses for both.
- `adapters/script.rs`: the setup script's process, `bash -e -c <script>` in the worktree, its
  stdout and stderr merged through one pipe and copied to the setup log and the live echo.
- `adapters/env.rs`: the current directory, `HOME`, `XDG_CONFIG_HOME` and the directories of
  `PATH`.
- `adapters/program.rs`: running a program for its `--version`.
- `adapters/stdin.rs`: the process's standard input, read whole: a hook's payload; or one line:
  the person's answer to `run this setup script? [y/N]`.
- `adapters/holder.rs`: starting a holder as a detached child with a pipe for its first word
  (`ok <pid>` or `error <why>`, 10 s at most), connecting to a holder's socket, typing into the
  agent's terminal as one more client (`type_input`: two input frames with a pause between, no
  size, no stop, never reading the replay), and asking it to stop. The socket is bound and connected by its bare name after a change of directory into the
  admin directory, since a unix socket address holds about 100 bytes.
- `adapters/holder/process.rs`: the holder process: a session of its own (`setsid`), the
  `flock` on `agent.lock` (retried for half a second, since `ls` probes it), the socket, the PTY
  and the agent on it, `agent.json`; then, until the agent exits, its threads: the main thread
  reaps the agent, the reader pumps the PTY into the scrollback (8 MiB) and to every client's
  bounded queue, the acceptor starts a client thread per connection, each client has a writer
  (replay first, then the queue), and a stop spawns the escalation (hang up, terminate after 3 s,
  kill after 3 more); a write to a client that blocks for 3 s drops that client as stuck, and
  the writers are joined for at most 5 s after the agent exits. A client's first size is "jiggled" (one column narrower, then back, again
  a second later) so the agent redraws for the new terminal. On the agent's exit every client
  gets `x <exit>`, the socket and `agent.json` go, and the process exits, releasing the lock.
  Each connection is first checked by `holder/peer.rs`; one it refuses is closed at once.
- `adapters/holder/peer.rs`: whether the holder serves a client (#61). On Linux it reads the
  client's pid from the socket (`SO_PEERCRED`), then its network namespace
  (`/proc/<pid>/ns/net`) and the holder's own (`/proc/self/ns/net`), and asks
  `protocol::admit`; elsewhere every client is served.
- `adapters/nvim.rs`: a running Neovim's server socket: `send` connects, writes one message and
  hangs up, on a thread of its own, within 200 ms, and only says whether it was `Delivered`, the
  Neovim is `Gone` (no socket, or nobody listening) or it was `Skipped` (anything else); it never
  fails.
- `adapters/terminal.rs`: the terminal of `agent attach`: raw mode (restored on the way out,
  followed by a reset of the screen modes an agent leaves behind), the size sent on connect and on
  every `SIGWINCH`, what is typed forwarded as input (Ctrl-\ detaches unless `--no-detach-key`),
  the holder's output written to stdout. Without a terminal it only moves bytes, and the end of
  its input detaches. `is_interactive` says whether stdin and stderr are both terminals, so a
  setup script can be approved at a prompt.

### domain

- `domain/worktree.rs`: a worktree's name rules and path, which worktree a `rm` target means,
  and the branch's fate on removal.
- `domain/branch.rs`: a branch's name, the start point a new branch is created from, and git's
  rules for a valid branch name, mirrored without running git.
- `domain/fleet.rs`: the fleet derived from git's list: the primary checkout apart, names,
  gone worktrees, what a prune cleaned.
- `domain/porcelain.rs`: parsing `git worktree list --porcelain -z`.
- `domain/config.rs`: `orca-term.yaml` parsing (`base`, `scripts.setup`, `sandbox.writable`),
  the precedence of the config files, which checkout supplies the committed file (the primary
  checkout for `base`, the worktree for `scripts.setup` and `sandbox.writable`), `~` and relative
  path expansion.
- `domain/state.rs`: a worktree's state and why it is broken or not ready, derived from
  `worktree.json`, the lock and the setup log; its admin directory and the files in it; and the
  `worktree.json` schema rules and bytes; and the git common dir and the worktree's path read
  from git's `commondir` and `gitdir` files beside the admin directory.
- `domain/nvim.rs`: the running Neovims of a repository: the registry,
  `<git common dir>/orca-term/nvim/`, one file per Neovim named by the 64-bit FNV-1a hash of its
  address in 16 lowercase hex digits and holding the address; `Address` (the absolute path of a
  unix socket, `v:servername`); and the msgpack-RPC request, encoded by hand, that fires
  `User OrcaTermWorktreeChanged` with the worktree's path.
- `domain/agent.rs`: the agent a worktree runs (`Agent::ClaudeCode`, recorded as `claude-code`,
  run as `claude`), whether its holder is alive, the holder's record (`agent.json`), a terminal
  size, and the reports of `agent start`, `stop` and `attach`.
- `domain/launch.rs`: claude's launch: the command line (`--settings <file>
  --permission-mode acceptEdits`, plus `--resume <id>` on a cold start), the session a cold start
  resumes (`session_to_resume`, `check_transcript`), the settings file that turns its sandbox on and denies our
  state (ADR 0007) and registers the hooks, the variables stripped
  from its environment, and the guard that refuses a launch bypassing permissions. The
  forbidden flags and modes are listed here and nowhere else.
- `domain/hook.rs`: what claude's hook payloads report and how each event changes a worktree's
  live status: a prompt opens a turn, `Stop` finishes it, `StopFailure` fails it, a new prompt or
  a session start or end interrupts the open one; queued prompts are told apart by `prompt_id`,
  subagents (payloads with `agent_id`) never open or close a turn, and events of a turn that
  already ended are ignored. Tools set the activity, permission requests and questions wait.
  `PostToolBatch` reports the tool calls denied (`is_denial`, by claude's words in a call's
  `tool_response`), recorded on the open turn its `prompt_id` names, a subagent's too, once per
  `tool_use_id`, never touching the phase or the activity. A failed turn keeps the agent's whole
  last message.
- `domain/turn.rs`: the live status (`status.json`, its schema rules and bytes, the current
  session and its `transcript`) and the turn log
  (`turns.jsonl`, one line at a turn's start and one at its end, numbered per worktree across
  sessions), with texts kept to one capped line; each turn's denials (`Denial`: the tool, the
  call as the activity shows it, and its id in `status.json` only; at most 100 a turn) and the
  latest turn's (`Status::latest_denials`); each turn's agent, its `begin` and `end` snapshots and
  whether its end was observed, and the chain's tip (`Status::snapshot`).
- `domain/snapshot.rs`: the snapshots that bound a turn (ADR 0006): which turns one snapshot
  bounds (`bounds`, from the lines an event wrote: none when it neither opened nor closed a turn),
  its message (subject and `Orca-Term-*` trailers), its author (`Identity`: the agent for an end,
  the person for a begin alone; `orca-term` commits), how it joins the chain (`chain_step`: the
  ref's commit is the parent only when it is the tip `status.json` records; any other ref is
  stale and replaced), filling its id into the records and the status (`fill`), and the chain's
  ref `refs/orca-term/worktrees/<admin-dir id>`.
- `domain/history.rs`: a worktree's turns as `turn ls` and `turn diff` show them (`turns`: the
  turn log's start and end lines merged per number, oldest first, the open turn and its phase
  from the live status), and the snapshots a turn's diff runs between (`diff_bounds`, refusing
  with `TurnError`).
- `domain/blame.rs`: `turn blame`'s mapping: `git blame --porcelain` parsed into lines
  (`parse_porcelain`), each line's commit mapped through the turn log (`attribute`: a turn's end
  snapshot is that turn, any other snapshot no turn, git's all-zero id since the latest
  snapshot), every line of a file no snapshot holds (`unsnapshotted`), and the path given made
  relative to the worktree (`worktree_file`).
- `domain/seen.rs`: the unseen mark: `seen.json` (`seen_through`, the latest turn the person
  has seen, and `marked`, the mark set by hand; its schema rules and bytes), the latest ended
  turn (an open turn whose holder is gone counts as ended), and whether a worktree is unseen.
- `domain/note.rs`: notes: `notes.json` (`next_id` and the pending notes, each with its `id`,
  `path`, `anchor`, `body`, `rev`, `created` and, on lines, the `copy` of the file's content it
  was made against; its schema rules and bytes; a file this core cannot use is refused, never
  written over), what a note's path, lines and body may be, how many lines a file has, choosing
  the notes to send and consuming them (a note whose revision changed is kept), and the reports of
  the note commands.
- `domain/note/translate.rs`: line translation: the hunks of a diff with no context lines, and a
  note's anchor mapped over them to the file now (a line moves by the hunks before it, a line
  inside a hunk's old side is gone, and a note whose lines are gone is on the whole file).
- `domain/note/prompt.rs`: Orca's prompt, identical (`File:`, `Line:`/`Lines:`/`Scope: file`,
  `User comment:` with the body escaped; blocks joined by a blank line), and the bytes that paste
  it: a bracketed paste with every ESC replaced by `␛`, then Enter alone after 50 ms.
- `domain/review.rs`: a worktree's review: `Commit`, a full commit id checked as git prints it
  (with its seven-character short form), what `worktree review` found (`Reviewed`), and its
  refusals (a worktree whose directory is gone, a history unrelated to the default branch).
- `domain/sandbox.rs`: whether claude's sandbox can run here: `sandbox-exec` on macOS, `bwrap`
  and `socat` on Linux, nothing elsewhere; missing means the typed `no_sandbox`, whose message is
  the doctor's sandbox check (`check`, `diagnosis`) word for word.
- `domain/prerequisite.rs`: the programs orca-term needs (`Requirement`: git from 2.38, claude
  from 2.1.295 and tested up to 2.1.303, opencode from 1.18.0 and optional until #35), read from
  what `--version` printed (`Version`, `Probe`), and each `Check` of the doctor's `Report`: `ok`,
  `warn` (claude newer than tested, an old optional program), `missing`, `too_old` or `info` (the
  contract). A missing or too old check is the typed `prerequisite` (`refusal`, `require`).
- `domain/protocol.rs`: the frames between a holder and its clients (a tag byte, a 4-byte
  big-endian length, the payload: input `i`, resize `r`, stop `s`, output `o`, exit `x`), their
  incremental decoder, and the scrollback a holder keeps; and `admit`, the rule for which
  clients a holder serves (both network namespaces read, and the same).
- `domain/setup.rs`: the setup: how it ended, what `worktree.json` records, the variables its
  script gets, the last line of its log, the end of its log as a person reads it (`log_tail`,
  the terminal's control sequences taken out by `strip_terminal_codes`), and the reports of the
  commands that run it.
- `domain/trust.rs`: the approval of what a committed `orca-term.yaml` asks for (#58), its setup
  script and its `sandbox.writable` list: `orca-term-trust.json` in the git common dir
  (`{"schema": 1, "approved": [{"kind", "sha256", "source", "approved_at"}]}`, `kind` `setup` or
  `sandbox`, a missing one read as `setup`; read from any schema, never written over a newer one,
  a corrupt file refused by name), the sha256 of the script's exact bytes or of the list's paths
  as written joined by `\n`, the answers that approve (`y`, `yes`, any case) and `gate`, what to
  do given whether it is committed, whether it is approved and the person's `Consent`.
- `domain/detail.rs`: what `worktree detail` reports (`Detail`): the whole detail
  (`whole_detail`: the state's own, or a failed turn's whole message), where the setup is
  (`SetupProgress`) and the drift counted from git's output (`Drift`).
- `domain/error.rs`: the typed errors a client tells apart, with their `kind` and exit code.
- `domain/include.rs`: the copy list: which paths `.worktreeinclude` makes copyable, and the
  rule that a path never escapes the worktree.
- `domain/contract.rs`: the `contract` and `version` integers every `--json` output carries.

### Domain types

Values that would otherwise travel as a bare `String`, `PathBuf` or `bool` get a type of their
own, so a mix-up does not compile and a value is checked once, where it enters ("parse, don't
validate"):

- `WorktreeName` (`domain/worktree.rs`): a name `worktree new` accepts. It can only be built by
  parsing (`FromStr`), which clap does on the argument, so an invalid name is refused before
  anything runs, with a `WorktreeNameError` naming the rule it breaks.
- `Target` (`domain/worktree.rs`): what `worktree rm` is given, a `Name` or a `Path` (it contains
  `/` or is `.` or `..`), parsed by clap.
- `Removal` (`domain/worktree.rs`): `Safe` or `Forced`, the `--force` flag from the edge down to
  `git worktree remove`.
- `BranchOutcome` and `KeptReason` (`domain/worktree.rs`): what `worktree rm` did with the
  branch, and why it kept it.
- `Branch` and `StartPoint` (`domain/branch.rs`): a local branch's name, and the default branch a
  new worktree starts from, `Local` or `Remote` (on `origin`). `BranchNameError` names the git
  rule a branch name breaks.
- `PrimaryCheckout` (`domain/fleet.rs`): the primary checkout's path and branch. The git adapter
  and the config sources take it where they need the primary checkout, so they cannot be handed a
  worktree's path by mistake.
- `State` (`domain/state.rs`): `NoAgent`, `Idle`, `SettingUp`, `NotReady(NotReadyReason)` or
  `Broken(BrokenReason)`. A ready worktree is `Idle` when an agent is recorded (whether or not
  its holder is alive) with no turn reported yet, and `NoAgent` otherwise. `NotReadyReason` is
  `Interrupted` (a setup recorded as running whose lock nobody holds) or `Failed` with its `Exit`
  and the last line of its log, or `NotApproved` with the worktree's name (a committed script not
  approved, so it did not run). `BrokenReason` says the directory is missing, git marks it
  prunable, or the state file cannot be read. Broken wins over every other state.
- `AdminDir` (`domain/state.rs`): the directory holding a worktree's state: its `worktree.json`,
  its `setup.log`, its `lock`, and the holder's files `agent.lock`, `agent.json`, `agent.sock` and
  `claude-settings.json`, the hooks' `status.json`, `turns.jsonl`, `hook.lock` and `hook.log`,
  and the person's `seen.json`, and the notes' `notes.json`, `notes.lock` and `notes/` (the
  contents kept for notes on lines), and `snapshot.index`, the throwaway index a snapshot is taken
  through. `worktree_id` is the name of git's admin directory it sits in, which names the chain.
- `Agent`, `HolderStatus`, `HolderRecord`, `Size` (`domain/agent.rs`): the agent kind, whether a
  holder is alive (`Alive`/`Gone`, from the probe of `agent.lock`), what `agent.json` holds (pid
  and socket path, volatile), and a terminal size.
- `StartOutcome`, `Started`, `StopOutcome`, `Stopped`, `AttachEnd` (`domain/agent.rs`): what the
  agent commands did and report (`Started` names the session it `resumed`, if any).
- `StartMode` (`domain/agent.rs`): `Resume` or `Fresh`, the `--fresh` flag of `agent start` from
  the edge down to the op.
- `SessionId` (`domain/launch.rs`): a session id safe on claude's command line: 1 to 128 ASCII
  letters, digits, `_` and `-`, not starting with `-`, so a recorded id never reads as a flag.
  `Resume` pairs it with the transcript a cold start checks.
- `Address` (`domain/nvim.rs`): a Neovim's server address, an absolute path, parsed by clap for
  `nvim register|unregister` (a relative one is refused as a usage error).
- `Frame`, `Decoder`, `Scrollback` (`domain/protocol.rs`): the holder protocol's messages, their
  decoder and the replay buffer.
- `NsId` (`domain/protocol.rs`): a network namespace, parsed from a `/proc/<pid>/ns/net` link
  (`net:[<number>]`).
- `NotePath`, `LineRange`, `Anchor` and `BlobId` (`domain/note.rs`): a note's path (relative to
  the worktree, `/`-separated, no `..`), a range of two or more lines (`S-E`, parsed by clap for
  `--lines`), where a note is attached (`Line`, `Lines` or `File`), and the git blob id naming a
  kept content. `Note`, `Notes` and `PendingNote` are a note as recorded, every pending note of a
  worktree, and a note with its anchor translated to the file now; `Noted`, `Listed`, `Unnoted`
  and `NotesSent` the reports of the note commands.
- `Platform` (`domain/sandbox.rs`): macOS, Linux or other, which decides the sandbox tools.
- `LockStatus` (`domain/state.rs`): `Held` or `Free`, what the read-only probe of a worktree's
  lock found.
- `SetupRecord` and `Exit` (`domain/setup.rs`): what `worktree.json` records about the setup,
  `Running`, `Succeeded` or `Failed(Exit)`, where `Exit` is a non-zero `Code` or a `Signal`, or
  `NotApproved` (`{"status": "not_approved"}`).
- `Trust`, `Approval`, `Kind`, `Subject`, `Unapproved`, `Consent` and `Gate`
  (`domain/trust.rs`): every approval of a repository, one approval and what it covers (`Setup`,
  `Sandbox`), the script or the list (`Subject`), what a committed file asks for not approved yet
  (the worktree's name, the subject, its file), how the person may approve (`Given`, `Ask`,
  `Unavailable`) and what to do (`Run`, `ApproveAndRun`, `Ask`, `Refuse`). `ops::Approval` is
  `Consent` with the function that asks.
- `SetupOutcome`, `SetupReport` and `SetupDone` (`domain/setup.rs`): what running the setup did
  (`NoScript`, `Succeeded`, `Failed`), the report of a command that ran it (the script and its
  source, the log, the state it left), and what `worktree setup` did.
- `Bounds`, `Ending`, `Identity` and `ChainStep` (`domain/snapshot.rs`): what one snapshot
  bounds (the turn it ends, with whether that end was observed, and the turn it begins), a git
  author or committer, and the parent and the expected old value of the ref for the next
  snapshot. `Pruned` (`domain/fleet.rs`): what `worktree prune` cleaned, worktrees and chains.
- `History`, `LoggedTurn`, `Progress` (`Ended`, `Open` or `Unended`), `DiffBounds` and
  `TurnDiff` (`domain/history.rs`): what `turn ls` and `turn diff` found; `Blame`, `BlamedLine`,
  `LineAuthor` and `Source` (`Turn`, `BetweenTurns` or `SinceSnapshot`, `domain/blame.rs`): what
  `turn blame` found.
- `Denial` (`domain/turn.rs`): a tool call the agent's permission settings denied, its tool and
  its text; `Detail`, `SetupDetail`, `SetupProgress` and `Drift` (`domain/detail.rs`): what
  `worktree detail` found, the latest setup and where it is, how far `HEAD` is ahead of and
  behind the local default branch.
- `TypedError` (`domain/error.rs`): the failures a client tells apart: `Busy` (with a
  `BusyReason`: the lock is held, or the agent is running), `NoSandbox`, `Dirty` (a worktree
  `worktree rm` keeps because it has modified or untracked files), `SessionGone` and `NoHolder`
  (a worktree whose agent is not running, so `note send` has nowhere to paste: the client can offer
  to start it), `Prerequisite` (a program the command needs is missing or too old, as the
  doctor's lines say) and `Untrusted` (a setup script or writable list from a committed
  `orca-term.yaml` not approved, with its `Subject` and file, which the client shows before
  approving it).
- `WorktreeLock` (`adapters/fs.rs`): proof that this process holds a worktree's lock. It can
  only be made by taking the lock, and dropping it releases the lock. `FileLock` is the same for
  the holder's `agent.lock`.
- `DetachKey` (`adapters/terminal.rs`): whether Ctrl-\ detaches an attach, or nothing does.

## Invariants

- **The domain is pure.** Nothing in `src/domain/` touches the file system, starts a process,
  reads the environment or prints. It takes strings, bytes and parsed values, and returns values,
  decisions or errors. That is why it is unit-tested without a repository.
- **Only adapters touch the outside world.** Only `adapters/git.rs` starts git, only
  `adapters/script.rs` starts the setup script, only `adapters/holder.rs` starts the holder (and
  only `adapters/holder/process.rs` starts an agent, on a PTY), only `adapters/program.rs` runs a
  program for its version, only `adapters/fs.rs` reads or writes files, and only
  `adapters/env.rs` reads the environment.
- **Only `output` prints**, and only `output/json.rs` knows the JSON shape of the contract.
  Domain types never derive `Serialize` for output. A setup's live echo is a writer `output`
  makes and `cli` hands down, so ops and adapters never name stdout or stderr. Two adapters move
  bytes through stdout without printing a report: the holder writes its one-line first word on
  the pipe `agent start` gave it (then points stdout at `/dev/null`), and `adapters/terminal.rs`
  writes the agent's screen to the attaching terminal.
- **Each command is one ops module**: a short sequence of adapter and domain calls, readable top
  to bottom. Shared steps go in `ops.rs`.
- **Dependencies point down**: main > edge > ops > adapters > domain. A module uses its own layer
  or a lower one, through `crate::` paths, never a higher one.
- **Nothing writes on a read path**: `worktree ls` and `worktree detail` only read (`detail`
  runs git for the drift and fetches nothing). It probes a worktree's lock with
  a non-blocking try that it releases at once, reads the setup log only for a failed setup, and
  never rewrites an interrupted setup (ADR 0003: volatile facts are checked on read).
- **Writes to a worktree's state hold its lock.** The functions that write `worktree.json`
  (`ops::write_state`, `ops::write_agent`) take a `&WorktreeLock`, so a write without the lock
  does not compile, and each keeps what the other records: a setup never erases the agent.
  `worktree ls` reads a setup recorded as running once more when its lock is free, since the
  setup records its end before releasing the lock. `worktree new` takes the lock as soon as
  git's worktree exists and holds it until the setup
  ends; `worktree setup` and `worktree rm` take it without waiting and fail as `busy` when it is
  held.
- **A failed setup undoes nothing**: the worktree, its branch and its log stay, and the worktree
  is not ready until `worktree setup` succeeds.
- **A committed setup script runs only once approved** (#58). A script whose winning config
  layer is a committed `orca-term.yaml` (the worktree's own) runs only when the sha256 of its
  exact text is in `<git common dir>/orca-term-trust.json`, or once the person approves it:
  `--approve-setup`, or `y`/`yes` at the prompt, shown only when stdin and stderr are terminals and
  the output is text. Otherwise nothing runs, `worktree.json` records `not_approved` (the
  worktree is not ready, `setup not approved; run orca-term worktree setup <name>`) and the
  command fails as `untrusted`; `worktree new` leaves the worktree in place. A script from
  `.git/orca-term.yaml` or the global config file always runs. The approvals' file is replaced
  whole under its lock and never written over a newer schema; `worktree ls` only reads the record.
  The same holds for a committed `sandbox.writable` list (an approval of its own, `kind:
  "sandbox"`): `agent start`, the only command that launches claude, asks (`widen the sandbox
  with these writable paths? [y/N]`), takes `--approve-sandbox`, or fails as `untrusted` before
  anything is launched or written; an empty list widens nothing and needs no approval.
- **One agent process per worktree, and its pid and socket are never trusted on their own**
  (ADR 0003, ADR 0009). The holder holds an exclusive `flock` on `agent.lock` for its whole life;
  `agent.json` (its pid and socket) and `agent.sock` are believed only while that lock is held,
  so a holder killed outright leaves files behind but no false liveness. `agent start` under the
  worktree's lock reports a live holder instead of starting a second one, and the holder itself
  refuses to run when another holds `agent.lock`. `worktree rm` and `worktree setup` fail as
  `busy` while a holder is alive, naming `agent stop`. `agent start` refuses a worktree that is
  not ready, and the primary checkout.
- **The agent is launched only through `domain/launch.rs`.** The command line is
  `claude --settings <admin>/claude-settings.json --permission-mode acceptEdits [--resume <id>]`,
  in the worktree,
  with the environment of `agent start` minus `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`,
  `CLAUDE_CODE_SESSION_ID` and `CLAUDE_CODE_CHILD_SESSION` (so an orca-term run from inside a
  Claude Code session never starts a child session). The settings turn claude's sandbox on with
  `failIfUnavailable`, `allowUnsandboxedCommands: false` (which also makes the repository's own
  `.claude/settings*.json` unable to loosen it, on claude 2.1.295 or newer, the floor `agent
  start` checks), every weakening key pinned, the network open, writes denied to every worktree's
  `orca-term` admin directory (each one listed, plus the `*` glob), `refs/orca-term` and
  `orca-term.yaml`, `orca-term-trust.json` (the approved setup scripts) and the `orca-term`
  directory (the Neovim registry) in the git common dir,
  pre-created before the launch, through both the sandbox's `denyWrite` and `Edit(...)`
  deny rules; the only configurable part is `sandbox.writable` in `orca-term.yaml`, extra
  writable paths, the highest config layer replacing the lower ones and printed with its source.
  `launch::check_launch` refuses, by whole argument and parsed value, `--dangerously-skip-permissions`,
  `--allow-dangerously-skip-permissions`, `--auto`, any `--permission-mode` but `acceptEdits`,
  a `--resume` whose value is not a `SessionId`, and settings whose `permissions.defaultMode` bypasses, before any holder is started; the
  literals exist in that file alone (`cargo xtask launch-guard`). Without the sandbox's tools on
  PATH (`sandbox-exec`; `bwrap` and `socat`) nothing is launched: the typed `no_sandbox`.
  It checks every argument, the values of flags included, so a forbidden flag hidden as a value
  is still refused.
- **A cold start resumes the current session** (ADR 0009). With no live holder, `agent start`
  launches claude with `--resume <id>` on the session `status.json` records, and a new session
  when none is recorded or the person passes `--fresh`. It never uses `--fork-session`: the
  worktree keeps one current session. Before launching it checks the id (a `SessionId`) and that
  the recorded transcript is an absolute path still on disk; otherwise nothing is launched and it
  fails as the typed `session_gone`, naming the reason and `agent start <name> --fresh`.
- **Hooks report the live status, and never reach claude.** The settings register one
  synchronous handler for SessionStart, UserPromptSubmit, PreToolUse, PermissionRequest,
  PostToolUse, PostToolUseFailure, PostToolBatch, Stop, StopFailure and SessionEnd: `/bin/sh -c '"$0" hook
  claude --admin "$1" "$2" >/dev/null 2>&1; exit 0'` in exec form, with orca-term's absolute path
  and the worktree's admin directory, a timeout of 5 s. It is wrapped because claude reads a
  hook's exit status 2 as a refusal (it rejects the prompt, or keeps working after `Stop`) and
  its stdout on some events as extra context, so whatever orca-term does, even missing or
  crashing, reaches claude as a silent success. Hooks run outside claude's sandbox (measured), so
  they can write the admin directory, while the agent's own Bash cannot (`denyWrite`). In the
  admin directory `status.json` is replaced atomically (it records the current session and its
  `transcript`, the `transcript_path` of the main agent's hooks only, forgotten when the session
  changes), `turns.jsonl` only grows, `hook.lock`
  serialises a worktree's hooks, and `hook.log` stays under 256 KiB. `worktree ls` derives the
  agent state on read: an open turn is `working` or `waiting` while the holder lives and
  `interrupted` once it is gone; otherwise the latest turn's outcome in the current session, or
  `idle` after a fresh session; `--json` carries the `activity`.
- **Turns are bounded by snapshots under a shared ref** (ADR 0006, #38). Only the hook of an
  event that opens or closes a turn takes one, exactly one, and it is both the end of the turn
  closed and the begin of the turn opened. It goes through a throwaway copy of the index, so the
  real index, `HEAD` and the branch are never touched; `commit-tree` never signs. The end of a
  turn is the agent's, a begin alone is the person's (their edits between turns, belonging to no
  turn), so `git blame` over the chain says who wrote each line, and a turn's diff is
  `begin..end`. A turn closed by `Stop` or `StopFailure` has its end observed; an interrupted one
  ends on the snapshot of whatever closed it, `end_observed: false`. The chain lives in
  `refs/orca-term/worktrees/<admin-dir id>`, shared by every worktree so a `git gc` from anywhere
  keeps it; the ref moves by compare-and-swap, before the status is written, and its commit is the
  next parent only when it is the tip `status.json` records (otherwise it is stale and replaced
  by a parentless snapshot). A failed snapshot is logged and the turn recorded without its id.
  Only `worktree rm` (its own worktree's, once removed) and `worktree prune` (orphans whose
  `.git/worktrees/<id>` is gone) delete a chain; nothing rewrites one. `turn ls|diff|blame` only
  read: no lock, no write, no fetch, and the turn log, not a commit's message, says which turn a
  snapshot ends.
- **A denial is never a state** (ADR 0007, #42). claude reports a tool call its permission
  settings denied only in `PostToolBatch`, by the text of the call's `tool_response` (no error
  flag): a denial starts `Permission to use ` and says ` denied`, or says `denied by your
  permission settings` or `deny rule in your permission settings`. The person's own refusal
  (`The user doesn't want to proceed with this tool use.`) is not one, nor is a command the
  sandbox stopped, which is a failed command. Denials are recorded on their turn (`status.json`
  while it is open, the end line of `turns.jsonl` once it ends) and only counted: `worktree ls`
  shows the latest turn's count as `denials`; the state and its derivation ignore them.
- **The unseen mark is the person's, derived on read.** A worktree is unseen when its latest
  ended turn (an open turn whose holder is gone counts as ended) is after `seen_through` in
  `seen.json`, whatever its outcome, or when the person marked it by hand. Only the person's
  commands, `worktree seen`, `unseen` and `review`, write `seen.json`: atomically (write and rename), with
  no lock, the last write winning, and never over a newer schema; a file the core cannot use
  reads as nothing seen and no mark. Hooks never touch it. `worktree ls` derives the mark on
  read, independent of the state.
- **Every write to a worktree's state tells the running Neovims.** After a hook, a setup (as it
  starts and as it ends, or once with no script), `agent start`, `agent stop`, the agent's own
  exit (the holder, once its files are gone), `worktree rm` and every note write (`note add`,
  `edit`, `rm`, `send`), the command calls
  `ops::notify_changed`, which sends every address in the registry the request firing
  `User OrcaTermWorktreeChanged` with the worktree's path, and waits for its answer before hanging
  up: Neovim drops a queued call whose caller has already gone (measured on 0.11 on Linux). All
  Neovims are told at once, one thread each, within 200 ms in all, so one that never accepts,
  reads or answers cannot hold the command up (it misses that change); notifying never fails the
  write before it. A Neovim that is gone has its file pruned; any other failure
  is skipped and the file kept. `worktree ls` sends nothing.
- **Notes are the person's, kept until sent.** Only the note commands write `notes.json`, each
  under `notes.lock` (waiting at most 2 s for another), replacing it whole; reading takes no lock.
  Ids are never reused and every edit bumps `rev`. A note on lines keeps a copy of its file's
  content under `notes/<blob id>`, written before the note under the same lock and removed by the
  first change after no note refers to it. `note send` reads the notes once, types the prompt into
  the agent through its holder, and only once both writes went through consumes the notes it sent
  whose `rev` is unchanged; it never holds the lock while typing, so a note edited meanwhile is
  kept, as in Orca. With no live holder nothing is typed and nothing consumed (`no_holder`).
  Translation never writes, and `worktree ls` only reads `notes.json` to count the pending notes.
- **The holder is agent-agnostic**: it runs the command line it is given, in the directory it is
  given, and knows nothing of claude. Building that command line is domain code.
- **On Linux, the holder serves only clients from its own network namespace** (#61). claude's
  sandbox runs commands in a network namespace of their own (bwrap `--unshare-net`), and without
  its optional seccomp helper nothing else stops a sandboxed command from connecting to another
  worktree's `agent.sock`. So the holder compares each client's namespace with its own, and
  closes a client from another namespace, or one whose namespace it cannot read, before writing
  it any replay or reading any of its input; it says nothing to the client, keeps serving the
  others, and the agent is untouched. `agent attach`, `agent stop`, `note send` and Neovim run
  in the person's own namespace and are served as before. macOS needs no check: Seatbelt blocks
  unix sockets.
- **Errors** are `anyhow` errors with context, written for a person. Domain parsers return small
  `thiserror` enums (`WorktreeNameError`, `BranchNameError`) whose messages a person can read.
  Failures the client must tell apart are `TypedError` cases (`domain/error.rs`), carried inside
  the `anyhow` error and found again by downcasting. Without `--json` an error is printed on
  stderr as `orca-term: <message>`; with `--json` it is a JSON document on stdout:
  `{"contract": 1, "version": "...", "error": {"kind": "...", "message": "..."}}`, where `kind` is
  the typed error's (`busy`, `session_gone`, `no_sandbox`, `dirty`, `no_holder`, `untrusted`,
  `prerequisite`) or `other`. The error of `untrusted` also carries `what` (`setup` or
  `sandbox`), the `script` or the `writable` paths as written, and `source` (the committed file).
- **Exit codes**: 0 on success; 1 for an untyped failure, and for `worktree new` and
  `worktree setup` when the setup failed (the worktree exists and the normal report says it is
  not ready); 2 for a usage error (clap); 3 for `busy`, a worktree whose lock another process
  holds or whose agent is running; 4 for `session_gone`, a recorded session `agent start`
  cannot resume on a cold start; 5 for `no_sandbox`, a machine where claude's sandbox cannot
  run; 6 for `dirty`, a worktree `worktree rm` refuses without `--force` because it has modified
  or untracked files; 7 for `no_holder`, a worktree `note send` refuses because its agent is not
  running; 8 for `untrusted`, a setup script or writable list from a committed `orca-term.yaml`
  not approved, so `worktree new` or `worktree setup` did not run the script, or `agent start` did
  not start; 9 for `prerequisite`, a program the command needs (git, claude) that is missing or
  too old, and for `doctor` when any check is.

## Known limits of the agent holder

- **The Linux peer check is network namespace equality, nothing finer.** A client is served
  when it shares the holder's network namespace, so a client run from another one on purpose
  (inside a container, under `unshare --net`, from another agent's sandbox) is refused too, with
  no message: it simply sees the connection close. A sandbox that kept the host's network
  namespace would not be told apart. The namespace is read from the client's pid as the kernel
  recorded it when it connected: a client that hands its connection to a child and exits leaves
  a pid that could be reused before the holder reads it.
- **The person's own `~/.claude/settings.json` can widen the sandbox**: array keys such as
  `sandbox.excludedCommands`, `sandbox.filesystem.allowWrite`, `permissions.additionalDirectories`
  and `Edit(...)` allow rules merge across scopes, and hooks, MCP servers and status lines run
  unsandboxed. This is accepted as the person's own configuration; the repository's files cannot
  do the same. Managed (organisation) settings override ours.
- **On Linux, `denyWrite` names only the admin directories that exist at launch**: the `*` glob
  is silently skipped there, so a worktree created after the launch is not covered until the
  agent is started again (macOS honours the glob).
- **`git push` over SSH fails inside claude's sandbox on macOS**, even with the network open;
  HTTPS works. The person pushes from outside, or uses HTTPS.
- **A replay may miss early terminal modes**: the scrollback is raw bytes, so a truncated replay
  can start mid escape sequence, and a mode switched on before the kept bytes (bracketed paste,
  mouse, cursor visibility) is not replayed; the jiggle redraws the screen, not the modes. On
  detach the client resets the usual modes.
- **Paths with glob characters** (`*`, `?`, `[`) in the repository's path are read as globs by
  claude's `denyWrite` on macOS.
- **The real sandbox boundary is not exercised by the tests**: a fake claude enforces nothing.
  Seam 1 asserts the generated settings and command line; the boundary with the real claude waits
  for the live checks of #44.
- **Esc fires no hook**, nor does Esc on a permission dialog or declining a question, so the turn
  shows `working` or `waiting` until the next prompt, the session's end or the holder's death.
- **Hooks can be silenced**: a person's or repository's `disableAllHooks: true`, or managed
  `allowManagedHooksOnly`, leaves the status where it was.
- **An approved long command reads `waiting`** until it finishes, since claude reports no
  approval before the tool's end.
- **The hook path is baked at launch**: moving the orca-term binary stops the status until the
  agent is started again.
- **A full socket backlog reads as gone on macOS**: a Neovim suspended through about 128 events
  refuses the connection, so its address is pruned; it updates again once it regains focus with
  the sidebar open, which registers it again.
- **A holder killed outright notifies nobody**: the sidebar shows the change on the next focus.
- **A Neovim that does not answer within 200 ms misses that change** (suspended, or busy in a
  blocking command): the core hangs up and Neovim drops the call. The next change or focus reads
  the fleet again. Every hook of a worktree then waits those 200 ms.
- **Viewing the agent's terminal does not clear the unseen mark**, unlike Orca: only `u` and
  opening the review ([#39](https://github.com/marivaldo/orca-term/issues/39)) do, because
  Unseen means "not reviewed" (`CONTEXT.md`), and watching an agent is not reviewing its work.
- **`•` is ambiguous-width**: with `ambiwidth=double` a marked row is one cell wider.
- **A `status.json` written before #66 records no transcript**, so the first cold start of such
  a worktree fails as `session_gone` and asks for a fresh session.
- **A transcript present but unusable to claude** passes the check, and claude exits at once: the
  terminal shows claude's own message and the attach ends.
- **Line translation is against the content kept when the note was made** until turns are
  bounded by snapshots (#38): not blame over the chain of turn snapshots. A line edited, even
  trivially, counts as gone, and a note goes as the whole file.
- **`note send` does not wait for the agent's prompt**: Orca waits for its TUI to be idle and
  sendable before it pastes; the core pastes at once, so a prompt sent while claude works is
  queued by claude, and one sent while a dialog is open lands in the dialog. Keys the person types
  in an attached terminal during the 50 ms between the paste and the Enter land between them.
- **A send whose notes could not be consumed after the paste** (another note command holding
  `notes.lock` for more than 2 s) reports the error and leaves the notes pending: sending again
  sends them twice.
- **Denials depend on claude's words**: `hook::is_denial` matches the text of claude's
  `tool_response`, which claude may reword in any release; the recordings of claude 2.1.303 in
  `tests/fixtures/claude-hooks/tui-deny*` and `tui-sandbox` are its guard, replayed by the unit
  tests of the hook module and by `tests/hooks.rs`. Record them again when the floor moves.
- **Hooks give a lower bound of the denials**: when the person refuses or presses Esc at a
  permission prompt, claude sends no `PostToolBatch` for that batch, so a rule denial in the same
  batch is lost. A turn keeps at most 100 denials, and the count tops out there.
- **A worktree whose directory is gone has no detail beyond its state**: `worktree detail` gives
  its state and reason, and no denials, setup or drift, since git no longer names its admin
  directory or its `HEAD`.
- **A failed turn's whole message is kept up to 4000 characters**, and only in `status.json`, so
  only the latest ended turn's.
- **`git gc` packs the chain's ref into `packed-refs`**, which the sandbox's `denyWrite` on
  `.git/refs/orca-term` does not cover, so once a gc has run an agent could rewrite the chain
  through `packed-refs`.
- **Snapshots store untracked, not-ignored files in the repository's objects**: a file the person
  never meant to commit, if `.gitignore` does not exclude it, lands in an object reachable from
  the chain. The default fetch and push refspecs do not carry `refs/orca-term/`, so it stays
  local unless pushed by hand (`git push --mirror` would).
- **A snapshot adds git time to the hooks that open or close a turn**: `git add -A` on the copied
  index costs ~0.13 s on a 40k-file tree, warm (measured in #15); the other hooks run no git.
- **Edits made during a turn, by anyone, belong to it**: nothing tells the person's saves apart
  from the agent's while a turn is open, and an interrupted turn's end snapshot may also hold
  edits made after the interruption.
- **A status that fails to write after the ref moved restarts the chain**: the ref then does not
  match the recorded tip, so the next snapshot is parentless and replaces it, and blame over the
  older snapshots is lost.
- **A turn recorded before snapshots reads `end_observed: false`** and has no snapshot ids.
- **claude refuses a resume whose transcript is gone**: measured on claude 2.1.301, it exits at
  once saying no session was found with that id, which is why the core checks the transcript
  first and reports `session_gone` instead.

## Guards

Each invariant above is checked, not just written down (see `docs/harness.md`):

- Clippy `disallowed-methods` and `disallowed-types` (`clippy.toml`) reject processes, file
  access and environment reads everywhere; each adapter opts out with a module-level
  `#![expect(..., reason = "...")]`.
- `cargo xtask layers` rejects an upward `crate::` reference, a file whose first line is not a
  `//!` naming its layer (`//! Domain: ...`, `//! Adapter: ...`, `//! Ops: ...`, `//! Edge: ...`,
  `//! Main: ...`), and the Clippy opt-out outside adapters. Its table of modules and layers is
  `MODULES` in `xtask/src/layers.rs`; a new top-level module is added there on purpose.
- `cargo xtask launch-guard` (`xtask/src/launch_guard.rs`) rejects the literals
  `--dangerously-skip-permissions`, `--allow-dangerously-skip-permissions`, `--auto` and
  `bypassPermissions`, as whole tokens outside comments, in every file under `src/`, `lua/` and
  `plugin/` but `src/domain/launch.rs`, where the run-time guard lists them.
- Clippy `print_stdout` and `print_stderr` keep printing in `output`.
- The compiler keeps writes to a worktree's state under its lock: `ops::write_state` and
  `ops::write_agent` take a `&WorktreeLock`, which only taking the lock can make.

## Tests

- **Seam 1, black box**: `tests/worktree_*.rs` and `tests/agent.rs` run the binary against
  throwaway git repositories built by `tests/support/mod.rs`, and check what a person or the
  client sees. `tests/agent.rs` runs it with `PATH` set to one directory
  (`tests/support/agent.rs`) holding a fake `claude` (a bash script that records its arguments,
  directory and environment variable names, echoes what is typed and exits on request), symlinks
  to the real `git` and `bash`, and fake `sandbox-exec`, `bwrap` and `socat`; attaches over
  pipes, where the end of stdin detaches; and stops every holder when a test ends. It needs no
  claude, no sandbox and no network. On Linux only, it also attaches from another network
  namespace, through the real `unshare --user --map-root-user --net` or `bwrap --unshare-net`,
  to check the holder refuses it; where neither can make a namespace, that test says why and
  skips (CI's `rust` job on Ubuntu lifts AppArmor's restriction on unprivileged user namespaces
  so it runs). `tests/notes.rs` drives the note commands with the same
  fixture, its fake claude taking its terminal raw to record the exact bytes a send types.
  `tests/snapshots.rs` and `tests/turns.rs` build turns by firing the real hooks as claude does
  (`tests/support/turns.rs`), then ask git, and the turn commands, what the snapshots hold.
  `tests/setup_approval.rs` drives the approval of a committed setup script, and
  `tests/sandbox_approval.rs` that of a committed writable list with the fake claude, without a
  terminal and, for the prompt, on a real pseudo-terminal (`Repo::pty_command` and
  `support::run_at_terminal`, through `pty-process`).
  `tests/notify.rs` registers fake Neovims (unix sockets the
  test listens on) and checks every write notifies them, a stale address is pruned, a Neovim that
  never reads holds nothing up, and `worktree ls` sends nothing.
- **Unit tests** sit next to the pure code they test, in a `#[cfg(test)] mod tests` at the end of
  each domain and edge module. Adapters keep the few tests that need a real directory.
- **Seam 2, the Neovim client against the real core**: `spec/*_spec.lua` run busted via nlua in
  headless Neovim against the binary `cargo build` makes, in throwaway git repositories built by
  `spec/support.lua`. The sidebar's text and highlights are compared with
  `spec/golden/sidebar.txt` (`UPDATE_GOLDEN=1 just lua-test` rewrites it). The person's answers
  and the notifications go through `support.stub_ui` (stubs of `vim.ui.input`, `vim.ui.select` and
  `vim.notify`), and a command that runs a setup is awaited with `vim.wait`. `spec/live_spec.lua`
  starts child Neovims to check the registration, the live updates across tabs and Neovims, a
  stale address and a burst of events; `spec/live_store_spec.lua` starts one to check that the
  live fleet hands one read to every subscriber. `spec/strip_spec.lua` compares the status strip,
  as a statusline expression and as Neovim draws it, with `spec/golden/strip.txt`, and starts
  child Neovims to check its live update and its lualine component, with lualine.nvim pinned in
  `.rocks/lualine.nvim` by `just lua-rocks`. `spec/review_spec.lua` opens real reviews in
  diffview.nvim, pinned in `.rocks/diffview.nvim` by `just lua-rocks` and added to the runtimepath
  for that file only, which closes every view and review tab it opens; `spec/notes_spec.lua` does
  the same to add, draw, list, send, edit and delete notes there, its fake claude recording every
  line it reads (`support.claude_input`); `spec/turn_review_spec.lua` the same to review single
  turns, made by running the real `orca-term hook claude` with claude's payloads on stdin.
  `spec/detail_spec.lua` opens the detail float with `K` on
  worktrees whose denials are replayed from a recording, whose setup failed and whose branch
  drifted, and fires `CursorMoved` itself, since headless Neovim runs no main loop between a spec's
  calls. See "Neovim client" below.
- **Lints** have their own unit tests in `xtask/src/`.

## Neovim client

The Neovim client is Lua at the repository root, beside the crate (ADR 0008), laid out as a
Neovim plugin: Neovim runs `plugin/` at startup and `require` finds the modules in `lua/`. It needs
Neovim 0.11 or later. The client is thin: it runs the core's commands with `--json` and shows
what they print.

- `plugin/orca-term.lua`: defines the `:OrcaTerm <subcommand>` command (`sidebar`, `jump`,
  `review`, `notes`, `turns`, `worktree N`), with completion of the subcommands, and the
  `<Plug>(OrcaTermSidebar)`, `<Plug>(OrcaTermJump)`, `<Plug>(OrcaTermReview)`,
  `<Plug>(OrcaTermNotes)`, `<Plug>(OrcaTermTurns)` and `<Plug>(OrcaTermWorktree1)` to
  `<Plug>(OrcaTermWorktree9)` maps, and nothing else.
- `lua/orca-term/init.lua`: `setup(opts)` and `config`, the settings (so far `binary`, the path to
  the core binary). Calling setup() is optional; `config` holds the defaults until then.
- `lua/orca-term/core.lua`: the link to the core. `find_binary` takes `binary` from setup(), else
  `target/release/orca-term` or `target/debug/orca-term` in the repository the client is loaded
  from, else `orca-term` on PATH. Each command runs the binary with `--json`, decodes the JSON
  and refuses a document whose `contract` differs from the client's own `CONTRACT`.
  `worktree_ls` waits for it (10 s at most); `worktree_new`, `worktree_setup` (with
  `--approve-setup` when asked to approve) and `worktree_rm`
  (which may run a setup for minutes) do not: `vim.system` runs them with no time limit and their
  result reaches a callback through `vim.schedule`. Each gives the document, or `nil` and an error
  `{ kind, message }` whose message names the fix (`kind` is the core's `error.kind`, or the
  client's `no_binary`, `contract`, `usage` for arguments the core refused, or `failed`); it never
  throws. An `untrusted` error also carries the core's `script` and `source`. `worktree_ls_async` reads the fleet without waiting (10 s at most), for the core's
  events. `worktree_seen` and `worktree_unseen` clear and set the unseen mark without waiting;
  `worktree_review(target, cwd, on_done)` runs `worktree review` without waiting (10 s at most),
  handing over the worktree (now seen), the review's `base` and the branch it was found `against`.
  The note commands run without waiting (10 s at most): `note_add(target, path, lines, body, cwd,
  on_done)` (`lines` the first and the last line, equal for one line, `nil` for the whole file; the
  body passed as `--body=<body>`), `note_ls`, `note_edit`, `note_rm` and `note_send(target, ids,
  cwd, on_done)` (every pending note when `ids` is empty), typed as `orca_term.Note` (its `anchor`
  as made and its `current` one, the line in the file on disk now) and one class per document; a
  send with no running agent fails as `no_holder`. Every worktree carries `notes`, its pending count,
  and `denials`, its latest turn's count. `worktree_detail(target, cwd, on_done)` runs `worktree
  detail` without waiting (10 s at most), typed as `orca_term.Detail`: the worktree, its whole
  `detail`, its `denials` (`orca_term.Denial`), its `setup` (`orca_term.SetupDetail`) and its
  `drift` (`orca_term.Drift`). `doctor()` runs `doctor` in Neovim's current directory and waits
  for it (10 s at most), for `:checkhealth orca-term` only, typed as `orca_term.Report`: a report
  with a missing or too old program is returned whole, its `error` inside, rather than as an
  error. `turn_ls(target, cwd, on_done)` runs `turn ls` without waiting (10 s at most), typed as
  `orca_term.TurnList`: the worktree's `name` and `path` and its `turns` (`orca_term.Turn`:
  `number`, `outcome` or, while open, `phase`, `prompt`, `end_observed`, the `begin` and `end`
  snapshots, ...), oldest first; it is the turn review's only source.
- `lua/orca-term/health.lua`: `:checkhealth orca-term`, through `vim.health`: the binary
  `core.find_binary` resolves (or its error), each check of `core.doctor()` as ok, warn, error
  (`missing`, `too_old`) or info, the contract (a core speaking another one is refused by
  `core.lua`), Neovim 0.11 or later, and diffview.nvim through `review.available()` (missing: a
  warning, the review needs it). It only reports.
  `nvim_register` runs without waiting; `nvim_unregister` waits 1 s at most, as it runs
  while Neovim exits. `agent_start(path, size, fresh, approve, cwd, on_done)` (with the terminal's size, `--fresh` when
  `fresh` and `--approve-sandbox` when `approve`) runs without blocking too; its document says which session was `resumed`. `agent_attach` is
  the terminal's job, `agent attach <path> --no-detach-key` started with `jobstart` in the current
  buffer, the one command run without `--json` (no key detaches: Ctrl-\ is Neovim's terminal
  prefix; closing the terminal detaches).
- `lua/orca-term/fleet.lua`: the attention order, pure: each worktree's rank (1 waiting to 8 no
  agent, from the spec) and the sections *Needs you*, *Running* and *Quiet*, sorted by rank then
  name (recency joins when the core prints it).
- `lua/orca-term/render.lua`: the sidebar's look, pure: turns the fleet (or an error), the
  selected worktree's path and a width (36) into lines, highlight spans and the worktree each line
  belongs to. It draws prototype variant D: the header with the primary checkout, the sections, a
  glyph and a second line per worktree, the rounded card around the selected one; an unseen
  worktree gets the mark `render.UNSEEN` (`•`) in the first cell, before the glyph, and a bold
  name. At the right end of a worktree's name line sit its icons, a space apart: the denials of its
  latest turn `⊘N` (`render.DENIED`) in `OrcaTermDenied`, then its pending notes `✎N`
  (`render.NOTE`) in `OrcaTermBadge`, the name cut with `…` to leave them room and a space; a zero
  count shows nothing. No line reaches past the width. `detail(detail, width)` lays out what `worktree
  detail` printed for the detail float, in the same lines and spans: the state glyph and label (in
  the state's colour) and the agent, the branch and its drift (`↑2 ↓1 main`, `up to date with
  main`, nothing without a local default branch), then, a blank line before each and only when it
  has something, the whole detail in the state's colour, `Denied` and one `⊘ <call>` per denial,
  and `Setup (<status>)` (in words: `not approved`) with the end of its log, muted; the detail, the denials and the log wrap
  to the width and are never cut. It also counts the fleet for the status strip: `counts(fleet)` gives the
  worktrees in each state, in `STRIP_ORDER` (the attention order: `waiting`, `failed`,
  `interrupted`, `broken`, `not_ready`, `working`, `setting_up`, `finished`, `idle`, `no_agent`,
  then a state the client does not know yet, by name), leaving out the states no worktree is in,
  and how many are unseen; `strip(fleet)` turns them into items `{ text, group }` such as
  `{ text = '?1', group = 'OrcaTermStateWaiting' }`, the unseen count last as `•N` in
  `OrcaTermUnseen`. The working glyph does not spin there.
- `lua/orca-term/highlights.lua`: the highlight groups (`OrcaTermHeader`, `OrcaTermMuted`,
  `OrcaTermBorder`, `OrcaTermError`, one `OrcaTermState*` per state, `OrcaTermUnseen`,
  `OrcaTermBadge` for a row's icons, `OrcaTermDenied` for the denials and `OrcaTermNote` for the
  notes in a review), defined as default links to `Title`, `Comment`, `FloatBorder` and
  `Diagnostic*` (`OrcaTermUnseen` and `OrcaTermDenied` to `DiagnosticWarn`, `OrcaTermBadge` to
  `DiagnosticHint`, `OrcaTermNote` to `DiagnosticInfo`) so a
  person's own definition wins; `OrcaTermUnseenName`, the unseen worktree's name, is a default of
  its own, only bold. The sidebar, while open, the status strip, once started, and the notes, once a
  review opened, define them again after every `:colorscheme`.
- `lua/orca-term/live.lua`: the live fleet, the fleet last read kept for its subscribers, touching
  no window or buffer. `subscribe(name, on_change)` calls `on_change(fleet, err)` after every read
  that is kept, until `unsubscribe(name)`; subscribing again under a name replaces its function.
  `read()` reads the fleet of the current directory through `core.lua`, waiting for it, and hands
  it to every subscriber; `read_soon()` reads it without waiting `DELAY_MS` (100 ms) later, so a
  burst makes one read, with one read in flight at a time (events during it make one more read
  after it); `fleet()` gives the fleet last kept. A read that ends after a newer one started is
  dropped, and so is every read once nobody is subscribed. Every subscription registers this
  Neovim (`v:servername`, when it is a unix socket path) with the core, from the primary checkout
  of the fleet last kept; it unregisters on `VimLeavePre`, and unsubscribing unregisters nothing.
  `User OrcaTermWorktreeChanged` (`EVENT`) calls `read_soon()`, and `FocusGained` reads the fleet,
  waiting, and registers again; both are ignored while nobody is subscribed. These autocommands
  live in the `orca-term-live` group, made at the first subscription and never deleted.
- `lua/orca-term/strip.lua`: the status strip, the fleet's health for a statusline while the
  sidebar is closed. `statusline()` returns the strip as a statusline expression (each item as
  `%#Group#text%*`, a space between items), for `%{%v:lua.require'orca-term.strip'.statusline()%}`;
  `items()` returns the items, for the lualine component; `counts()` returns the counts, for a
  statusline of one's own. Each returns what was kept from the fleet last read, or the empty
  string (no item, zero counts) when there is no worktree, the fleet could not be read or was not
  read yet; the strip never shows an error. The first call of any of them only schedules the start
  (`vim.schedule`), since a statusline is drawn under textlock: the start defines the highlight
  groups, makes the `orca-term-strip` autocommand group (`ColorScheme` defines them again,
  `DirChanged` of the global or a tab directory reads the fleet again, `read_soon()`), subscribes
  to `live.lua` as `strip`, shows the fleet already kept (when the sidebar read it) and reads it
  again without waiting. Every fleet the live fleet keeps is turned into the strip once, through
  `render.strip`; when what it shows changed, `redrawstatus!` and `redrawtabline` (only while a UI
  is attached: with none, a redraw writes the screen's mode message to the terminal) and, when
  lualine is loaded, `require('lualine').refresh()` draw it. `_reset()` forgets it, for the specs. After a read, when the fleet's primary checkout is not
  the one it registered from, it subscribes again, which registers this Neovim from there. It stays
  subscribed for the rest of the session, open sidebar or not, and touches no window or buffer.
- `lua/lualine/components/orca-term.lua`: the strip as a lualine component, `lualine_x = {
  'orca-term' }`; lualine finds it by name. It extends `lualine.component`: `init` (run again by
  lualine after every `:colorscheme`) defines the highlight groups and creates one lualine
  highlight per strip group with only that group's foreground (links followed), which lualine
  completes with the section's background; `update_status` draws `strip.items()` in them, back to
  the section's colour after each item, and returns the empty string when the strip is empty, so
  lualine hides the component and its separators. The first draw starts the strip through
  `strip.lua`: the live logic is not repeated.
- `lua/orca-term/sidebar.lua`: `:OrcaTerm sidebar`. One scratch buffer, `orca-term://sidebar`,
  shown in a 36-column left split of every tab while open (and of every tab made later). It reads
  the fleet through `live.lua` on open and when an action asks (`refresh()`), and subscribes to it
  while open, so it is drawn again on `FocusGained` and when the core says a worktree changed,
  never polling; it paints what `render.lua` returns, and keeps the selection (a worktree's path) across refreshes;
  on `TabEnter` and on `DirChanged` of a tab, the worktree whose directory is the tab's becomes
  the selection (other tabs leave it alone). Its keys are local to its buffer: `j` and `k` move
  the selection, `<CR>` goes to the tab of the worktree under the cursor (through `tabs.lua`), `a`
  creates a worktree, `u` toggles the unseen mark of the worktree under the cursor, `d` opens its
  review and `.` opens the menu of the worktree under the cursor (through
  `actions.lua`), and `K` shows or hides its detail float (through `detail.lua`, which closing the
  sidebar closes too). It is never shown in a review tab (`review.lua`): `show_in` skips one, also
  while a review opens, and on `User DiffviewViewOpened` (a diffview the person opened by hand,
  where the sidebar showed itself before diffview registered its view) it closes the sidebar's
  window there; diffview split its diff windows from that window, so those get back a text window's
  options. A tab is never left holding only the sidebar: on `QuitPre` for the tab's last
  other window it closes the tab's sidebar first, and on `WinClosed` it closes a tab left with
  only the sidebar (unless it is the last tab). The second line of each worktree shows its
  activity; while a listed worktree is working, one timer turns its glyph through
  `render.SPINNER` by painting the fleet already read again, never reading it, and stops once
  nothing works or the sidebar closes. Opening the sidebar subscribes to `live.lua` after its first
  read, which registers this Neovim from that fleet's primary checkout; closing it unsubscribes, so
  the core's events read nothing for it while it is closed.
- `lua/orca-term/detail.lua`: the detail float, `K` in the sidebar. `toggle(sidebar, worktree,
  cwd)` asks the core for `worktree detail` without waiting and opens a float of `render.detail`'s
  lines in a scratch buffer (`bufhidden=wipe`, highlights in `NAMESPACE`): `relative = 'win'` on the
  sidebar's window, just right of it at the cursor's row, moved up or left only as far as it takes
  to stay in the editor, `border = 'rounded'`, the worktree's name as the title, `style =
  'minimal'`, as wide as its longest line between `MIN_WIDTH` (40) and `MAX_WIDTH` (80) and the room
  right of the sidebar, as high as its lines up to half the editor. It is never entered: the cursor
  stays in the sidebar. From the moment `K` asks, the `orca-term-detail` group closes it (and drops
  the answer still to come) on `CursorMoved` in the sidebar once the cursor left where `K` was
  pressed, on `WinEnter` of any window but the sidebar or the float, and on `WinClosed` of the
  sidebar's window; `K` again, `q` or `<Esc>` in the float and `close()` close it too. There is one
  at most, and an answer for a request closed or replaced meanwhile is dropped; a refusal is told
  with `vim.notify`. `answered()` counts the answers, shown or dropped, for the specs.
- `lua/orca-term/tabs.lua`: the worktrees' tabs. A worktree's tab is the tab whose directory
  (`getcwd(-1, tabnr)`, compared with symlinks resolved) is the worktree's path, found again each
  time, never remembered. A review tab is never a worktree's tab, although diffview makes it from
  the worktree's tab and it inherits that tab-local directory: `find_all` and `is_current` leave
  review tabs out. `go_to` enters it and focuses its main window (not the sidebar), or
  makes it after the last tab with `$tabnew` and `:tcd`; `close_all` closes every tab of a
  worktree after its removal, pointing the last tab at the primary checkout instead.
- `lua/orca-term/terminal.lua`: the agents' terminal buffers. A worktree's terminal is the buffer
  whose `b:orca_term_agent` is the worktree's path, found again each time, never remembered.
  `open` shows it in the current window in terminal mode, reusing a live one or attaching a new
  one through `core.agent_attach`. Closing it only detaches: `:q` hides it, still attached, and
  wiping it ends the attach; the agent runs on in its holder. The first terminal opened adds an
  `ExitPre` autocommand that detaches every agent terminal, so a running attach never makes Neovim
  refuse `:wqa` or `:xa` (E948).
- `lua/orca-term/review.lua`: the review tabs, and the only module that calls diffview.nvim (its
  modules and views typed loosely, as plain tables, since LuaLS knows no types of diffview). A
  review tab is a tab holding a diffview view (`diffview.lib`'s `tabpage_to_view`, asked only once
  diffview is loaded), a tab marked `t:orca_term_review` (the worktree's path, set on the tabs
  orca-term opens), or the current tab while a review is being opened (diffview makes its tab with
  `:tab split` before it registers its view). `open(path, base)`, called from the worktree's tab,
  goes back to the worktree's review tab and refreshes it (`refresh_files`) when one is open, or
  runs `require('diffview').open({ '-C' .. fnameescape(path), base })` (diffview passes `-C`
  through `expand()`, so the path is escaped; spaces need nothing, the arguments being a list) and
  marks the new tab. The first review makes the `orca-term-review` group, whose `User
  DiffviewViewOpened`, `DiffviewViewEnter` and `DiffviewDiffBufWinEnter` put a buffer-local `gf` on
  the current buffer and on the view's file panel in orca-term's review tabs only; diffview sets
  its own `gf` on a diff buffer each time it shows it, just before `DiffviewDiffBufWinEnter`, so
  ours, set on that event, wins. That `gf` takes the file from the view's `infer_cur_file()` (the
  panel's line, or the file the diff windows show), goes to the worktree's tab through `tabs.lua`
  and `:edit`s its absolute path in the current window, or the first that is neither floating, the
  sidebar nor an agent's terminal; when there is none, beside the terminal with `:vsplit`
  ('splitright' picks the side). From the right-hand window (the layout's main window, the
  worktree's file) the cursor lands on the same line. A file not on disk (the change deleted it) is
  told and opens nothing. On a buffer diffview shares with a tab orca-term did not open, the same
  map falls back to diffview's own `gf` there, or to Neovim's elsewhere. `close_all(path)` closes
  the worktree's review tabs, letting diffview close its view and tab. For the notes, `shown()`
  says what orca-term's review in the current tab shows (`nil` in any other tab, a diffview opened
  by hand included): its worktree, the right-hand window when it shows the worktree's file on disk
  (diffview's `LOCAL` revision: the window, the file's own buffer and its relative path), the
  buffers of the base side, the file panel's buffer and the panel's line of each file (from the
  components diffview rendered; a file in a folded directory has none); `file_in_panel()` gives
  the file under the cursor in the panel. Opening a review starts the notes (`notes.start()`) and
  lists them (`notes.refresh()`), and so does going back to one already open.
  A turn review is a review tab of its own kind, marked `t:orca_term_turn` (`{ path, number }`,
  `TURN_MARK`) and never `t:orca_term_review`: `open_turn(path, turn)`, called from the worktree's
  tab, goes to that turn's review when one is open (`find_turn`), or runs `require('diffview').open({
  '-C' .. fnameescape(path), begin .. '..' .. end })` and marks the new tab, so it shows exactly
  `git diff <begin> <end>`. `is_review` counts it (the sidebar and `tabs.lua` leave it out), `gf`
  works there as in the review (its right-hand side is the turn's end snapshot), and `close_all`
  closes it with the worktree's review; but `current()`, `find` and `shown()` ignore it, so `d`
  never goes to it and no note is drawn on it. `reviewed()` gives the worktree of either kind, and
  `turn_buffers()` the buffers of the current turn review, for the `gn` that refuses there.
- `lua/orca-term/notes.lua`: the notes in orca-term's review tabs, with no buffer of their own. It
  asks `review.lua` what the current tab shows, never diffview itself, and keeps each worktree's
  notes as `note ls` last printed them. `draw()` draws them in one namespace (`NAMESPACE`) at their
  `current` anchor, the line in the file on disk now: on the right-hand window's buffer a sign
  `✎` (`render.NOTE`) on a note's line (on a range's first line, `┆` on its others) and its body as
  `virt_lines` below its last line, prefixed `✎ `, or below line 1, prefixed `✎ file: `, for a
  whole-file note; in the file panel `✎N` at the end of each file's line (`virt_text`). All in
  `OrcaTermNote`. It puts buffer-local keys on those buffers and on the base side's: `gn` (normal
  and visual) and `gN`. Every other buffer it marked loses its extmarks and keys, so outside
  orca-term's review tab nothing of the notes is left: it is drawn on `User DiffviewDiffBufRead`,
  `DiffviewDiffBufWinEnter` and `DiffviewViewClosed`, drawn and listed again on
  `DiffviewViewEnter`, and taken off every buffer on `DiffviewViewLeave` (diffview's `tab_leave`,
  where it removes its own keys); the right-hand buffer is the file's own, which `gf` edits in the
  worktree's tab, and diffview removes only the keys it set. The file panel's buffer is watched
  with `nvim_buf_attach`, since diffview replaces its lines on each redraw, which moves the
  extmarks; it is drawn again just after. `refresh()` lists the current review's notes again
  through `note ls` without waiting (a listing overtaken by a newer one is dropped) and draws them:
  after every note action, when a marked buffer is reloaded or written (`FileChangedShellPost`,
  `BufWritePost`), and on every fleet the live fleet keeps, to which it subscribes as `notes` while
  a review tab is current (so the core's `User OrcaTermWorktreeChanged` reaches it, and this Neovim
  is registered). `gn` in the right-hand window adds a note on the cursor's line, in visual mode on
  the selected lines (one line is a line note), in the file panel on the whole file under the
  cursor; on the base side it says notes go on the right-hand side; it refuses a buffer with unsaved
  changes, since the core anchors on the file on disk. The body is asked with `vim.ui.input`
  (`PROMPT`, `Note for the agent: `); an empty or cancelled answer adds nothing. `list(path)`
  (`gN`, and `:OrcaTerm notes` through `actions.lua`) asks with `vim.ui.select`: `Send all (N)`,
  then each note as `path:N  body` (`path:S-E`, `path`; the body on one line, cut to 60 cells),
  which asks `Send`, `Edit` (the body offered as `default`), `Delete` (no confirmation, as Orca) or
  `Cancel`. With no pending notes it says so. Sending tells `sent N notes to <worktree>`; a
  `no_holder` refusal says to press `<CR>` on the worktree in the sidebar to start its agent. After
  every change the notes are listed again and the sidebar reads the fleet again (its `✎N`).
  `_reset()` forgets it all, for the specs. In a turn review it draws nothing and lists nothing: its
  buffers get only a `gn` (normal and visual) that says `notes go on the worktree's files: add them
  from its review (d)`, taken off with the other keys.
- `lua/orca-term/actions.lua`: what the person does to worktrees, asked through `vim.ui.input` and
  `vim.ui.select` and told through `vim.notify`: `open` (`<CR>`: the worktree's tab and, when it
  has an agent, `agent start` without blocking, resuming the recorded session on a cold start,
  then its terminal through `terminal.lua`; a `session_gone` refusal asks `Cannot resume the
  session of <name>: <reason>` with `Start a fresh session` (`agent start --fresh`, then the
  terminal) or `Cancel` (nothing starts); an `untrusted` refusal for the sandbox asks
  `sandbox_prompt` (the worktree, the source, the paths) with `Allow` (`agent start
  --approve-sandbox`, then the terminal) or `Cancel` (`APPROVE_SANDBOX`; nothing starts); any other
  refusal is told with the core's reason), `create` (`a`, asking for the agent after the name),
  `toggle_unseen` (`u`: `worktree seen` when the fleet last read
  says unseen, `worktree unseen` otherwise, then the sidebar is read again; a refusal is told),
  `review` (`d`: without diffview.nvim it only says it is needed, naming its URL, and runs
  nothing; else `worktree review` without blocking, then the sidebar is read again (the mark goes)
  and, unless the core refused, the worktree's tab through `tabs.lua` and the review through
  `review.lua`), `review_current` (`:OrcaTerm review`: the worktree whose tab is current, or
  whose review tab is; anywhere else it says it needs a worktree's tab),
  `notes_current` (`:OrcaTerm notes`: the notes of the same worktree, through `notes.lua`;
  anywhere else it says it needs a worktree's tab or its review),
  `turns` and `turns_current` (`:OrcaTerm turns`: the same worktree's turns from `turn ls`, newest
  first, picked with `vim.ui.select` (`Turns of <worktree>`, each `N  outcome  prompt` by
  `turn_item`, the prompt cut to 60 cells); a turn without both snapshots says `turn N has not
  ended yet` or `turn N has no snapshots` (`unreviewable`) and opens nothing; else the worktree's
  tab through `tabs.lua` and the turn review through `review.lua`, then `turn N of <worktree>`, or,
  for an interrupted turn, that its end was not observed; no turns is said; without diffview.nvim
  it only says it is needed; a turn review counts as its worktree's for all three commands),
  `menu` (`.`: `run_setup`, `copy_path`, `remove`), `jump` (`:OrcaTerm jump`) and `go_to_nth`
  (`:OrcaTerm worktree N`), the last two in the sidebar's order (`render.fleet(...).order`).
  `worktree new` and `worktree setup` run without blocking: the sidebar is read again 500 ms after
  they start (showing *setting up*) and when they finish. When either fails as `untrusted`, a
  `vim.ui.select` (`approval_prompt`: the worktree, the source and the script's lines) asks
  `Run it` or `Cancel` (`APPROVE_SETUP`); `Run it` runs `worktree setup --approve-setup` (by name
  after `a`, by path after `.`) and goes on as the command would have, `Cancel` runs nothing and
  says the worktree is not ready. `remove` always asks first; a
  `dirty` refusal asks once more before `--force`; `busy` is only reported; a removed worktree's
  tabs close, its review tabs included. It touches no window or buffer.

Its rules:

- **Only `core.lua` talks to the core.** It is the only Lua file that starts a process; every
  other module calls it. `cargo xtask client-boundary` refuses a process started anywhere else in
  `lua/` or `plugin/`.
- **Only `sidebar.lua`, `detail.lua`, `tabs.lua`, `terminal.lua`, `review.lua` and `notes.lua`
  touch windows, tabs and buffers; `fleet.lua` and `render.lua` are pure.** The order and the look are plain
  functions of the fleet, tested without a window; the sidebar only paints them, and `actions.lua`
  moves between tabs only through `tabs.lua`, opens terminals only through `terminal.lua`, reviews
  only through `review.lua`, the one module that calls diffview.nvim (`notes.lua` asks it what a
  review shows), and lists notes only through `notes.lua`.
  `cargo xtask client-boundary` refuses a window, tab or buffer call (and a `vim.cmd` that splits
  or runs `tabnew`, `tabedit`, `tabclose`, `tabonly`, `tcd` or `tchdir`) anywhere else, and any
  `vim.api` or `vim.cmd` in the pure modules.
- **A worktree is independent of its tabs.** Opening, closing or moving a tab never changes a
  worktree; only `actions.lua`, through the core, creates, sets up or removes one, and removing
  always asks for confirmation (twice when it would discard changes).
- **The client never computes state.** The fleet, each worktree's state and every decision come
  from the core's JSON; the client renders them. No logic is bound to the Neovim process.
- **The status strip never blocks a statusline and never touches the layout.** Drawing it only
  returns what was kept: it never waits and never starts a process, and its first call only
  schedules the start. Without a UI it never redraws, so it writes nothing to the terminal. Neither requiring nor starting it changes `statusline`, `tabline`,
  `winbar`, `laststatus`, `showtabline` or lualine's config: the person places it.
  `spec/strip_spec.lua` checks both.
- **A review tab is never a worktree's tab, and never shows the sidebar.** A tab holding a
  diffview view, or marked `t:orca_term_review`, is left out by `tabs.lua` and skipped by the
  sidebar, whoever opened it; `spec/review_spec.lua` checks both. A turn review, marked
  `t:orca_term_turn`, is a review tab too, but never the worktree's review: `d` never goes to it;
  `spec/turn_review_spec.lua` checks it.
- **No global keymap.** Every key the client adds is local to one of its buffers, and the
  commands are reached through `:OrcaTerm` and `<Plug>` maps a person binds;
  `spec/plugin_spec.lua` checks that loading the client defines no global keymap.
- **Notes never leak out of the review.** The notes' extmarks and their `gn` and `gN` are only on
  the buffers of orca-term's review tab while it is current: leaving it takes them off every buffer,
  the file's own included, so editing the same file in the worktree's tab has Neovim's own `gn` and
  no note drawn; entering the review puts them back. `spec/notes_spec.lua` checks it.

Its known limits:

- If quitting is refused for another reason after `ExitPre` detached the terminals (a buffer with
  unsaved changes), they stay detached until the next `<CR>` attaches again.
- A person who changes tab while the agent is starting gets no terminal until their next `<CR>`.
- A Neovim whose server is not a unix socket path (no server, or TCP) is never registered: its
  sidebar updates on focus and after actions only.
- The status strip shows the fleet of `getcwd()`: it reads again when the global or a tab's
  directory changes, not on a window's `:lcd` nor when entering a tab whose directory is in
  another repository.
- In a plain statusline, each strip item ends with `%*`, which goes back to the statusline's own
  colour (`StatusLine`), not to a colour set by an item placed before the strip.
- The working glyph does not spin in the strip: it is always `◐`.
- The review does not list files git does not track: it compares a commit with the working tree,
  and diffview lists untracked files only when it compares the index with the working tree.
- A review tab already open is refreshed, not reopened: when the worktree's fork point moved (a
  rebase), it keeps comparing with the base it opened with until it is closed and opened again.
- `gf` is set again on diffview's file panel when the view is entered or a diff window is filled:
  after hiding and showing the panel again (`:DiffviewToggleFiles`), the panel has diffview's own
  `gf` until then.
- Removing a worktree closes its review tabs before the core runs, while the files are still on
  disk (diffview restores their windows as it closes, and Neovim says `E211` for a file that is
  gone). When the core then refuses (a busy worktree, or a cancelled forced removal), the review
  stays closed.
- `:OrcaTerm review` and `:OrcaTerm notes` read the fleet, waiting for it, to find the current
  tab's worktree.
- Notes are drawn only in orca-term's review tabs, on the right-hand side and the file panel: not
  in a diffview opened by hand, not on the base side, not in the worktree's tab. `gn` adds notes
  only on the files the review lists (a file git does not track is not listed); a note on any
  other file, added with `orca-term note add`, is still listed by `gN` and sent.
- While the right-hand buffer has unsaved changes, the notes drawn move with its text, but `gn` is
  refused until it is written; once written (or reloaded), they are drawn where the core says.
- After hiding and showing the file panel again (`:DiffviewToggleFiles`), its `✎N` and keys come
  back on diffview's next event there, as `gf` does.
- `gn` and `gN` take the place of Neovim's own `gn` (select the next match) and `gN` in the
  review's buffers, while the review tab is current.
- In a turn review, `gf` from the right-hand side, the turn's end snapshot, lands on the same line
  number in the file on disk, which later edits may have moved.
- The detail float is as high as half the editor at most: a longer detail shows the rest only once
  the person enters the float (`<C-w>w`) and scrolls. It shows the detail as it was when `K` was
  pressed, and closes when the sidebar is drawn again with the cursor moved.
