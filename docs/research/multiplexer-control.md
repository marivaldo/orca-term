# Research: what tmux and kitty remote control allow an external process to drive

Resolves [#2](https://github.com/marivaldo/orca-term/issues/2). Blocks the root decision on who owns the PTYs.

Facts and sources only. No recommendation: the tmux-vs-kitty choice belongs to the decision this
blocks. Where the primary sources do not answer a question, this document says **not found**
instead of inferring.

## Method and trust level

Three classes of evidence are used, and each claim says which one it rests on:

- **Measured** — run on this machine on 2026-10-07 against tmux 3.7b / kitty 0.47.4. Highest trust.
- **tmux(1)** — the man page shipped with the installed tmux 3.7b,
  `/opt/homebrew/share/man/man1/tmux.1`. This is the authoritative text for the installed version;
  the upstream copy is at <https://github.com/tmux/tmux/blob/master/tmux.1>.
- **kitty docs** — <https://sw.kovidgoyal.net/kitty/>, plus the self-documenting
  `kitten @ <cmd> --help` of the installed 0.47.4 binary and the commented default `kitty.conf`
  that kitty 0.47.4 generated locally.

Third-party web pages and repositories were treated as untrusted data.

## 1. Local environment facts (measured, not assumed)

| Fact | Value | How it was obtained |
| --- | --- | --- |
| tmux | `tmux 3.7b` | `tmux -V` |
| tmux `default-shell` | `/opt/homebrew/bin/fish` | `tmux show-options -g default-shell` |
| tmux `history-limit` | `2000` (the default) | `tmux show-options -g history-limit` |
| kitty | `kitty 0.47.4 created by Kovid Goyal` | `kitty --version` |
| `kitten` on PATH | `/opt/homebrew/bin/kitten` | `command -v kitten` |
| kitty `allow_remote_control` | `yes` (active, uncommented) | `~/.config/kitty/kitty.conf:1601` |
| kitty `listen_on` | **not set** — 0 active lines, only the commented `# listen_on none` at line 1630, so the effective value is the default `none` | `grep -c '^listen_on' ~/.config/kitty/kitty.conf` → `0` |
| kitty `scrollback_lines` | commented out → default `2000` | `~/.config/kitty/kitty.conf:377` |
| `kitten @ ls` from a non-kitty process | **fails**: `Error: open /dev/tty: device not configured` | ran it from this agent's shell |
| nvim | `NVIM v0.12.4`, LuaJIT 2.1.1783773675 | `nvim --version` |
| ghostty | **not installed.** No `ghostty` on PATH; `/Applications/Ghostty.app` does not exist. `/opt/homebrew/Caskroom/ghostty/1.0.0/` exists but contains only a dangling symlink `Ghostty.app -> /Applications/Ghostty.app`, i.e. a stale Homebrew cask record from 2024-12-29. **This contradicts the environment fact stated in issue #2.** | `command -v ghostty`, `ls -d /Applications/Ghostty.app`, `ls -R /opt/homebrew/Caskroom/ghostty/1.0.0` |
| shell | fish 4.8.1 (`/opt/homebrew/Cellar/fish/4.8.1`) | Homebrew Cellar listing |

The `kitten @ ls` failure is the single most consequential local fact. With
`allow_remote_control yes` but `listen_on` unset, kitty listens on **no socket**: the only remote
control channel is the TTY escape-code channel, which requires the controlling process to be running
*inside* a kitty window. An external daemon is not, so it has no way in. See §3.6.

## 2. tmux

### 2.1 Spawn with a stable, addressable id

- tmux assigns ids prefixed `$` (session), `@` (window), `%` (pane). tmux(1), *CLIENTS AND
  SESSIONS*: "These are unique and are unchanged for the life of the session, window or pane in the
  tmux server." The pane id is also handed to the child in the `TMUX_PANE` environment variable.
- `new-session`, `new-window` and `split-window` take `-P` ("prints information after creation") and
  `-F format`, so the id comes back on stdout from the creating call — no list-then-guess race.
- Measured:
  - `tmux -L orcaid new-session -d -s s1 -x 80 -y 24 -P -F '#{session_id} #{window_id} #{pane_id}' /bin/cat` → `$0 @0 %0`
  - `tmux -L orcaid split-window -d -t %0 -P -F '#{pane_id}' /bin/cat` → `%1`
- Measured: the id survives `respawn-pane`. After `respawn-pane -k -t %0 /usr/bin/false` and again
  `respawn-pane -k -t %0 /bin/cat`, the pane was still `%0` with a new `pane_pid`. So the id is a
  slot identity, not a process identity.
- Uniqueness is scoped to "the life of the ... pane **in the tmux server**". tmux(1) does not state
  whether ids are reused after a server restart; `next_session_id` exists as a format but no
  equivalent guarantee is documented. Treat ids as unique only within one server lifetime.
- A dedicated socket isolates the orchestrator from the user's own tmux: `-L name` creates the
  socket in `tmux-UID` under `$TMUX_TMPDIR` or `/tmp`; `-S path` gives a full path. tmux(1),
  *SYNOPSIS*/options. All probes in this document used `-L` and did not touch the user's server.

### 2.2 Reading output — full scrollback

- `capture-pane -p -t <target> -S - -E -` writes the pane contents to stdout. tmux(1): `-S` and `-E`
  "specify the starting and ending line numbers, zero is the first line of the visible pane and
  negative numbers are lines in the history. `-` to `-S` is the start of the history and to `-E` the
  end of the visible pane. The default is to capture only the visible contents of the pane."
- Relevant flags, all from tmux(1) `capture-pane`: `-e` includes SGR escape sequences for text and
  background attributes; `-C` escapes non-printables as octal; `-J` preserves trailing spaces and
  joins wrapped lines (and implies `-T`); `-N` preserves trailing spaces; `-L` prefixes line
  numbers; `-F` emits per-line flags where `O` marks a line as output and `P` as a prompt; `-a` uses
  the alternate screen, in which case "the history is not accessible".
- Measured: with `cat` in the pane and one line sent, `capture-pane -p -t %0 -S -` returned both the
  terminal echo and cat's copy.
- **Bound:** scrollback is capped by `history-limit` ("Set the maximum number of lines held in pane
  history"), which is **2000 on this machine**. Anything older is gone; `capture-pane` cannot
  recover it. `history_size` and `history_limit` are readable as formats.
- The `-a` caveat matters: while a full-screen program (vim, less, a TUI) holds the alternate screen,
  the scrollback is not reachable.

### 2.3 Reading output — incremental

Two documented mechanisms, both measured working.

**(a) `pipe-pane`.** tmux(1): "Pipe output sent by the program in `target-pane` to a shell command or
vice versa. A pane may only be connected to one command at a time, any existing pipe is closed
before `shell-command` is executed." `-O` connects the command's stdin so pane output is piped to it
(and `-O` is the default when neither `-I` nor `-O` is given); `-I` connects stdout so what the
command prints is written to the pane "as if it were typed". `-o` only opens a pipe if none exists.

Measured: `pipe-pane -t %0 -O -o 'cat >> .../pipe.log'`, then `send-keys -t %0 second-incremental-line Enter`,
then reading the log gave:

```
second-incremental-line^M
second-incremental-line^M
```

So it is a live, raw byte stream of what the pane received, CRs and (if present) escape sequences
included — not post-rendered screen text. One pipe per pane is a hard limit.

**(b) Control mode (`tmux -C`).** tmux(1), *CONTROL MODE*: a client sends commands terminated by
newlines on stdin; each produces a `%begin` / `%end` (or `%error`) block, and notifications are
emitted outside those blocks. `%output <pane-id> <value>` means "A window pane produced output",
with `value` escaping "non-printable characters and backslash as octal \\xxx".

Measured, attaching `tmux -C attach` over a FIFO:

```
%begin 1791397492 310 0
%end 1791397492 310 0
%session-changed $0 probe
%begin 1791397493 314 1
@1 %1
%end 1791397493 314 1
%session-window-changed $0 @1
%window-add @1
%window-renamed @1 tmux
%begin 1791397494 318 1
%end 1791397494 318 1
%output %1 [oh-my-zsh] Would you like to update? [Y/n] 
%window-renamed @1 zsh
%session-window-changed $0 @0
%unlinked-window-close @1
%exit
```

That is one process multiplexing spawn (`new-window -P -F` returning `@1 %1` inside the block),
incremental output, and lifecycle events over a single stdin/stdout pair.

Control-mode details that constrain a design, all from tmux(1):
- The full notification set is: `%client-detached`, `%client-session-changed`, `%config-error`,
  `%continue`, `%exit`, `%extended-output`, `%layout-change`, `%message`, `%output`,
  `%pane-mode-changed`, `%paste-buffer-changed`, `%paste-buffer-deleted`, `%pause`,
  `%session-changed`, `%session-renamed`, `%session-window-changed`, `%sessions-changed`,
  `%subscription-changed`, `%unlinked-window-add`, `%unlinked-window-close`,
  `%unlinked-window-renamed`, `%window-add`, `%window-close`, `%window-pane-changed`,
  `%window-renamed`.
- **There is no `%pane-died` or `%pane-exited` notification.** See §2.5.
- `refresh-client -C <width>x<height>` or `-C @<window-id>:<w>x<h>` sets the control client's size.
- `refresh-client -A <pane-id>:off|on|pause|continue` lets the client mute a pane: "if `off`, tmux
  will not send output from the pane to the client and if all clients have turned the pane off, will
  stop reading from the pane". With `pause-after`, `%output` is replaced by
  `%extended-output <pane-id> <age> ... : <value>`, where `age` is how long tmux buffered it.
- `%output` and `refresh-client -B` subscriptions are scoped to the session the control client is
  attached to (`%*` is "all panes in the attached session"). Driving panes across several sessions
  needs one control client per session, or a different channel.

### 2.4 Writing input

- `send-keys -t <target> <key ...>`. tmux(1): "Each argument `key` is the name of the key (such as
  `C-a` or `NPage`) to send; if the string is not recognised as a key, it is sent as a series of
  characters." `-l` "disables key name lookup and processes the keys as literal UTF-8 characters";
  `-H` takes hex ASCII values; `-N` is a repeat count; `-R` resets terminal state.
- Measured: `send-keys -t %0 orca-probe-line Enter` reached the `cat` in the pane and the text came
  back out of `capture-pane`.
- `pipe-pane -I 'cmd'` is a second write path: the command's stdout is written into the pane as if
  typed.
- For large or arbitrary payloads, `send-keys -l` avoids key-name collisions (a literal `Enter` or
  `C-c` in the data would otherwise be interpreted). This is an escaping obligation on the caller,
  not something tmux solves.

### 2.5 Exit detection and status

This is where tmux's control mode and its hook system diverge, and the distinction is load-bearing.

- Formats (tmux(1), *FORMATS*): `pane_dead` — "1 if pane is dead"; `pane_dead_status` — "Exit status
  of process in dead pane"; `pane_dead_signal` — "Exit signal of process in dead pane";
  `pane_dead_time` — "Exit time of process in dead pane".
- The pane must be kept alive to be inspected. `remain-on-exit`: "A pane with this flag set is not
  destroyed when the program running in it exits. If set to `failed`, then only when the program
  exit status is not zero." Without it, the pane (and its ids and scrollback) is gone the moment the
  child exits, taking the status with it.
- Measured, with `remain-on-exit on`:
  - after `respawn-pane -k -t %0 /usr/bin/false`: `pane_id=%0 dead=1 status=1 signal= time=1791397455`
  - after `kill -9` on the pane's pid: `dead=1 status= signal=kill`
  - So normal exit populates `pane_dead_status` and leaves `pane_dead_signal` empty; signal death
    populates `pane_dead_signal` with the symbolic name and leaves the status empty. The two cases
    are distinguishable.
- Hooks (tmux(1), *HOOKS*): `pane-died` "Run when the program running in a pane exits, but
  `remain-on-exit` is on so the pane has not closed"; `pane-exited` "Run when the program running in
  a pane exits".
- **Critical:** tmux(1) states "All the notifications listed in the CONTROL MODE section are hooks
  (without any arguments), except `%exit`." `pane-died` and `pane-exited` are in the *additional*
  hooks list, i.e. they are hooks that have **no** control-mode notification. A plain control-mode
  client is therefore never told a process exited. Two documented ways to bridge the gap, both
  measured working:

  1. **A hook that emits a notification.** `set-hook -g pane-died 'display-message "..."'` surfaces
     as `%message`, which *is* a notification ("A message sent with the `display-message` command").
  2. **A format subscription.** `refresh-client -B name:what:format`: "After a subscription is
     added, changes to the format are reported with the `%subscription-changed` notification, at
     most once a second." `what` may be a pane id, `%*` for all panes in the attached session, a
     window id, or `@*`.

  Measured output of one control-mode client with both wired up, around a `respawn-pane -k -t %0 /usr/bin/false`:

  ```
  %subscription-changed orcasub $0 @0 0 %0 : %0 dead=0 st= sig=
  %message ORCA_PANE_DIED %0 dead=1 status=1 signal=
  %subscription-changed orcasub $0 @0 0 %0 : %0 dead=1 st=1 sig=
  POLLED %0 dead=1 st=1
  ```

  All three routes — hook-to-`%message`, subscription, and explicit `list-panes -F` polling —
  delivered exit status 1 over the single control-mode connection.
- Measured nuance: with `remain-on-exit on`, the `pane-died` hook fired and `pane-exited` did **not**.
  Code must set whichever hook matches its `remain-on-exit` configuration, or set both.
- The subscription path is rate-limited to once a second by design ("at most once a second"). A
  process that starts and exits inside one second may coalesce into a single change event; the hook
  path has no such documented limit. Whether a fast start/exit pair can be missed entirely by a
  subscription is **not found** in tmux(1) — assume it can and use the hook as the authoritative
  edge.

### 2.6 Surviving detach/reattach and reboot

- **Detach/reattach: yes, by design.** tmux(1), *DESCRIPTION*: "Each session is persistent and will
  survive accidental disconnection (such as ssh(1) connection timeout) or intentional detaching
  (with the `C-b d` key strokes)." The tmux server is a separate process from the client; panes keep
  running with no client attached. Everything in §2.1–§2.5 works against a detached session: all the
  measurements above were taken against `new-session -d` with no interactive client.
- A detaching control-mode client is announced to other clients via `%client-detached <client>`, and
  `client-detached` is a hook.
- `pipe-pane` targets the server, not a client, so an output pipe survives client churn.
- tmux(1): "Once all sessions are killed, tmux exits."
- **Reboot: no.** The tmux server is an ordinary user process holding a socket under
  `$TMUX_TMPDIR`/`/tmp`; there is no persistence layer. The word "reboot" does not appear anywhere
  in tmux(1) (`grep -ci reboot` → 0), and no save/restore mechanism is documented. Session
  restoration across a reboot is third-party only (e.g. tmux-resurrect), which was **not evaluated**
  here. Pane ids, scrollback and `pane_dead_*` are all lost with the server.

### 2.7 What the user must enable for tmux

Nothing. No configuration, no opt-in, no permission prompt. `tmux -L <socket> new-session -d ...`
works out of the box, and control mode, `capture-pane`, `pipe-pane`, `send-keys`, hooks and
subscriptions are all core commands. Two defaults are worth overriding deliberately rather than
asking the user to:

- `history-limit 2000` caps retrievable scrollback (set per-session via `set-option history-limit`).
- `remain-on-exit` is off by default, so exit status is unobservable unless the orchestrator turns it
  on for the panes it owns.

Both can be set by the orchestrator on its own session, so they are not user-facing requirements.

## 3. kitty remote control

### 3.1 Spawn with a stable, addressable id

- `kitten @ launch [CMD ...]`. From the installed binary's help: "Prints out the id of the newly
  opened window. Any command line arguments are assumed to be the command line used to run in the
  new window, if none are provided, the default shell is run."
- `--type` selects `window`, `tab`, `os-window`, `overlay`, `overlay-main`, `background`,
  `clipboard`, `primary`, `os-panel`. `background` runs the process with no kitty window at all —
  which also means no pane to read.
- Matching is `--match field:query` with window fields `id, title, pid, cwd, cmdline, num, env, var,
  state, neighbor, session, recent`. "For numeric fields: id, pid, num and recent, the expression is
  interpreted as a number, not a regular expression. Negative values for id match from the highest
  id number down, in particular, -1 is the most recently created window." Window ids are therefore
  monotonically increasing integers within a kitty instance.
- "The window id of the current window is available as the `KITTY_WINDOW_ID` environment variable" —
  kitty's equivalent of `TMUX_PANE`.
- `kitten @ ls` returns "a list of operating system kitty windows. Each OS window has an id and a
  list of tabs. Each tab has its own id, a title and a list of windows. Each window has an id,
  title, current working directory, process id (PID), command-line and environment of the process
  running in the window", as JSON.
- **Uncertainty:** whether kitty window ids are guaranteed stable/non-reused across a kitty restart
  is **not found** in the docs. The `-1` / "highest id number down" semantics imply a counter per
  instance; nothing states what happens to the counter on restart. Since processes do not survive a
  kitty restart anyway (§3.6), this is largely moot.
- **No live verification was possible.** kitty was not running with a reachable control channel on
  this machine (§1), so every kitty claim here rests on the docs and on `--help` of the installed
  0.47.4 binary, not on measurement. This is a real asymmetry in evidence quality against §2.

### 3.2 Reading output — full scrollback

- `kitten @ get-text --match id:<n> --extent all`. From the installed binary's help, `--extent`
  (default `screen`): "The default of `screen` means all text currently on the screen. `all` means
  all the screen+scrollback and `selection` means the currently selected text.
  `first_cmd_output_on_screen` means the output of the first command that was run in the window on
  screen. `last_cmd_output` means the output of the last command that was run in the window.
  `last_visited_cmd_output` means the first command output below the last scrolled position via
  scroll_to_prompt. `last_non_empty_output` is the output from the last command run in the window
  that had some non empty output. **The last four require shell_integration to be enabled.**"
- `--ansi` includes formatting escape codes (default: plain text only). `--add-cursor` appends cursor
  position/style escapes.
- The per-command extents (`last_cmd_output` etc.) are strictly more capable than anything tmux
  offers out of the box — tmux can mark output vs prompt lines with `capture-pane -F` but has no
  "give me the last command's output" primitive. They are also the part that depends on shell
  integration, hence on fish (§4).
- **Bound:** `scrollback_lines`, default `2000`, uncommented nowhere locally. kitty.conf: "Number of
  lines of history to keep in memory for scrolling back. Memory is allocated on demand. Negative
  numbers are (effectively) infinite scrollback."
- `scrollback_pager_history_size` (default `0`, in MB) is a separate, larger buffer, but kitty.conf
  says it is "used only for browsing the scrollback buffer with pager. This separate buffer is not
  available for interactive scrolling". Whether `get-text --extent all` can reach that pager-history
  buffer is **not found** in the docs. Assume it reads only the in-memory `scrollback_lines` buffer
  until proven otherwise.

### 3.3 Reading output — incremental

**Not found.** There is no streaming, tailing or subscription facility in kitty remote control.

- The complete subcommand list of the installed `kitten @` (0.47.4) is: `action, close-tab,
  close-window, create-marker, detach-tab, detach-window, disable-ligatures, env, focus-tab,
  focus-window, get-colors, get-text, goto-layout, kitten, last-used-layout, launch, load-config,
  ls, new-window, remove-marker, resize-os-window, resize-window, run, scroll-window, select-window,
  send-key, send-text, set-background-image, set-background-opacity, set-colors,
  set-enabled-layouts, set-font-size, set-spacing, set-tab-color, set-tab-title, set-user-vars,
  set-window-logo, set-window-title, signal-child`. Nothing streams window output.
- `kitten @ run` is not it: it "Run[s] a program on the computer in which kitty is running and
  get[s] the output", i.e. it forwards the stdout/stderr of a *new* program it starts. It does not
  read an existing window's output.
- The Python watcher API (`launch --watcher`, or the global `watcher` kitty.conf option) has
  callbacks `on_load, on_resize, on_focus_change, on_close, on_set_user_var, on_title_change,
  on_cmd_startstop, on_color_scheme_preference_change, on_tab_bar_dirty, on_quit`. **There is no
  output callback.** `on_cmd_startstop` gives `is_start`, `cmdline` and `time` — command boundaries,
  not bytes, and it requires shell integration.
- So incremental reading against kitty means one of: polling `get-text` and diffing (lossy — a
  2000-line cap plus whatever scrolled past between polls), or not using kitty as the output channel
  at all and wrapping the child so its output is teed somewhere the orchestrator can read. The
  latter means the orchestrator owns the PTY, which changes the root decision.
- This is the sharpest asymmetry found: tmux offers two first-class incremental channels (§2.3),
  kitty offers none.

### 3.4 Writing input

- `kitten @ send-text [TEXT TO SEND]`, with `--match`, `--all`, `--exclude-active`, `--stdin`. From
  the installed binary's help: "The text follows Python escaping rules. So you can use escapes like
  `\e` to send control codes and `⇺` to send Unicode characters."
- `kitten @ send-key` sends "arbitrary key presses to the specified windows" — the named-key path,
  analogous to `send-keys` without `-l`.
- **Caveat, verbatim from the help:** "Note that errors are not reported, for technical reasons, so
  `send-text` always succeeds, even if no text was sent to any window." An external driver gets **no
  delivery confirmation** from `send-text`. tmux's `send-keys` returns a command error for a bad
  target; `send-text` does not. Any write-then-verify loop must verify out of band, e.g. by reading
  the window back with `get-text`.
- `kitten @ signal-child` sends a signal to the foreground process in matched windows.

### 3.5 Exit detection and status

- **At spawn time only, and mutually exclusive with getting the id.** `kitten @ launch
  --wait-for-child-to-exit`: "Wait until the launched program exits and print out its exit code. The
  exit code is printed out instead of the window id. If the program exited normally its exit code is
  printed, which is always greater than or equal to zero. If the program was killed by a signal, the
  symbolic name of the SIGNAL is printed, if available, otherwise the signal number with a leading
  minus sign is printed." `--response-timeout` defaults to `86400` (one day).

  Note "**instead of** the window id". One `launch` call yields either the addressable id or the
  eventual exit status, never both. Getting both requires a second channel: launching with
  `--no-response` is no help, and `ls` can find the window by `--match cmdline:` or by a `--var`
  set at launch — but correlating a *blocking* `--wait-for-child-to-exit` call with an id obtained
  some other way is the caller's problem. No documented single call does both.
- **For a window that already exists: not found.** No `kitten @` subcommand reports the exit status
  of a window's process. There is no kitty analogue of `pane_dead_status`. `ls` reports the live
  process (pid, cmdline), so the only signal that a process ended is that the window stops appearing
  in `ls` — which conflates "exited 0", "exited 1" and "killed".
- Whether the exit status is reachable from a watcher is **not found**: the documented callback list
  has `on_close` ("called when window is closed, typically when the program running in it exits")
  but the docs do not state that the child's exit status is in its `data` dict, and there is no
  `on_child_death` callback.
- Window lifetime around child exit is governed by `close_on_child_death`, default `no`: "With the
  default value `no`, the terminal will remain open when the child exits as long as there are still
  other processes outputting to the terminal (for example disowned or backgrounded processes). When
  enabled with `yes`, the window will close as soon as the child process exits." `launch --hold`
  keeps the window open after the command exits, at a shell prompt. Neither records the status.
- There is no `remain-on-exit` equivalent that preserves an inspectable corpse carrying the exit
  code. This is the second sharp asymmetry against tmux.

### 3.6 Surviving detach/reattach and reboot

- **Detach/reattach: no.** kitty is not a multiplexer. The kitty FAQ states that "terminal
  multiplexers are a bad idea, do not use them, if at all possible. kitty contains features that do
  all of what tmux does, but better, **with the exception of remote persistence**." There is no
  detach/reattach: the child processes are children of the kitty GUI process, and nothing documents
  them outliving it.
- kitty *sessions* are layout, not process persistence. The sessions doc describes a session file as
  capturing "kitty windows, tabs and what programs to run in them as well as how to layout the
  windows" — on restore kitty recreates windows and **re-runs** the command lines. `--use-foreground-process`
  extends what gets *recorded* ("save that process so that when the session is used both the shell
  and the process running inside it are re-started", and it needs shell integration) — note
  "re-started", not resumed. Scrollback, window ids and in-flight process state are not preserved.
- **Reboot: no**, for the same reason, and a fortiori.
- `kitten @ detach-window` / `detach-tab` move a window to a different tab or OS window **within the
  same kitty instance**. Despite the name, this is not tmux-style detach; it does not survive kitty
  exiting.
- Consequence for an external daemon: a long-running job in a kitty window dies when kitty dies, and
  its output buffer dies with it. Any durability (job survives a kitty restart, output replayable
  afterwards) has to live outside kitty.

### 3.7 What the user must enable for kitty, and what breaks

This is the part the issue's own environment facts get wrong, and it is the most actionable finding.

`allow_remote_control` values, verbatim from the kitty 0.47.4 generated kitty.conf:

- `password` — "Remote control requests received over both the TTY device and the socket are
  confirmed based on passwords, see remote_control_password."
- `socket-only` — "Remote control requests received over a socket are accepted unconditionally.
  Requests received over the TTY are denied. See listen_on."
- `socket` — "Remote control requests received over a socket are accepted unconditionally. Requests
  received over the TTY are confirmed based on password."
- `no` — "Remote control is completely disabled." (the default)
- `yes` — "Remote control requests are always accepted."

`allow_remote_control yes` is **necessary but not sufficient** for an external process. There are two
transports, and the config above only authorizes them:

1. **TTY escape-code channel.** `kitten @ --help`: if `--to` is absent and `KITTY_LISTEN_ON` is
   unset, "messages are sent to the controlling terminal for this process, i.e. they will only work
   if this process is run within a kitty window."
2. **Socket.** Requires `listen_on` in kitty.conf or `kitty --listen-on`. kitty.conf: "Listen to the
   specified socket for remote control connections. Note that this will apply to all kitty
   instances. ... For UNIX sockets, such as `unix:${TEMP}/mykitty` or `unix:@mykitty` (on Linux). ...
   If `{kitty_pid}` is present, then it is replaced by the PID of the kitty process, otherwise the
   PID of the kitty process is appended to the value, with a hyphen. For TCP sockets such as
   `tcp:localhost:0` a random port is always used even if a non-zero port number is specified. Note
   that this will be ignored unless `allow_remote_control` is set to either: `yes`, `socket` or
   `socket-only`. **Changing this option by reloading the config is not supported.**"

**On this machine `listen_on` is unset, so its value is the default `none`: there is no socket.**
Measured consequence: `kitten @ ls` run from this agent (not inside a kitty window) failed with
`Error: open /dev/tty: device not configured` — it fell through to the TTY channel and found no
kitty-controlled tty.

So, concretely, what the user must enable:

| Requirement | Status here | What breaks without it |
| --- | --- | --- |
| `allow_remote_control` set to `yes`, `socket` or `socket-only` | **done** (`yes`) | every `kitten @` call is refused |
| `listen_on unix:...` in kitty.conf (or `kitty --listen-on`) | **missing** | an external daemon cannot connect at all; `kitten @` only works from inside a kitty window. This is the blocker. |
| kitty restarted after adding `listen_on` | n/a | "Changing this option by reloading the config is not supported" — a config reload is not enough, kitty must be restarted |
| `shell_integration` (default `enabled`) | default, enabled | `get-text --extent last_cmd_output / first_cmd_output_on_screen / last_visited_cmd_output / last_non_empty_output`, `--cwd last_reported`, `on_cmd_startstop`, and session `--use-foreground-process` all stop working |
| `scrollback_lines` raised above 2000 | default 2000 | scrollback reads silently truncate; kitty.conf notes a change "will only affect newly created windows, not existing ones" |

Two more security-shaped facts worth recording, since `yes` is broad:

- kitty.conf on `allow_remote_control`: "If you turn this on, other programs can control all aspects
  of kitty, including sending text to kitty windows, opening new windows, closing windows, reading
  the content of windows, etc. **Note that this even works over SSH connections.**"
- `remote_control_password` can scope a password to specific commands or glob patterns
  (`remote_control_password "my passphrase" get-colors set-colors focus-window focus-tab`,
  `remote_control_password "my passphrase" set-tab-* resize-*`), or delegate to a custom Python
  checker. Passwords are supplied via `--password`, `--password-file` (default: an `rc-pass` file in
  the kitty config dir), or `--password-env` (default `KITTY_RC_PASSWORD`). With a socket plus
  `socket-only`, no password is needed.
- For a child kitty itself spawns, `launch --type background --allow-remote-control` sets
  `KITTY_LISTEN_ON` "to a dedicated socket pair file descriptor that the process can use for remote
  control" — a path to a privileged helper without opening a filesystem socket. Whether that fd
  survives the helper re-execing or being restarted independently is **not found**.

## 4. Side-by-side

| Capability | tmux 3.7b | kitty 0.47.4 remote control |
| --- | --- | --- |
| Spawn + get stable id in one call | yes — `new-session`/`new-window`/`split-window -P -F '#{pane_id}'` (measured) | yes — `kitten @ launch` prints the window id |
| Id stable for the object's life | yes, "unchanged for the life of the ... pane in the tmux server"; survives `respawn-pane` (measured) | monotonic int per kitty instance; cross-restart reuse **not found** |
| Id visible to the child | `TMUX_PANE` | `KITTY_WINDOW_ID` |
| Full scrollback read | `capture-pane -p -S - -E -`, bounded by `history-limit` (2000 here) | `get-text --extent all`, bounded by `scrollback_lines` (2000 here) |
| Per-command output read | only indirectly (`capture-pane -F` output/prompt line flags) | yes — four `--extent` modes, needs shell integration |
| Incremental output | yes, two ways: `pipe-pane -O` raw byte stream, and control-mode `%output` (both measured) | **not found** — no streaming API; only `get-text` polling |
| Write input | `send-keys` (`-l` literal, `-H` hex), `pipe-pane -I`; errors reported | `send-text` (Python escapes), `send-key`; **errors never reported** |
| Exit detected | yes — `pane-died`/`pane-exited` hooks, `%subscription-changed` on `#{pane_dead}`, or polling (all measured) | at spawn only, via blocking `launch --wait-for-child-to-exit`; for an existing window, **not found** |
| Exit status / signal | yes — `pane_dead_status`, `pane_dead_signal`, `pane_dead_time`; measured `status=1` and `signal=kill` separately. Requires `remain-on-exit`. | only as the return value of `--wait-for-child-to-exit`, **instead of** the window id; signals as symbolic names |
| Exit event reaches a control-mode client natively | **no** — `pane-died`/`pane-exited` are hooks with no notification; needs `display-message` → `%message`, or `refresh-client -B` | n/a (no event channel) |
| Survives client detach | yes, by design — server is a separate process; all measurements ran against a detached session | no — children belong to the kitty GUI process |
| Survives reboot | no; "reboot" absent from tmux(1); third-party only | no |
| Session save/restore | not in tmux(1) | layout + command lines only; restore **re-runs** commands |
| User must enable | **nothing** | `allow_remote_control` (done) **and** `listen_on` (**missing**), plus a kitty restart |
| Verified by measurement here | yes, extensively | no — docs and `--help` only; no reachable control channel |

## 5. fish-specific notes

The shell is fish 4.8.1, which is not POSIX. Everything below is a place where shell glue is
implicated.

**tmux spawns fish by default.** `tmux show-options -g default-shell` → `/opt/homebrew/bin/fish`
(measured). tmux(1): `default-shell` "is used as the login shell for new windows when the
`default-command` option is set to empty", and tmux "tries to set a default value from the first
suitable of the `SHELL` environment variable, the shell returned by getpwuid(3), or /bin/sh". So any
`new-window`/`new-session` with no explicit command starts fish, and then `send-keys` input is
interpreted by fish — its quoting rules, abbreviations and autosuggestions all apply to synthesized
keystrokes. `send-keys -l` avoids tmux-side key-name collisions but does nothing about fish-side
interpretation.

**tmux's own glue is always `/bin/sh`, never fish.** This is the trap. tmux(1):

- `run-shell`: "Execute `shell-command` using **/bin/sh**".
- `if-shell`: "Execute the first `command` if `shell-command` (run with **/bin/sh**) returns success".
- `pipe-pane` takes a `shell-command`, run the same way.
- A single-argument `shell-command` to `new-window` and friends: "`new-window 'vi ~/.tmux.conf'`
  will run `/bin/sh -c 'vi ~/.tmux.conf'`".

So fish syntax (`set -x`, `&&`/`||` differences, `(cmd)` substitution, `$status`) must never be
embedded in a tmux `shell-command`. Conversely, tmux(1) notes that "`new-window`, `new-session`,
`split-window`, `respawn-window` and `respawn-pane` commands allow `shell-command` to be given as
multiple arguments and executed directly (without" a shell). **Passing argv as separate arguments is
the shell-free path**, and the probes in this document used it (`... /bin/cat`, `... /usr/bin/false`)
precisely to avoid shell glue.

**kitty's write/spawn paths are not shell paths.** `kitten @ launch [CMD ...]` takes argv directly.
`send-text` "follows Python escaping rules", not shell rules — `\e`, `⇺` — which is a different
escaping contract from `send-keys`, and a shared abstraction over both has to normalize them.

**kitty shell integration does support fish**, which matters because four `get-text --extent` modes,
`--cwd last_reported` and `on_cmd_startstop` depend on it. The kitty docs say integration covers
"zsh, fish and bash", added in kitty 0.24.0, and that for fish kitty "prepends the integration
script directory path to the `XDG_DATA_DIRS` environment variable" to autoload it — no manual
sourcing. The integration script warns "Update fish to version 3.3.0+ to enable kitty shell
integration"; fish here is 4.8.1, so that floor is met. The specific manual-setup line for fish was
**not found** in the fetched portion of the shell-integration page.

**Both are affected by the orchestrator's own shell.** Anything that shells out from a daemon written
against fish should exec binaries with argv arrays rather than build command strings, since neither
tmux's `/bin/sh` nor kitty's Python escaping matches fish quoting.

## 6. Explicit "not found" and uncertainty

Recorded so the decision does not quietly assume these:

1. **kitty incremental output.** No streaming/subscription API exists in the 0.47.4 command set or
   the docs. Not found.
2. **kitty exit status for an existing window.** No command, and no documented watcher data field.
   Not found.
3. **Exit status in a kitty watcher's `on_close` data dict.** Not documented either way. Not found.
4. **kitty window id reuse across kitty restart.** Not found.
5. **Whether `get-text --extent all` can read the `scrollback_pager_history_size` buffer.** Not
   found; assumed no.
6. **Durability of the `launch --type background --allow-remote-control` socketpair fd** across the
   helper restarting. Not found.
7. **tmux pane id reuse across server restarts.** The guarantee is explicitly scoped to "in the tmux
   server". Not found for across servers.
8. **Whether a `refresh-client -B` subscription can miss a start/exit pair faster than its 1 Hz
   rate.** Not found; the hook path should be treated as the authoritative edge.
9. **The fish manual-setup line for kitty shell integration.** Not found in the portion fetched.
10. **ghostty.** Issue #2 states ghostty is installed. On this machine it is not (§1). Any ghostty
    option in the root decision is currently unsubstantiated.
11. **Evidence asymmetry.** All tmux claims marked "measured" were executed here. **No kitty claim
    was measured**, because kitty had no reachable control channel (`listen_on` unset). The kitty
    column of §4 is documentation-grade, not test-grade. Before the root decision leans on kitty,
    adding `listen_on` and re-running the same probe set against kitty is the cheapest way to
    close the gap.

## 7. Sources

- tmux(1) for tmux 3.7b, as installed: `/opt/homebrew/share/man/man1/tmux.1`. Sections cited:
  *DESCRIPTION* (session persistence), *CLIENTS AND SESSIONS* (`$`/`@`/`%` ids, `TMUX_PANE`,
  `refresh-client`), *COMMAND PARSING AND EXECUTION* (`/bin/sh -c`, multi-argument form),
  *WINDOWS AND PANES* (`capture-pane`, `pipe-pane`, `respawn-pane`, `send-keys`, `split-window`),
  *OPTIONS* (`default-shell`, `default-command`, `history-limit`, `remain-on-exit`), *HOOKS*,
  *FORMATS* (`pane_dead*`, `history_size`), *MISCELLANEOUS* (`run-shell`, `if-shell`, `wait-for`),
  *CONTROL MODE*, *FILES*. Upstream: <https://github.com/tmux/tmux/blob/master/tmux.1>
- tmux behaviour measured locally on isolated servers (`tmux -L orcaresearch`, `-L orcaexit`,
  `-L orcaid`, all killed afterwards; the user's own tmux server was never touched):
  `new-session -P -F`, `split-window -P -F`, `send-keys`, `capture-pane -p -S -`, `pipe-pane -O -o`,
  `respawn-pane -k`, `remain-on-exit`, `pane_dead`/`pane_dead_status`/`pane_dead_signal`,
  `tmux -C attach` notifications, `set-hook pane-died` → `%message`, `refresh-client -B` →
  `%subscription-changed`.
- kitty remote control: <https://sw.kovidgoyal.net/kitty/remote-control/>
- kitty `launch` command and watchers: <https://sw.kovidgoyal.net/kitty/launch/>
- kitty configuration reference: <https://sw.kovidgoyal.net/kitty/conf/>
- kitty sessions: <https://sw.kovidgoyal.net/kitty/sessions/>
- kitty FAQ (multiplexers, remote persistence): <https://sw.kovidgoyal.net/kitty/faq/>
- kitty shell integration: <https://sw.kovidgoyal.net/kitty/shell-integration/>
- Installed kitty 0.47.4 self-documentation: `kitten @ --help`, `kitten @ launch --help`,
  `kitten @ get-text --help`, `kitten @ send-text --help`, `kitten @ ls --help`, `kitty --help`.
- Locally generated kitty default configuration with upstream comments:
  `~/.config/kitty/kitty.conf` (`allow_remote_control` at line 1601, `listen_on` at 1630,
  `close_on_child_death` at 1555, `remote_control_password` at 1566, `watcher` at 1686,
  `shell_integration` at 1774, `scrollback_lines` at 377, `scrollback_pager_history_size` at 408).
- Version and presence probes: `tmux -V`, `kitty --version`, `nvim --version`,
  `command -v kitten`, `command -v ghostty`, `ls -R /opt/homebrew/Caskroom/ghostty/1.0.0`,
  `ls /opt/homebrew/Cellar/fish`, `tmux show-options -g`.
