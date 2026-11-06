# Research: building orca-term as a terminal application

Resolves [#167](https://github.com/marivaldo/orca-term/issues/167), for the map
[#165](https://github.com/marivaldo/orca-term/issues/165). Feeds the decision in
[#168](https://github.com/marivaldo/orca-term/issues/168).

## The question

What can a terminal application in Rust be built on to show an agent's terminal inside one of its
own panels, and what do the existing tools of this kind do? The answer must end in a recommendation
between two families:

- **Family A**: our own terminal application, which draws every panel itself and shows each
  agent's terminal by attaching to that agent's holder.
- **Family B**: tmux or zellij draws and arranges the panes, driven from outside, with our side
  menu running in one pane.

## Short answer and recommendation

**Recommendation: Family A.** Build the application on the TUI library and `crossterm` for
drawing, put `alacritty_terminal` behind a small trait of ours as the screen of each panel, and
feed it from the holder over the protocol that `agent attach` already speaks. Keep `vt100` as the
fallback: it is six crates instead of 56 and drew both agents' screens the same in the measurement
below. ("The TUI library" is the crate at
[crates.io/crates/ratatui](https://crates.io/crates/ratatui); see the note on its name under
Method.)

The three findings that drove the recommendation:

1. **The holder already does what the tmux-based tools need tmux for.** The tmux-based tools
   found use tmux first of all to keep the agent alive without a client and to reattach
   (section 4; agent-of-empires and agent-manager say so in their READMEs). orca-term has that in
   the holder (ADR 0009): several clients at once, replay, resize and stop frames.
   Under Family B tmux or zellij would only draw panes and route keys, in exchange for a hard
   dependency ADR 0001 and ADR 0009 refused, a second terminal emulation between the agent and the
   person's terminal, and tmux options the agents need (`extended-keys`, `allow-passthrough`) that
   we would have to own.
2. **Drawing an agent's screen is the cheap part, and it is measured.** A recorded claude-code
   2.1.319 screen came out identical from `vt100` 0.18.1 and `alacritty_terminal` 0.26.0 (0 of
   3000 cells differ); an opencode 2.2.5 screen differed in 4 of 3000 cells, all of them the second
   column of a wide character, where only the bookkeeping differs. Parsing 8 MiB of such output,
   the size of a full replay, took 40 to 80 ms. Both crates pass `deny.toml`.
3. **The costly part is everything around the grid, and `alacritty_terminal` covers more of it.**
   Both agents ask the terminal questions at startup and switch on modes that change how keys and
   the mouse must be encoded (section 1.1). `alacritty_terminal` tracks those modes and writes the
   answers; `vt100` hands every one of them to a callback for us to implement. Input forwarding is
   where the one large Rust tool of Family A, herdr, has its open bugs (section 2), and the holder
   needs changes that are independent of the library (section 6).

Family B is the faster route to something on screen and is where most prior art sits. It stays
available as a convenience at no cost, because `orca-term agent attach` already runs in any tmux
or zellij pane.

The added section 7 finds that linking the agent and Neovim does not depend on where Neovim is
drawn.

## Method and trust

Each claim rests on one of these, named where it is used:

- **Measured**: run on this machine on 2026-11-06 (macOS, `claude` 2.1.319, `opencode` 2.2.5,
  tmux 3.8; zellij is not installed). Throwaway programs lived in a scratch directory outside the
  repository, were built with the machine's default toolchain (Rust 1.99.1; the repository pins
  1.97) and are not committed. Section 1.2 describes them.
- **Source**: the crate's or tool's own source or manifest, read from the cargo registry or
  through `gh api` at the commit or version named.
- **Docs**: the project's own documentation, linked.
- **Not verified**: said so in place and collected in the last section.

Third-party repositories and pages were read as data. Star counts and dates are what GitHub and
crates.io returned on 2026-11-06 and are only a rough signal.

A note on one name: "the TUI library" in this document always means the crate at
[crates.io/crates/ratatui](https://crates.io/crates/ratatui). The repository's spell check
(typos 1.52.0) rewrites that crate's bare name, and `_typos.toml` is outside what this ticket may
edit, so the name appears only inside links. Adopting the crate means adding it to `_typos.toml`.

## 1. Embedding: crates that turn PTY output into a screen

### 1.1 What the two agents ask of a terminal (measured)

Each agent was started on a PTY of 40 by 120 in this repository's checkout, with no prompt ever
submitted. The recorder answered a few queries as a plain terminal would, typed
`hello wide 日本語 😀 ok` into the prompt box without sending it, resized to 30 by 100, and hung
up. Every byte the agent printed was kept (claude: 4770 bytes, opencode: 22220 bytes).

| What the agent sent | claude-code 2.1.319 | opencode 2.2.5 |
| --- | --- | --- |
| Alternate screen (`?1049h`, and `l` on exit) | yes, once | yes, once |
| Mouse reporting (`?1000h ?1002h ?1003h`) with SGR encoding (`?1006h`) | yes, sent again on redraw | yes, once |
| Bracketed paste (`?2004h`) | yes | yes |
| Focus reporting (`?1004h`) | yes | no (only asks whether it exists) |
| 24-bit colour (`38;2` and `48;2`) | 36 sequences | 506 sequences |
| Synchronised output (`?2026h` and `l`) | no | 17 pairs |
| Kitty keyboard: asks (`CSI ? u`) | yes | yes |
| Kitty keyboard: turns on (`CSI > 5 u`, off with `CSI < u`) | not in this recording | yes |
| xterm modifyOtherKeys (`CSI > 4 ... m`) | reset only (`CSI > 4 m`) | on, then off (`CSI > 4;1 m`, `CSI > 4;0 m`) |
| Cursor style (`CSI n SP q`) | no | yes |
| Save and restore cursor as `CSI s` and `CSI u` | no (uses `ESC 7` and `ESC 8`) | yes |
| Queries that expect an answer | DA1 (`CSI c`), kitty keyboard, XTVERSION | DA1, kitty keyboard, XTVERSION, cursor position (`CSI 6n`), six DECRQM mode queries, OSC 10 and 11 colours, window size in pixels (`CSI 14 t`), XTGETTCAP, kitty graphics |
| Other OSC | 0 (title), 7501 | 0, 12, 22, 66, 99, 112, 1337, 7501 |
| Other modes | `?2031` (colour scheme reports) | `?2027` (grapheme clusters), `?2031` |
| OSC 8 links | none | none |

Three things follow for a panel:

- Both agents ran full screen on the alternate screen with mouse reporting on. A panel's own
  scrollback is therefore empty for them: scrolling means forwarding wheel events to the agent.
  Claude Code's docs describe this as its fullscreen rendering, which is a setting
  ([terminal-config](https://code.claude.com/docs/en/terminal-config.md)); a person who runs the
  default renderer gets a main-screen claude instead. Only the fullscreen one was recorded.
- The agent decides how keys must be encoded by asking the terminal first. opencode turned the
  kitty keyboard on only after the recorder answered `CSI ? 0 u`. A panel that does not answer
  gets the older, ambiguous encoding, in which Shift+Enter cannot be told from Enter (docs: same
  page, "Shift+Enter for newline").
- Several of these are sent once, at startup. Section 6 comes back to this: a replay cut at 8 MiB
  no longer contains them.

No OSC 8 link appeared, so that column of the table below rests on source reading only.

### 1.2 One measurement: the same recording through `vt100` and `alacritty_terminal`

A throwaway Rust program fed each recording to `vt100` 0.18.1 and `alacritty_terminal` 0.26.0
(with `Config.kitty_keyboard` on), applied the resize at its byte offset, and compared the two
grids cell by cell just before the agent was hung up: text, width flags, foreground, background,
bold, dim, italic, underline, inverse. It also listed what `vt100` passed to its `unhandled_*`
callbacks and what `alacritty_terminal` asked its host to write back.

| | claude-code recording | opencode recording |
| --- | --- | --- |
| Cells compared (30 by 100) | 3000, 441 with text | 3000, 294 with text |
| Cells that differ | 0 | 4, colours only |
| Cursor position | same | same |
| Alternate screen, bracketed paste, mouse mode and encoding | same in both | same in both |
| Left the alternate screen when the agent ended | both | both |
| Parsing 8 MiB of the recording, `vt100` | 70 to 80 ms | 65 to 70 ms |
| Parsing 8 MiB of the recording, `alacritty_terminal` | 37 ms | 41 to 42 ms |

The 4 differing cells are the second columns of `日`, `本`, `語` and `😀`: `vt100` leaves a wide
character's continuation cell with default colours, `alacritty_terminal` copies the colours onto
its spacer cell. A renderer has to take the colour of a wide character's second column from its
first; nothing is drawn wrong.

Where they differ is outside the grid:

- `vt100` passed to callbacks, unimplemented: `?1004` (focus), `?2026` (synchronised output),
  `?2027`, `?2031`, `?1016`, every kitty keyboard sequence (`CSI ? u`, `CSI > 5 u`, `CSI < u`),
  `CSI > 4 m`, `CSI s` and `CSI u`, the cursor position query, DA1, the DECRQM queries, the window
  size query, and every OSC except the title. It answered no query.
- `alacritty_terminal` tracked focus reporting and the kitty keyboard flags in its mode bits
  (`DISAMBIGUATE_ESC_CODES | REPORT_ALTERNATE_KEYS` after opencode's `CSI > 5 u`), and asked its
  host to write these answers to the PTY: `CSI ? 6 c`, `CSI ? 0 u`, three cursor positions, and
  the six DECRQM replies. It raised `ColorRequest` for OSC 10 and 11 and `TextAreaSizeRequest` for
  the window size, which the host answers.

Limits of this measurement: two short recordings, one per agent, of startup, typing and one
resize. No long turn, no tool output, no diff view, no permission prompt, no scrolling. It shows
the two crates agree on these screens, not that either is free of drawing bugs on others.

### 1.3 Comparison

Licence results are measured: one throwaway crate per candidate, `cargo deny check licenses
advisories sources` with this repository's `deny.toml` (allow list `MIT`, `Apache-2.0`,
`Unicode-3.0`; `unknown-git = "deny"`). "Crates" is the number of packages in that probe's lock
file, then the number built on this machine.

| | `vt100` | `alacritty_terminal` | `tui-term` | `termwiz` and `wezterm-term` | `libghostty-vt` |
| --- | --- | --- | --- | --- | --- |
| What it is | parser plus in-memory screen | the terminal half of Alacritty | a widget for the TUI library that draws a screen | wezterm's toolkit; its emulator is `wezterm-term` | Rust binding to Ghostty's terminal library (Zig) |
| Last release | 0.18.1, 2026-10-19 | 0.26.0, 2026-04-06 | 0.3.4, 2026-04-07 | `termwiz` 0.23.3, 2025-03-20; `wezterm-term` is not on crates.io | 0.2.2, 2026-09-28 |
| Repository | [doy/vt100-rust](https://github.com/doy/vt100-rust), 125 stars, 13 open | [alacritty/alacritty](https://github.com/alacritty/alacritty), 66k stars | [a-kenji/tui-term](https://github.com/a-kenji/tui-term), 234 stars, 7 open | [wezterm/wezterm](https://github.com/wezterm/wezterm), 29k stars, last tagged release 2024-02-03 | [uzaaft/libghostty-rs](https://github.com/uzaaft/libghostty-rs), 405 stars |
| Licence | MIT | Apache-2.0 | MIT | MIT | MIT OR Apache-2.0 |
| Against `deny.toml` | passes | passes | fails on `foldhash` (Zlib), which comes from the TUI library | fails: WTFPL (`terminfo`), Unicode-DFS-2016 (`finl_unicode`, `wezterm-bidi`), Apache-2.0 WITH LLVM-exception (`ar_archive_writer`); `wezterm-term` would be a git dependency, refused by `unknown-git` | passes |
| Crates | 6, 6 | 56, 33 | 44, 44 | 119, 98 | 9, 9, plus a Zig build |
| Alternate screen | yes | yes | draws what its screen holds | not evaluated | not evaluated |
| 24-bit colour | yes | yes | yes | not evaluated | not evaluated |
| Mouse modes tracked | 9, 1000, 1002, 1003; encodings 1005, 1006 | click, drag, motion; SGR and UTF-8 | no input handling | not evaluated | not evaluated |
| Bracketed paste tracked | yes | yes | n/a | not evaluated | not evaluated |
| Wide and emoji | yes (VS16 since 0.17) | yes (spacer cells, zero-width characters kept in the cell) | yes | not evaluated | not evaluated |
| Cursor style | yes, since 0.17 | yes | yes (`CursorShape`) | not evaluated | not evaluated |
| Resize | rows move to and from scrollback; a width change truncates rows, no reflow | reflows wrapped lines | n/a | not evaluated | not evaluated |
| Scrollback | rows, fixed capacity | rows (`scrolling_history`, default 10000) | shows the backend's | not evaluated | not evaluated |
| OSC 8 links | no (callback only) | yes, kept per cell | no | not evaluated | not evaluated |
| Kitty keyboard flags | no (callback only) | tracked when `Config.kitty_keyboard` is on | n/a | not evaluated | not evaluated |
| Focus reporting mode | no (callback only) | tracked | n/a | not evaluated | not evaluated |
| Synchronised output | no (callback only) | yes, in its parser | n/a | not evaluated | not evaluated |
| Answers queries | none | yes, as events for the host to write | n/a | not evaluated | not evaluated |
| Text attributes | bold, dim, italic, underline, inverse | those plus strikeout, hidden, four more underline styles, underline colour | what the backend has | not evaluated | not evaluated |

Notes per crate, all from source unless said otherwise:

- **`vt100`**. Small and readable (about 5100 lines). Three releases in October 2026 after fifteen
  months without one ([CHANGELOG](https://github.com/doy/vt100-rust/blob/main/CHANGELOG.md)). The
  resize behaviour is from its own doc comment on `Parser::set_size`: on a width change "each row
  is naively truncated or extended with blank cells". Open issue
  [#43](https://github.com/doy/vt100-rust/issues/43) asks for `CSI s` and `CSI u` and mode 2026,
  both of which opencode sent; [#42](https://github.com/doy/vt100-rust/issues/42) reports that
  some sequences with a large count take seconds. Everything it does not implement reaches the
  `Callbacks` trait, so a host can track the kitty flags, focus mode and queries itself.
  agent-of-empires uses 0.18 for its live preview (section 4).
- **`alacritty_terminal`**. Published from the Alacritty repository; a release roughly twice a
  year (0.25.0 in 2025-02, 0.25.1 in 2025-10, 0.26.0 in 2026-04), each a 0.x version that may
  break its API. It also carries a PTY spawner and an event loop (`tty/`, `event_loop.rs`) we would
  not use, and it has no key encoder: it holds the keyboard flags, and turning a key into bytes is
  the host's job. Open issue [#7697](https://github.com/alacritty/alacritty/issues/7697): an
  endless loop when a wide character is resized into a terminal one column wide, which a panel
  must guard with a minimum width. yicheng47/runner vendors this crate.
- **`tui-term`**. A widget, not an emulator: `PseudoTerminal` draws anything that implements its
  `Screen` trait into the TUI library's buffer. Its default backend is `vt100` pinned to `0.16.2`, so
  with it we would not get the 0.17 and 0.18 fixes; the `Screen` trait lets us implement the
  drawing for `alacritty_terminal` ourselves, at which point the crate saves little. It forwards
  no input; its README calls the process controller experimental, behind an `unstable` feature.
- **`termwiz` and `wezterm-term`**. Refused by policy before features were looked at: the
  emulator proper, `wezterm-term`, is not published (crates.io answers 404), and `termwiz` pulls
  in four licences outside our allow list.
- **`libghostty-vt`**. Passes the licence check, but its build script compiles Ghostty's library
  with Zig, and Zig is not installed here, so nothing was built or evaluated. herdr, the largest
  tool of Family A, vendors its own binding and requires Zig 0.16.0 to build. For us that would
  put Zig in the release build of all four targets.
- **Others found**: `avt` 0.20.0 (asciinema's virtual terminal, Apache-2.0, 2 crates, passes;
  not evaluated for interactive use), `shadow-terminal` 0.2.3 (wraps wezterm, last release
  2025-07, 15 stars), and the forks `vt100-ctt` and `mprocs-vt100`.

The drawing side, measured the same way: `crossterm` 0.29.0 passes `deny.toml` (37 crates in its
lock file). The TUI library, 0.30.2 (2026-06-19, MIT, 23k stars), fails it on one crate,
`foldhash` (Zlib), which arrives through `hashbrown` even with default features off. Adopting it
means adding `Zlib` to the allow list. The TUI library, `crossterm` and `alacritty_terminal`
together lock 214 packages and build 86 on this machine; the core's `Cargo.lock` holds 86 today.

## 2. Passing input through

**What exists to build on.**

- `crossterm` reads keys, mouse, paste and focus as events and can switch the outer terminal's
  modes: `PushKeyboardEnhancementFlags`, `EnableBracketedPaste`, `EnableMouseCapture`,
  `EnableFocusChange` ([docs](https://docs.rs/crossterm/0.29.0/crossterm/event/index.html)).
- Turning an event back into the bytes a terminal program expects is not in the TUI library,
  `crossterm`, `tui-term` or `alacritty_terminal`. `terminput` 0.5.16 (MIT OR Apache-2.0, passes
  `deny.toml`, 2 crates) says it supplies "methods for parsing and encoding ANSI escape
  sequences" and has a `crossterm` adapter
  ([README](https://github.com/aschey/terminput/blob/main/README.md)). Which encodings it writes
  (legacy, kitty flags) was not verified.
- `agent attach` today does neither: it puts the terminal in raw mode and forwards stdin bytes
  untouched as `Input` frames, watching for one byte, Ctrl-\ (`src/adapters/terminal.rs`). The
  agent's own mode sequences reach the person's terminal directly, so the terminal encodes keys
  the way the agent asked. That works because the agent owns the whole screen.

**Two ways for a panel**, both seen in the wild:

1. **Decode and encode again.** Read events, and for the focused panel write the bytes its agent
   asked for: legacy or kitty encoding by the flags it pushed, the paste wrapped in `ESC [ 200 ~`
   only when it enabled bracketed paste, mouse events in SGR form with the panel's origin
   subtracted, focus in and out when it enabled `?1004`. This needs the panel's emulator to track
   those modes (section 1.2) and an encoder. tmux works this way and needs `extended-keys` to keep
   modified keys apart ([Claude Code docs, "Configure tmux"](https://code.claude.com/docs/en/terminal-config.md)).
2. **Pass bytes through and mirror modes.** Keep forwarding raw bytes as `agent attach` does, and
   put the outer terminal in whatever keyboard and paste modes the focused panel's agent asked
   for, switching them when focus moves to the side menu. Only the mouse must be decoded, since
   its coordinates change. No tool was found that documents doing this; it is an inference from
   how `agent attach` works, and something for the prototype in #170 to try.

**Focus switching.** tmux and herdr use a prefix key (herdr's README: "tmux-style prefix keys and
click, drag, split", detach on `ctrl+b q`); zellij uses modes. `agent attach` reserves one key,
Ctrl-\, and the Neovim client passes `--detach-key none` because that key is Neovim's own. Any
key the application keeps is a key the agent never sees, so the choice belongs to #168 and #170.

**Pitfalls reported by projects that did this** (herdr, at commit `ee5bfb5041bb`, is the one Rust
project of this kind with a public tracker of size):

- It does not use `crossterm`'s event reader for panel input. `src/raw_input.rs` is its own byte
  framer, with timing rules for an escape split across reads and for a lone Escape versus the
  start of a sequence (its comments cite its issues 4630, 3480, 4751 and 5052), and special
  handling for the outer terminal's own replies arriving mixed with keys.
- Open issues on the kitty keyboard alone:
  [#5654](https://github.com/herdrdev/herdr/issues/5654) (Ctrl+letter keys disappear on Ghostty
  after the push), [#4476](https://github.com/herdrdev/herdr/issues/4476) (Ctrl+letter sends the
  plain letter on iTerm2), [#3146](https://github.com/herdrdev/herdr/issues/3146) (shifted
  punctuation arrives without its character), [#5291](https://github.com/herdrdev/herdr/issues/5291)
  (stale keyboard flags left behind after a handoff).
- Replayed output contains old queries. Orca's headless emulator scopes replies to live output
  only: "Scope replies to their live chunk so replayed queries cannot reach the PTY"
  (`headless-emulator.ts`, section 5). Our protocol allows the same, because the replay is exactly
  the first `Output` frame of a connection.
- Mouse reporting takes text selection away from the person's terminal. With both agents asking
  for all mouse motion, selecting text in a panel needs either our own selection or the outer
  terminal's bypass modifier. Not investigated further.

## 3. Family B: tmux and zellij arranged from outside

tmux's programmatic surface was measured for #2 (the note on that ticket's research branch,
`docs/research/`, against tmux 3.7b). It is not repeated here. What matters from it: panes get
stable ids returned by the creating call, control mode (`tmux -C`) carries commands, output and
layout notifications on one connection, `send-keys` writes input, a private server (`-L`) keeps
the person's own tmux untouched, nothing has to be enabled, and nothing survives a reboot.

New for this question:

- **Two shapes.** In the first, tmux holds the agents, as every tmux-based tool in section 4 does.
  That replaces the holder and reverses ADR 0009. In the second, tmux only draws: one pane runs
  our side menu, the centre pane runs `orca-term agent attach <worktree>`, and the side menu
  switches worktree by asking tmux to respawn or swap that pane. The holder stays as it is. Only
  the second is compatible with what map #165 keeps from map #1.
- **Versions** (tmux [CHANGES](https://github.com/tmux/tmux/blob/master/CHANGES)): control-mode
  format subscriptions (`refresh-client -B`), pausing a pane that falls behind, `extended-keys`
  and `display-popup` arrived in 3.2; `allow-passthrough` in 3.3; `extended-keys-format` in 3.5;
  floating panes in 3.7. tmux 3.8 is installed here. wainmann/agent-manager checks for 3.1 or
  later.
- **What the agents need from tmux** (Claude Code docs, "Configure tmux"): inside tmux,
  Shift+Enter submits instead of inserting a line and notifications do not reach the outer
  terminal until `allow-passthrough on` and `extended-keys on` are set. On a private server we
  would set them ourselves, which puts the floor at tmux 3.3.
- **What the person installs.** tmux. On this machine it comes from Homebrew
  (`/opt/homebrew/bin/tmux`), not from macOS. The tmux-based tools in section 4 list it as a
  prerequisite or install it themselves.
- **Failure modes.** tmux missing: the application cannot start, so `doctor` would need a new
  prerequisite. tmux older than 3.3: modified keys reach the agent wrong. On the person's own
  server instead of a private one: their prefix, key bindings and status line apply, and whatever
  we set changes their tmux. A person who already works inside tmux gets tmux inside tmux, with
  two prefixes (inference, not measured). The borders, status line and copy mode are tmux's, so
  the look can only approach Orca's as far as tmux options allow.
- **zellij** 0.45.1 (2026-08-28, MIT, 36k stars), from its docs, nothing measured since it is not
  installed here. Layouts are KDL files in which a pane has a `command`, a `size` and a
  `split_direction` ([layouts](https://zellij.dev/documentation/creating-a-layout.html)).
  `zellij action` works from inside or outside a session (`zellij --session <name> action ...`)
  and includes `new-pane`, `focus-pane-id`, `list-panes`, `send-keys`, `write-chars`,
  `dump-screen`, `launch-plugin` and `pipe`
  ([CLI actions](https://zellij.dev/documentation/cli-actions.html)). Plugins are WebAssembly
  (WASI), Rust being the only officially supported language, and ask the person for permissions
  when loaded ([plugins](https://zellij.dev/documentation/plugins.html),
  [permissions](https://zellij.dev/documentation/plugin-api-permissions.html)). Our side menu
  could therefore be a plain pane that calls `zellij action`, or a plugin, which is a second
  build target that cannot link the core. zellij holds its panes' terminals in its own
  background process, as tmux does.

## 4. Prior art

Stars and last push are from GitHub on 2026-11-06. "Built on" is from each project's manifest and
README at its default branch; nothing here was installed or run.

| Tool | Built on | Who keeps the agent's terminal | Stars, last push | Where it falls short for us |
| --- | --- | --- | --- | --- |
| [herdr](https://github.com/herdrdev/herdr) | Rust; the TUI library 0.30, `crossterm` 0.29, its own binding to a vendored libghostty-vt, a vendored `portable-pty` | its own background process; a client reattaches | 48.6k, 2026-11-03 | Family A done at full size. Needs Zig 0.16 to build. A general terminal application with its own way of holding terminals, not a library we can take the panel from. Apache-2.0 |
| [agent-of-empires](https://github.com/agent-of-empires/agent-of-empires) | Rust; the TUI library 0.30, `crossterm` 0.29, `vt100` 0.18 | tmux ("Requires tmux") | 3.7k, 2026-11-06 | tmux is required. Notable: its `Cargo.toml` says it added `vt100` to "parse a pane's raw byte stream into an owned grid instead of scraping `capture-pane` snapshots" for the live preview |
| [claude-squad](https://github.com/smtg-ai/claude-squad) | Go; bubbletea | tmux (listed under prerequisites) | 8.7k, 2026-08-20 | tmux is required; AGPL-3.0 |
| [workmux](https://github.com/raine/workmux) | Rust; the TUI library 0.30, a patched `crossterm` | tmux windows, with kitty, WezTerm and zellij as other backends | 3.0k, 2026-11-06 | Arranges the person's tmux; no screen of its own beyond a dashboard |
| [agtx](https://github.com/fynnfluegge/agtx) | Rust; the TUI library 0.30, `crossterm` 0.29 | tmux (by its keywords) | 1.7k, 2026-11-03 | A board over tmux; how it shows an agent was not verified |
| [dmux](https://github.com/standardagents/dmux) | Node; ink | tmux ("Tmux pane manager") | 1.8k, 2026-08-16 | tmux is required |
| [agent-deck](https://github.com/asheshgoplani/agent-deck) | Go; bubbletea, `charmbracelet/x/vt` | tmux, optionally a private server (`tmux -L agent-deck`) | 1.2k, 2026-11-06 | tmux is required |
| [agent-manager](https://github.com/wainmann/agent-manager) | Go; bubbletea | tmux 3.1 or later, on a private server | 0.6k, 2026-11-06 | tmux is required |
| [ccmanager](https://github.com/kbwo/ccmanager) | TypeScript; ink, `@xterm/headless`, `@xterm/addon-serialize` | itself, in process | 1.3k, 2026-10-27 | Family A in Node. Its README sells "No tmux dependency". Whether it shows more than one agent at once was not verified |
| [runner](https://github.com/yicheng47/runner) | Rust; a vendored `alacritty_terminal` | not verified | 0.5k, 2026-11-06 | Only its manifest was read |
| zellij, tmux | their own emulators | themselves | 36k, 50k | The tools Family B would arrange |

Graphical kin, outside this map's scope and listed only so they are not mistaken for terminal
applications: Orca (section 5), [cmux](https://github.com/manaflow-ai/cmux) (Swift, built on
Ghostty), emdash, Paradev and Crystal (Electron).

What the table says:

- The tmux-based tools are the majority, and each uses tmux first of all to keep agents alive when
  the tool closes ("Sessions are tmux sessions running in the background", agent-of-empires'
  README). orca-term already has that without tmux.
- The tools that embed carry a full emulator: Ghostty's (herdr), xterm.js (ccmanager, Orca),
  Alacritty's (runner, going by its manifest). None of those found embeds with `vt100` for an
  interactive panel; agent-of-empires uses it for a preview, and by its README tmux serves the
  interactive attach.
- Nothing found is a Rust library that gives us a ready panel. The closest reusable pieces are the
  crates of section 1.

## 5. Orca

From `stablyai/orca` at commit `10b8666c458f`, only as far as layout and behaviour to copy.

- **Terminal area.** xterm.js (`@xterm/xterm` 6.1 beta) with the webgl, fit, search, unicode11,
  image, ligatures and serialize addons, on `node-pty` (`package.json`). The component names show
  a tab bar per worktree and split surfaces inside it (`TerminalTitlebarTabs.tsx`,
  `TerminalSplitWorkspaceSurfaces.tsx`, `tab-bar/`, `terminal-pane/`).
- **Splits.** A binary tree: a node is a leaf or a split with a `direction` (`vertical` or
  `horizontal`), two children and an optional `ratio`; a pane is added on a side (`left`, `right`,
  `top`, `bottom`) of an existing leaf, and panes on the same axis are equalised into equal shares
  (`src/shared/workspace-layout/terminal-pane-tree.ts`). This is the same shape tmux layouts have,
  and a tree of nested splits is what the TUI library's layouts are made for (its docs were not
  re-read here).
- **Reattaching.** Orca's terminals live in a separate long-lived process. For each one it keeps
  a headless xterm (`@xterm/headless` with `SerializeAddon`, in `headless-emulator.ts`). On
  reattach it sends a serialised snapshot of the screen plus a chosen number of scrollback rows,
  not the byte history, and then restores the modes explicitly: `terminal-frame-restore-sequences.ts`
  writes `?1049h`, `?1h`, `?2004h`, `?1004h`, the mouse modes, `?1006h` or `?1016h` and `?25l`
  as the emulator's state requires, and a separate step restores the kitty keyboard flags. The
  comments in `headless-emulator.ts` say queries in restored output must never be answered.
- **Side menu.** `src/renderer/src/components/sidebar/`, with `worktree-list/`, `workspace-card/`
  and `workspace-status/`. The package list has a virtualised list and drag and drop
  (`@tanstack/react-virtual`, `@dnd-kit`); whether the side menu uses them was not checked. Its
  contents were researched for map #1 and the sidebar prototype; the components were not read
  again here.

What this informs: the layout to copy is a left side menu, and a centre that belongs to the
selected worktree with tabs and a split tree. And Orca answers the question section 6 raises about
replay by keeping an emulator next to the PTY.

## 6. Our holder: what a panel needs beyond `agent attach`

Read from `src/domain/protocol.rs`, `src/adapters/holder.rs`, `src/adapters/holder/process.rs`,
`src/adapters/holder/client_queue.rs` and `src/adapters/terminal.rs` at `c4011e5`.

What is already there and carries over unchanged:

- **Several clients at once.** The holder keeps a map of clients, gives each its own queue and
  writer, and serialises their input under one lock. A second panel, or the application beside
  the Neovim client, can attach today.
- **The codec.** `Frame` and `Decoder` live in the domain and are not tied to a terminal: input
  (`i`), resize (`r`), stop (`s`), output (`o`), exit (`x`).
- **Typing without attaching.** `holder::type_input` connects as one more client, sends input and
  no size, and leaves (section 7).
- **The agent's end.** An `Exit` frame carries `exit <code>` or `signal <n>`; a client attaching
  after the end still gets the replay and the exit.

What a panel needs that is missing:

1. **An attach that is not the terminal.** `terminal::attach` is wired to stdin, stdout, raw mode
   and SIGWINCH. A panel needs the same loop with other ends: output frames into an emulator, keys
   into input frames, the panel's size into resize frames.
2. **A connect that does not change directory.** `holder::connect` changes the process's working
   directory to the worktree's admin directory and connects by the socket's bare name, because a
   unix socket address holds about 100 bytes; its doc says to call it before starting any thread.
   An application with many panels and threads cannot do that per panel.
3. **A rule for the size.** There is one PTY size. Any client's resize sets it, the last one
   wins, and each client's first resize also jiggles the PTY (one column narrower and back, at
   once and again a second later) to make the agent redraw. A single panel is fine. Two clients of
   different sizes, such as the application and an `agent attach` inside Neovim, or the same
   worktree in two panels, overwrite each other, and the loser draws a screen laid out for another
   size. No frame tells a client the PTY's current size, so a client cannot even letterbox. tmux's
   answers are its `window-size` values `latest` and `smallest` (tmux CHANGES, 2.9 and 3.1).
4. **State, not only bytes, on attach.** The replay is the last 8 MiB of raw output, sent as the
   first output frame. Feeding it to an emulator is cheap (section 1.2: 40 to 80 ms). The problem
   is what it no longer contains once the agent has printed more than 8 MiB. In the recordings
   opencode sent `?1049h`, the mouse modes and `CSI > 5 u` once, at startup; claude repeated the
   mouse and paste modes on redraw but sent `?1049h` once. An emulator fed a cut replay does not
   know the alternate screen, mouse or keyboard state, and the jiggle repaints cells, not modes.
   The cut can also land inside an escape sequence. Orca's answer is an emulator beside the PTY
   and a snapshot (section 5). A smaller one is for the holder to remember the mode-setting
   sequences it saw and put them in front of the replay. How long an agent takes to print 8 MiB
   was not measured.
5. **Someone to answer the agent.** Both agents ask questions at startup (section 1.1), and
   `agent start` starts the agent before any client connects. Today, by the code, the only
   answers can come late, from whichever terminal attaches first and meets the questions in the
   replay; this was not observed. A panel's emulator must not answer replayed questions
   (section 2). So with panels, unless the holder answers, nobody does. What the agents do when
   no answer comes was not measured.
6. **Focus and mouse for the panel.** claude enabled focus reporting, so a panel should send
   focus in and out as it gains and loses focus; mouse events need the panel's origin subtracted.
   Both need the mode tracking of section 1.2.

None of the six depends on which emulator crate is chosen, and items 2 to 5 are changes to the
core, not to the client.

## 7. Linking the agent's terminal and Neovim

The question: can the application link the agent and the person's own Neovim, whether Neovim runs
in a panel or tab of the application, zoomed to the whole screen, or in a separate terminal
window?

**Main finding: yes, and none of the mechanisms depends on where Neovim is drawn.** They are a
loopback WebSocket plus a file under `~/.claude/` (Claude Code's editor protocol), Neovim's own
RPC socket (which the core already uses), and the holder's socket (which the core already types
into). What differs between a Neovim in a panel and one in another window is discovery and focus,
not capability.

### 7.1 Claude Code's editor integration

- **The protocol** (docs: [vs-code, "The built-in IDE MCP server"](https://code.claude.com/docs/en/vs-code.md)).
  The editor side runs a local MCP server on `127.0.0.1`, on a random port from 10000 to 65535,
  over unencrypted `ws://`. It writes `~/.claude/ide/<port>.lock` (mode 0600, or under
  `$CLAUDE_CONFIG_DIR/ide/`) holding a fresh token, and the CLI must present that token in the
  `X-Claude-Code-Ide-Authorization` header. Through it the CLI opens diffs in the editor, reads
  the current selection, and attaches the selection and the active file's path to each prompt.
  Two tools are visible to the agent itself, `mcp__ide__getDiagnostics` and
  `mcp__ide__executeCode`; the rest are internal calls of the CLI.
- **How the CLI finds the editor.** Started in the editor's own terminal it connects by itself.
  From any other terminal the person runs `/ide` (same page, "External terminal"). `claude --help`
  lists `--ide`: "Automatically connect to IDE on startup if exactly one valid IDE is available".
  The variables `CLAUDE_CODE_AUTO_CONNECT_IDE` and `CLAUDE_CODE_IDE_SKIP_VALID_CHECK` force an
  attempt and skip "validation of IDE lockfile entries"
  ([env-vars](https://code.claude.com/docs/en/env-vars.md)).
- **In Neovim.** [coder/claudecode.nvim](https://github.com/coder/claudecode.nvim) (MIT, 3.2k
  stars, last push 2026-11-02, last release v0.3.0 on 2025-09-16, 95 open issues and pull
  requests, pure Lua) implements the editor side. Its `PROTOCOL.md` (commit `af04fbf9c27c`)
  records the lock file's fields (`pid`, `workspaceFolders`, `ideName`, `transport`, `authToken`),
  the two variables it sets for a claude it launches itself (`CLAUDE_CODE_SSE_PORT`,
  `ENABLE_IDE_INTEGRATION`), the notifications it sends (`selection_changed`, `at_mentioned`) and
  the tools it serves: `openFile`, `openDiff` (blocking until the person accepts or refuses),
  `getCurrentSelection`, `getLatestSelection`, `getOpenEditors`, `getWorkspaceFolders`,
  `getDiagnostics`, `checkDocumentDirty`, `saveDocument`, `close_tab`, `closeAllDiffTabs`. Its
  README has terminal providers `external` and `none`, for which "Claude runs outside Neovim".
  The other plugin of that name, greggh/claude-code.nvim, was last pushed in 2026-02 and was not
  examined.
- **With claude on our holder's PTY and Neovim elsewhere.** Nothing in the protocol involves the
  terminal claude runs on, so it should work: Neovim loads claudecode.nvim with the `none`
  provider, and the claude in the holder connects with `/ide`, which the core can type through
  the holder (7.3), or by being started with `--ide`. This was **not run**. Four things stand
  between "should" and "does":
  - The holder starts claude with a fixed command line (`claude_argv` in
    `src/domain/launch.rs`: `--settings`, `--permission-mode acceptEdits`, and `--resume`), so
    `--ide` would be a change to the launch and its guard.
  - The agent inherits the holder's environment as it was at `agent start`, minus four variables
    (`STRIPPED_VARS`). The port variables would pass through, but they are frozen at start and go
    wrong as soon as Neovim restarts on another port. Discovery through the lock file does not
    have that problem.
  - What makes a lock file "valid" for a given claude is not documented. claudecode.nvim writes
    Neovim's working directory as `workspaceFolders`. Whether a Neovim opened on the primary
    checkout counts for a claude running in a worktree directory elsewhere was not tested.
  - Every launch uses `acceptEdits`, under which edits inside the worktree are not asked about.
    `openDiff` is the approval step for an edit, so the "proposed diff in the editor" part of the
    integration would mostly not fire; the selection and file context would (inference from the
    two documents, not observed).
  - With several agents and one Neovim: `PROTOCOL.md` says each connection is a separate client.
    Whether a selection is then sent to every connected claude was not verified.
- **opencode.** Its docs describe an extension for VS Code and similar editors that shares the
  selection or tab and requires running opencode "in the integrated terminal"
  ([ide](https://opencode.ai/docs/ide/)); no lock-file protocol for an arbitrary editor was found.
  What it has instead is its own server: "When you run opencode it starts a TUI and a server"
  ([server](https://opencode.ai/docs/server/)). The Neovim plugins go through that, from the
  editor's side: [opencode.nvim](https://github.com/nickjvandyke/opencode.nvim) (MIT, 3.9k stars,
  v1.0.2 on 2026-09-14) finds a running opencode's server, sends prompts with editor context and
  forwards its events as autocommands. Whether the opencode our holder starts exposes that server
  to a Neovim outside the sandbox was not verified.

### 7.2 Driving Neovim from the core

Researched for #3 (the note on that ticket's research branch, `docs/research/`): from a separate
process over msgpack-RPC on Neovim's socket, opening a buffer (`nvim_cmd` `edit`), moving the
cursor, reading the mode and the active selection (`getpos` and `getregion` through
`nvim_exec_lua`) all worked on Neovim 0.12.4.

What the core has today: a running Neovim records its server address with the hidden
`orca-term nvim register <address>`, one file per Neovim under the repository's git common
directory (`src/domain/nvim.rs`). On a change the core sends each one a single request, encoded
by hand, that fires the `User OrcaTermWorktreeChanged` autocommand, and reads only the first byte
of the answer, within 200 ms (`src/adapters/nvim.rs`).

What is new for a terminal application:

- **Open a file at a line, open a diff.** Both are requests of the same kind the core already
  sends (`nvim_cmd` with `edit`, then `nvim_win_set_cursor`; for a diff, the Lua client's own
  review, whose entry point was not looked up). The hand encoder covers arrays, maps, strings and
  integers up to 127, so a line number above 127 needs one more msgpack type.
- **Learn the current file or selection.** This needs the answer decoded, which the core does
  not do today: it reads one byte. Either the decoder grows, or the direction is turned around
  and Neovim tells the core (as notes already do, 7.3).
- **Which Neovim.** A Neovim the application starts in a panel can be given `--listen <socket>`
  by us, so its address is known without the registry. A Neovim in a separate window is found
  through the registry, and only if it loaded the Lua client, which is what runs `nvim register`.
- **Focus.** After asking a Neovim in a panel to open a file, the application can focus or zoom
  that panel. It cannot raise another terminal window; there only the effect inside Neovim
  happens (inference).

### 7.3 The other direction, with what the core has

- `holder::type_input` types into the agent's terminal as one more client: one input frame, a
  pause, a second input frame, no size sent and no stop (`src/adapters/holder.rs`).
- `orca-term note send` uses it to paste the pending notes as one prompt in Orca's format, as a
  bracketed paste followed by Enter on its own (`src/ops/note_send.rs`). Notes are attached from
  Neovim to a line, a range of lines or a whole file (`note add`), so "send this selection to the
  agent" exists today as a note.
- `orca-term agent resolve` types a fixed prompt the same way (`src/ops/agent_resolve.rs`).
- There is no command that types arbitrary text into an agent. Typing `/ide`, or a file
  reference that is not a note, would need one.

### 7.4 Does any of it depend on Neovim being inside the application?

No. Claude's protocol is loopback and a file in the home directory. Neovim's RPC is a unix
socket. The holder's socket is in the worktree's admin directory. All three work the same from a
panel, a tab or another window. Inside the application we gain two conveniences: we choose
Neovim's socket and environment when we start it, and we can move focus to it. Neither is needed
for the link itself.

## 8. What each family would cost orca-term

| | Family A: our own application | Family B: tmux or zellij arranged by us |
| --- | --- | --- |
| New Rust dependencies | the TUI library, `crossterm`, and `alacritty_terminal` or `vt100`; perhaps `terminput`. The probe with the first three locks 214 packages against 86 today | none if the side menu is drawn by hand; the TUI library and `crossterm` if not |
| `deny.toml` and `_typos.toml` | `Zlib` added to the allow list for the TUI library (one crate, `foldhash`), and that library's name added to the spell check's words; the emulators pass as they are | the same two lines only if the TUI library is used |
| Build | stays pure Rust on all four targets (no Zig, as long as libghostty is not chosen) | unchanged |
| What changes in the core | the six items of section 6: an attach for panels, a connect without changing directory, a size rule and frame, state on attach, answering the agent's queries, focus and mouse | a tmux (or zellij) adapter: a private server with our options, layout commands, pane ids; a new `doctor` prerequisite with a minimum version. Of section 6 only the size rule, since tmux panes use `agent attach` as it is |
| New client code | drawing, the split tree, the side menu, input forwarding, selection and scrolling inside panels | the side menu and the commands that arrange panes |
| ADRs touched | none reversed: the holder stays the only owner of the PTY | ADR 0001 and ADR 0009 refused tmux as a hard dependency; this makes it one for the client |
| What the person installs | nothing beyond orca-term | tmux 3.3 or later, or zellij |
| Look and feel | ours to make like Orca's, at the price of building it | tmux's or zellij's borders, status line, copy mode and prefix |
| Largest risk | input forwarding (section 2) | the agents' keys through a second emulator, and living with the person's own tmux |
| Shown to work by | herdr, ccmanager, zellij itself | claude-squad, agent-of-empires and five more in section 4 |

## 9. Open questions for #168

1. **Family A, or the second shape of Family B as a first step?** The recommendation is A. B's
   second shape needs almost no core change and could ship first, but its client code is thrown
   away afterwards.
2. **Which emulator.** `alacritty_terminal` (tracks the modes and answers the queries the agents
   use, reflows, 56 crates, an API that breaks about twice a year) or `vt100` (6 crates, drew the
   same screens, leaves modes and queries to us). Recommended: `alacritty_terminal` behind a trait
   of ours, so the choice can be undone.
3. **Where the emulator lives.** Only in the application, or also in the holder so that attach
   gets a snapshot and the agent's queries are answered with no client attached (sections 5 and
   6). This changes the protocol and is worth its own ruling.
4. **The size rule** when two clients of different sizes are attached, and whether the Neovim
   client keeps attaching at all (#166).
5. **Input**: decode and encode again, or pass bytes through and mirror modes; and which key the
   application keeps for itself, given that Orca has none, tmux uses a prefix and `agent attach`
   uses Ctrl-\.
6. **Drawing**: the TUI library with `Zlib` allowed, or `crossterm` alone.
7. **How many agents are visible at once.** Orca's centre belongs to one worktree and splits
   inside it. "Panels like tmux" may mean that, or several worktrees' agents side by side. The
   second multiplies the size and focus questions.
8. **Scrolling and selecting text in a panel**, given that both agents take the mouse.
9. **The Neovim link** (section 7): whether to start claude with `--ide`, and whether the core
   gains a command that types arbitrary text into an agent.

## 10. What I did not verify

- Neither family was prototyped. No panel was drawn, no key was forwarded through an emulator.
- The recordings are short: startup, typing and one resize, with claude in fullscreen rendering.
  No long turn, tool output, diff, permission prompt, scrolling, paste or mouse use; no claude in
  its default renderer.
- Why claude did not turn the kitty keyboard on in its recording (its docs say it does on
  terminals that support it since 2.1.269). The recorder did not answer XTVERSION and removed
  `TERM_PROGRAM`, which may be what it decides on.
- What either agent does when its startup queries get no answer, and how today's late answers
  from a replay affect it.
- How long a busy agent takes to print 8 MiB, and so how often a replay is cut.
- `termwiz`, `wezterm-term`, `libghostty-vt`, `avt` and `shadow-terminal` were not built or
  exercised; `libghostty-vt` needs Zig, which is not installed.
- Which key encodings `terminput` writes.
- zellij: nothing was run. The minimum version for each `zellij action` used above, how its key
  modes interact with the agents' keys, and what a plugin can and cannot reach were not checked.
- tmux: this note ran no tmux command. Versions come from its CHANGES file, behaviour from #2's
  note; the nested-tmux and own-configuration failure modes are inferences.
- Prior art was read, not run: manifests and READMEs only. For agtx, runner and ccmanager the
  table says what was not established. Whether agent-of-empires hands interactive attach to tmux
  is inferred from its README and manifest.
- Orca's side menu components and its keyboard shortcuts for splits were not read; which Orca
  version the commit corresponds to was not recorded.
- Claude Code's editor link was not run with claudecode.nvim: the connection from a claude in a
  holder, the "valid IDE" rule for a worktree directory, behaviour with several agents, and
  whether `openDiff` fires under `acceptEdits`.
- Whether the claude process itself can reach a loopback port from where the holder starts it.
  The sandbox is described in the code as applying to the commands the agent runs, but this was
  not tested.
- opencode's server from a Neovim outside the sandbox, and the second opencode Neovim plugin
  (sudo-tee/opencode.nvim), were not examined.
- Whether a terminal application can raise another terminal window.

## Sources

- This repository at `c4011e5`: `docs/adr/0001-*.md`, `docs/adr/0009-*.md`, `CONTEXT.md`,
  `ARCHITECTURE.md`, `deny.toml`, `Cargo.lock`, `src/domain/protocol.rs`, `src/domain/launch.rs`,
  `src/domain/nvim.rs`, `src/adapters/holder.rs`, `src/adapters/holder/process.rs`,
  `src/adapters/holder/client_queue.rs`, `src/adapters/terminal.rs`, `src/adapters/nvim.rs`,
  `src/ops/note_send.rs`, `src/ops/agent_resolve.rs`; the research notes for #2 and #3 on their
  branches.
- Crates, read from the cargo registry: `vt100` 0.18.1, `alacritty_terminal` 0.26.0, `vte`
  0.15.0, `tui-term` 0.3.4, `libghostty-vt-sys` 0.2.2. Release dates and licences from the
  crates.io API.
- Measured here: `cargo deny` 0.20.2 against this repository's `deny.toml`; the recordings of
  `claude` 2.1.319 and `opencode` 2.2.5; the comparison program.
- GitHub, through `gh api`, on 2026-11-06: the repositories linked in sections 1, 2, 4, 5 and 7;
  `stablyai/orca` at `10b8666c458f`, `herdrdev/herdr` at `ee5bfb5041bb`,
  `coder/claudecode.nvim` at `af04fbf9c27c`, `agent-of-empires/agent-of-empires` at
  `bbe8e66627a7`.
- Docs: [Claude Code terminal configuration](https://code.claude.com/docs/en/terminal-config.md),
  [Claude Code in VS Code](https://code.claude.com/docs/en/vs-code.md),
  [Claude Code environment variables](https://code.claude.com/docs/en/env-vars.md),
  `claude --help`, [opencode IDE](https://opencode.ai/docs/ide/),
  [opencode server](https://opencode.ai/docs/server/),
  [tmux CHANGES](https://github.com/tmux/tmux/blob/master/CHANGES),
  [zellij documentation](https://zellij.dev/documentation/),
  [crossterm events](https://docs.rs/crossterm/0.29.0/crossterm/event/index.html).
