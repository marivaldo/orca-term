# Research: moving around the terminal application without a prefix key

Resolves [#198](https://github.com/marivaldo/orca-term/issues/198), for the map
[#197](https://github.com/marivaldo/orca-term/issues/197). Feeds the decisions in
[#199](https://github.com/marivaldo/orca-term/issues/199) (which keys),
[#200](https://github.com/marivaldo/orca-term/issues/200) (focus stops and jumps) and
[#201](https://github.com/marivaldo/orca-term/issues/201) (mouse), and gives
[#202](https://github.com/marivaldo/orca-term/issues/202) (learning the keys) its material.

## The question

How do terminal programs that host other terminals, and the agents themselves, let a person move
around without a prefix key, and what can the application see of the keyboard? The panels hold
programs that take every key (claude-code, opencode, Neovim with LazyVim, a shell), so the
application can only own keys nothing inside them uses, and only keys the terminal sends at all.
The driver's terminal is kitty on macOS; the application must still work in Ghostty, iTerm2 and
Terminal.app.

## Short answer and recommendation

1. **No modifier family is free everywhere.** Every family that reaches a TUI in every terminal
   (Ctrl+letter, Alt+letter, the arrows with Ctrl, Alt or Shift, Ctrl+Tab) is already bound by at
   least one of claude-code, opencode, LazyVim or fish (section 4). The families nothing inside
   uses are the ones that do not arrive everywhere: Cmd (super), which only kitty forwards
   by default and Terminal.app never does, and Alt+digit (bound only by bash and zsh, as a
   numeric argument, not by fish), which on macOS needs Option set to act as Alt (kitty's
   default is not to).
2. **Claude Code is fluid because it binds few global keys, and many keys per context.** Its
   bindings belong to contexts (the prompt, a dialog, the transcript viewer, a footer item, a
   panel of a mod), and inside a context single bare keys act (`j`, `k`, `q`, `{`, `[`, `?`).
   Its global keys are a handful of Ctrl letters, three Option letters of its own (plus
   readline's Alt editing keys) and two-key sequences on `Ctrl+X`. It moves focus between its own regions with `Ctrl+X Tab` and comes back with `Esc`
   (section 1).
3. **Hosts keep keys for themselves in one of three ways**, all with known collisions: a prefix
   (tmux `Ctrl-b`, zellij's `unlock-first` preset with `Ctrl g`), a root modifier family (zellij's
   default `Alt` keys, kitty's `ctrl+shift` and `cmd` keys, Ghostty's `cmd` keys), or a lock
   mode that hands every key to the program until one key unlocks (zellij's `locked`). zellij
   ships the lock for "terminal applications with keyboard shortcuts that collide" with its own
   (section 2).
4. **On the mouse, every host does the same two things**: a click on a split that is not focused
   focuses it, and inside the focused one the click and the wheel go to the program when it asked
   for mouse reporting. A held modifier (Shift in kitty, Ghostty, WezTerm and zellij; Option in
   iTerm2; Fn in Terminal.app) bypasses all of it so the terminal selects text natively
   (section 5).

**Recommendation for the three tickets, given kitty on macOS:**

- **#199**: make the application's own keys bare single keys wherever the focus is on something
  the application draws (the side menu, the right column, the help, pickers), as Claude Code
  does, and reserve as few keys as possible for leaving a panel. For those few, use **`Cmd` in
  kitty** (nothing inside the panels binds the free `Cmd` letters, and kitty forwards them as
  `super` by default) with **`Alt+digit` and one fallback prefix** for the other terminals. Keep a
  prefix only as that fallback, not as the main route. This is a recommendation to prototype in
  #203, not to rule: the Cmd route rests on reading kitty's source and on a crossterm probe, not
  on keys pressed in kitty itself.
- **#200**: three stops (side menu, the centre's panels, the right column), with focus that
  enters a panel on `Enter` or a click and leaves it with the one reserved key; jumps to a
  numbered worktree and to the `agent` / `nvim` / `shell` tab by digit.
- **#201**: turn mouse capture on, focus on click, forward clicks and the wheel to a panel whose
  program asked for the mouse, and leave Shift-drag to the terminal for selecting text.

Section 6 lays out the options and their trade-offs.

## Method and trust

Each claim rests on one of these, named where it is used:

- **Source** or **Docs**: the project's own code or documentation, read on 2026-11-11 at the
  commit linked (GitHub repositories), at the version installed (kitty 0.50.0, tmux 3.8,
  Neovim 0.12.6, fish 4.9.3, crossterm 0.29.0, vt100 0.18.1), or at the URL given (Claude Code,
  iTerm2, Apple and GNU pages, read on the same date; they carry no version).
- **Measured**: run on this machine. One throwaway program printed the events crossterm 0.29.0
  parsed from byte sequences written into its terminal by a private tmux server
  (`tmux -L research198 -f /dev/null`, not the person's). It shows what crossterm makes of the
  bytes a terminal sends; it says nothing about which bytes kitty or Ghostty send for a key.
  Scratch files only; nothing is committed but this document.
- **Not verified**: said so in place and collected in the last section. zellij, Ghostty,
  WezTerm and iTerm2 are not installed here; nothing about them was run.

Third-party pages and repositories were read as data.

Installed versions of the panels' programs: claude-code 2.1.322, opencode 2.2.5, Neovim 0.12.6.
Claude Code's docs, read on the same day, describe a version at least as new.

## 0. What the application does today

From `src/app/state.rs`, `src/app/keys.rs` and `src/adapters/screen.rs` on `main` (9d8687e):

- **Terminal modes.** `Screen::enter` turns on raw mode, the alternate screen and bracketed paste,
  nothing else: no mouse capture (`EnableMouseCapture` is not sent) and no keyboard enhancement
  (`PushKeyboardEnhancementFlags` is not sent). Every key arrives in the terminal's legacy
  encoding, except what a terminal sends as `CSI u` on its own (section 3.1). Key release events
  are dropped in `app.rs` (`key.kind != KeyEventKind::Release`).
- **The prefix.** `Ctrl-a` arms the application's keys for one keystroke, for 1500 ms
  (`PREFIX_FOR`). After it: `h` / `l` (or left / right) move the focus between the side menu,
  the centre (and the split's halves) and the right column; `j` / `k` (or down / up) open the next
  or previous worktree; `1`, `2`, `3` show the `agent`, `nvim`, `shell` tab; `|` or `\` split;
  `z` zoom; `f` the files; `w` the worktree picker; `?` the help; `P` a draft pull request; `q`
  quit; `a` sends a literal `Ctrl-a` to the panel.
- **Without the prefix**, keys go to whatever has the focus. The side menu takes bare `j` / `k`,
  arrows, `Enter`, `n` (new worktree), `/` (filter) and `Esc`; the right column `j` / `k`,
  `Enter`, `Esc`; a failed agent's card `r`, `R`, `l`, `x`; its log `j`, `k`, `g`, `G`, `Ctrl-u`,
  `Ctrl-d`, page keys. The centre forwards every key to the panel's program, re-encoded as xterm
  sends it by `keys::encode` (legacy encoding, with the program's cursor-keys mode).
- So the application already has the Claude Code shape in its own regions (bare keys, `Esc` to
  back out). What hurts is the way in and out of a panel, which today only exists behind the
  prefix.

## 1. Claude Code's own keys

Sources: [interactive mode](https://code.claude.com/docs/en/interactive-mode.md) (keyboard
shortcuts), [keybindings](https://code.claude.com/docs/en/keybindings.md),
[terminal configuration](https://code.claude.com/docs/en/terminal-config.md) and
[fullscreen rendering](https://code.claude.com/docs/en/fullscreen.md).

**What it binds globally, and with which modifiers.** In the prompt (`Chat` context) and
everywhere (`Global`):

| Family | Keys (default) |
| --- | --- |
| Ctrl+letter | `C` interrupt or clear, `D` exit (both reserved, cannot be rebound), `G` editor, `L` redraw, `O` transcript, `R` history search, `S` stash, `T` to-do checklist, `B` background, `V` image paste, `Z` suspend, `J` newline, `_` undo; readline editing `A`, `E`, `K`, `U`, `W`, `Y`, `P`, `N` |
| Ctrl+X then a key | `Ctrl+K` stop subagents, `Ctrl+E` editor, `Enter` queue, `Ctrl+A` collapse the band, `Ctrl+S` send now, `Tab` move focus into the band and the panes of mods; `B`, arrows and `X` in other contexts. Each key within 3 s of the previous one. |
| Option (Meta) | `Option+P` the picker of models, `Option+T` thinking, `Option+O` fast mode; readline `Alt+B`, `Alt+F`, `Alt+D`, `Alt+Y`; `Option+Enter` newline. The docs say these need "Option as Meta" on macOS. |
| Shift | `Shift+Tab` permission modes; `Shift+Enter` newline; in fullscreen, `Shift+arrows`, `Shift+Home/End` extend a selection |
| Arrows | Up / Down history (or the cursor in a multi-line prompt); Left / Right tabs of dialogs, footer items |
| Ctrl+arrows, Meta+arrows | `Ctrl+Up/Down` and `Meta+Up/Down` scroll the diff panel's file list and jump between groups in the agent view; `Ctrl+Home/End` scroll to top / bottom |
| Cmd | `Cmd+K` clear screen, `Cmd+C` / `Cmd+V` copy and paste, only where the terminal reports super ("such as those supporting the Kitty keyboard protocol"); the docs advise `ctrl` or `meta` "for bindings you want to work everywhere" |
| Bare keys | `Esc` interrupt / close, `Esc Esc` clear or rewind, `Enter`, `Tab`, `?` on an empty prompt (help), `!` shell mode, `/` commands, `@` files, Space (push-to-talk when voice is on) |

The keybindings page also lists what it cannot use: `Ctrl+C`, `Ctrl+D`, `Ctrl+M` (Enter),
`Ctrl+[` (Esc), `Ctrl+I` (Tab), `Ctrl+H` (Backspace), and warns that `Ctrl+B` is tmux's prefix
("press twice to send") and `Ctrl+A` GNU screen's.

**How it shows them.** `?` on an empty prompt toggles a shortcut help panel; `?` in the
transcript viewer does the same there. Hints in the footer name the right key for the terminal
detected (the fullscreen page says so for the native-selection key). Bindings are configurable in
`~/.claude/keybindings.json` by context. The exact layout of the help panel was not looked at
(not verified: running claude here would touch the person's Claude state).

**What makes it feel fluid**, from the docs above:

- **Context, not modifiers.** The keybindings page lists 30 contexts (`Chat`, `Confirmation`,
  `Transcript`, `Select`, `Footer`, `DiffDialog`, `AbovePrompt`, `Scroll`…). Inside a list,
  `j` / `k` / `Enter` / `Esc` act; inside the transcript viewer `{` `}` `[` `v` `q` act. Only
  the prompt, where letters are text, needs modifiers.
- **One thing at a time, `Esc` to back out.** Dialogs, pickers and viewers take the keys until
  `Esc` closes them; "If you bind a bare letter, digit, or Space, you can still type that
  character in a text field".
- **Focus between regions is one key sequence and `Esc`.** `Ctrl+X Tab` "Move keyboard focus into
  the band", then through each open panel of a mod, then back to the prompt; `Esc` returns to
  the prompt (`abovePrompt:leave`). The footer is reached with the arrows from the prompt
  (`footer:up` / `footer:down`). This is the closest thing in Claude Code to the application's
  focus stops.
- **Pickers instead of memorised keys.** `/` opens a command menu filtered as you type, `@` a file
  picker, `Option+P` a picker of models, `Ctrl+R` history search.
- **Two-key sequences with a visible timeout.** A sequence waits 3 s and "shows a brief notice"
  when it is cancelled; unbinding every sequence on `Ctrl+X` frees `Ctrl+X` itself.

## 2. Hosts of other terminals

### 2.1 zellij

Source: [`default.kdl`](https://github.com/zellij-org/zellij/blob/d9df94cca1ebf5819a8c6e1dcdc872cb512fed91/zellij-utils/assets/keybinds/default.kdl)
and [`unlock-first.kdl`](https://github.com/zellij-org/zellij/blob/d9df94cca1ebf5819a8c6e1dcdc872cb512fed91/zellij-utils/assets/keybinds/unlock-first.kdl)
at d9df94c (latest release v0.46.0); docs at
[zellij-org.github.io e91418a](https://github.com/zellij-org/zellij-org.github.io/tree/e91418a6026cc7b02daed85f4392cb7ebe03758e/docs/src).

- **Two built-in presets.** The `default` preset has a primary modifier `Ctrl` and a secondary
  `Alt`. From the base mode, `Ctrl` + a letter enters a mode (`Ctrl p` panes, `Ctrl t` tabs,
  `Ctrl n` resize, `Ctrl s` scroll, `Ctrl o` session, `Ctrl h` move, `Ctrl b` tmux mode, `Ctrl g`
  lock, `Ctrl q` quit), and inside a mode single bare keys act. The `Alt` keys act directly, with
  no mode
  ([lines 225-241](https://github.com/zellij-org/zellij/blob/d9df94cca1ebf5819a8c6e1dcdc872cb512fed91/zellij-utils/assets/keybinds/default.kdl#L225-L241)):
  `Alt h/j/k/l` and `Alt+arrows` move the focus (left and right go on to the next tab at the
  edge), `Alt n` new split, `Alt f` floating panes, `Alt i/o` move the tab, `Alt =/-` resize,
  `Alt [ ]` layouts, `Alt p` grouping.
- **Why `locked` exists.** The presets page says `unlock-first` "is tailored to users who use
  terminal applications with keyboard shortcuts that collide with Zellij's own keybindings. In
  this preset, users 'unlock' the interface before accessing the various input modes. The modes
  themselves can then be accessed with a single character shortcut."
  ([keybinding-presets.md](https://github.com/zellij-org/zellij-org.github.io/blob/e91418a6026cc7b02daed85f4392cb7ebe03758e/docs/src/keybinding-presets.md)).
  The FAQ's first answer to "Zellij overrides certain key combinations that I use for other apps"
  is that preset ([faq.md](https://github.com/zellij-org/zellij-org.github.io/blob/e91418a6026cc7b02daed85f4392cb7ebe03758e/docs/src/faq.md)).
  It starts in `locked` (every key goes to the program), `Ctrl g` unlocks, then `p`, `t`, `n`…
  enter modes and each action drops back to `locked`. It keeps the `Alt` focus keys active in
  `locked` too
  ([lines 213-227](https://github.com/zellij-org/zellij/blob/d9df94cca1ebf5819a8c6e1dcdc872cb512fed91/zellij-utils/assets/keybinds/unlock-first.kdl#L213-L227)):
  so zellij's own answer to collisions is a one-key unlock plus `Alt` keys for moving.
- **macOS.** The FAQ: "In the Zellij terminal, the left Option key acts as Alt by default";
  elsewhere the terminal has to be set up, and the compatibility page lists "Alt button mapping on
  Mac hardware" as a known issue.
- **Known collisions**, from the bindings above against section 4: `Alt h` / `Alt l` are fish's
  man page and directory listing, `Alt j` / `Alt k` LazyVim's move line, `Alt f` fish, readline,
  Claude Code and opencode's word forward, `Alt p` Claude Code's picker of models and fish's pager;
  `Ctrl p`, `Ctrl t`, `Ctrl s`, `Ctrl o`, `Ctrl n` are each bound in claude-code or opencode.

### 2.2 tmux

Source: `man tmux` and `tmux list-keys -T root` on a private server, tmux 3.8, no configuration.

- **The prefix.** `prefix C-b`, `prefix2 None`. Keys after the prefix come from the `prefix`
  table. The prefix repeats only for bindings made with `-r`, for `repeat-time` 500 ms.
- **Root bindings.** `bind-key -n` (an alias for `-T root`) binds a key "pressed without the
  prefix key"; the man page calls a bare `c` there "not recommended". With no configuration the
  root table holds only mouse keys: no keyboard key is taken without the prefix.
- **How it keeps keys.** Everything not bound is written to the program. A literal `Ctrl-b`
  needs `send-prefix` (Claude Code's docs: "Tmux users press twice").
- **Extended keys.** `extended-keys` is `off` by default, so a program inside tmux gets legacy
  encodings only; Claude Code's docs ask for `set -s extended-keys on` (and
  `allow-passthrough on`) so Shift+Enter works.
- **Known collisions.** `Ctrl-b` is Claude Code's "background" and opencode's "background
  blocking tools", readline's back-one-character; Claude Code's docs list it as a conflict.

### 2.3 kitty's windows

Source: kitty 0.50.0 options, read from
[`kitty/options/definition.py`](https://github.com/kovidgoyal/kitty/blob/6d8f7ac14ff14757b87977cc0b177ef0274e3759/kitty/options/definition.py)
at 6d8f7ac (the parse below was also checked against the installed 0.50.0).

- 133 default shortcuts; 76 are on `kitty_mod`, which defaults to `ctrl+shift`
  (`ctrl+shift+enter` new window, `ctrl+shift+]` / `[` next and previous window,
  `ctrl+shift+1…0` window N, `ctrl+shift+right/left` tabs…).
- On macOS another 54 use `cmd`: `cmd+1…9` window N, `cmd+enter` new window, `cmd+t` / `cmd+w`
  tab, `cmd+c` / `cmd+v`, `cmd+f` search, `cmd+k` / `cmd+l` clear, `cmd+r` resize, `cmd+n`,
  `cmd+h` / `cmd+m` / `cmd+q` (hide, minimise, quit), `cmd+,`, `cmd+plus/minus/0` font,
  `cmd+up/down/page/home/end` scroll, `shift+cmd+[ ]` tabs. Also `ctrl+tab` / `ctrl+shift+tab`
  next and previous tab, on every platform.
- **How it keeps keys.** A kitty shortcut always wins; the option docs for `macos_option_as_alt`
  add that "any kitty shortcuts using Option/Alt+Key will take priority, so that any such key
  presses will not be passed to terminal programs".
- **Known collisions.** `ctrl+shift+…` collides with opencode's `ctrl+shift+a/e/d/t` and Claude
  Code's `Ctrl+Shift+-` and `Ctrl+Shift+C`; `ctrl+tab` with opencode's next session tab and
  Neovim's `<C-Tab>`. The driver's own `kitty.conf` adds `ctrl+alt+z` (stack layout).

### 2.4 Ghostty's splits

Source: [`src/config/Config.zig`](https://github.com/ghostty-org/ghostty/blob/108cacb17ed8990840cd14abfce43d8ed8503ca1/src/config/Config.zig)
at 108cacb.

- **On macOS** the split keys are on `super` (Cmd)
  ([lines 7256-7339](https://github.com/ghostty-org/ghostty/blob/108cacb17ed8990840cd14abfce43d8ed8503ca1/src/config/Config.zig#L7256-L7339)):
  `cmd+d` / `cmd+shift+d` new split, `cmd+[` / `cmd+]` previous / next split,
  `cmd+alt+arrows` go to the split in that direction, `cmd+ctrl+arrows` resize, `cmd+ctrl+=`
  equalise, `cmd+up/down` jump to prompt; `cmd+1…8` go to tab N
  ([lines 7052-7082](https://github.com/ghostty-org/ghostty/blob/108cacb17ed8990840cd14abfce43d8ed8503ca1/src/config/Config.zig#L7052-L7082)).
- **On Linux** they are on `ctrl+shift` and `ctrl+alt+arrows` (lines 6900-6941), and tab N is
  `alt+1…8`, because "On macOS we default to super but everywhere else is alt".
- On every platform `ctrl+tab` / `ctrl+shift+tab` switch tabs (lines 6833-6839) and
  `ctrl+shift+enter` (Linux) or `cmd+shift+enter` (macOS) zoom a split (line 7133).
- On macOS Ghostty also rewrites `alt+left/right` to `ESC b` / `ESC f` and `cmd+left/right` to
  `Ctrl-a` / `Ctrl-e` ("Natural text editing", lines 7400-7430).
- **Known collisions.** On Linux, `alt+digit` (tab N) is the family this research would otherwise
  propose as a fallback.

### 2.5 WezTerm

Source: [`docs/config/default-keys.md`](https://github.com/wezterm/wezterm/blob/458a100c1bf4134d36cb5c42dd90e4c8266d9e1a/docs/config/default-keys.md)
at 458a100.

- Panes are split with `ctrl+shift+alt+"` / `%`, focused with `ctrl+shift+arrows`, resized with
  `ctrl+shift+alt+arrows`. Tabs: `super+1…9` and `ctrl+shift+1…9`, `ctrl+tab`,
  `ctrl+pageup/down`.
  `super` letters copy, paste, new tab, close, hide, search, clear.
- It has a leader key only if the person configures one. The kitty keyboard protocol is
  off unless `enable_kitty_keyboard = true`
  ([enable_kitty_keyboard.md](https://github.com/wezterm/wezterm/blob/458a100c1bf4134d36cb5c42dd90e4c8266d9e1a/docs/config/lua/config/enable_kitty_keyboard.md)).
- On macOS the **left** Option is Alt and the right one composes characters, by default
  ([keyboard-concepts.md](https://github.com/wezterm/wezterm/blob/458a100c1bf4134d36cb5c42dd90e4c8266d9e1a/docs/config/keyboard-concepts.md)).

### 2.6 TUIs that hand the terminal to another program: lazygit and yazi

Neither embeds a terminal in a region of its own screen. lazygit runs a subprocess (an editor, a
shell command) by suspending itself and giving it the whole terminal, then asks to press a key
to come back (`promptToReturnFromSubprocess: true`, `editInTerminal`;
[Config.md](https://github.com/jesseduffield/lazygit/blob/5d00ff7aac915d35f01538ff51111692d47269a0/docs/Config.md)).
yazi does the same with `shell --block`
([keymap-default.toml](https://github.com/sxyazi/yazi/blob/805579e9832e315134da551bda2705c83e3c75d7/yazi-config/preset/keymap-default.toml)).
So they never need keys that work "through" a program, and they use bare keys freely: lazygit
moves between its blocks with `h` / `l`, arrows and `Tab` / `Shift-Tab`, shows its keys with `?`;
yazi switches tabs with bare `1`…`9`, `[` and `]`. They are evidence for bare single keys on the
application's own regions, not for a key that leaves a panel.

## 3. What reaches the application

### 3.1 How keys are encoded

- **Legacy encoding** (every terminal by default): `Alt+key` is `ESC` then the key; `Ctrl+letter`
  is a C0 control byte; arrows and function keys are `CSI` sequences that carry the modifiers
  (`CSI 1;5A` is Ctrl+Up). It is ambiguous: `Ctrl+I` is Tab, `Ctrl+M` Enter, `Ctrl+[` Esc, and
  `Ctrl` on a digit gives `Ctrl+2` NUL, `Ctrl+3` ESC, `Ctrl+4…7` the bytes 28 to 31, `Ctrl+8` DEL,
  and `Ctrl+1` / `Ctrl+9` just the digit
  ([kitty's spec, "Legacy ctrl mapping"](https://github.com/kovidgoyal/kitty/blob/6d8f7ac14ff14757b87977cc0b177ef0274e3759/docs/keyboard-protocol.rst#L675)).
  `Ctrl+Tab` and `Ctrl+Shift+letter` have no legacy encoding at all. The super / Cmd modifier has
  none either.
- **The kitty keyboard protocol**
  ([spec](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)). A program pushes flags with
  `CSI > flags u`. Flag 1 (disambiguate) reports `Esc`, `alt+key`, `ctrl+key`, `ctrl+alt+key`,
  `shift+alt+key` as `CSI code;mods u`, keeping `Enter`, `Tab` and `Backspace` legacy. Modifiers
  include super (8), hyper and meta. Flag 8 (report all keys) also encodes keys that produce text.
- **kitty sends `CSI u` even without the protocol** for combinations the legacy encoding cannot
  express: "Any other combination of modifiers with these keys is output as the appropriate
  `CSI u` escape code" ([Legacy text keys](https://github.com/kovidgoyal/kitty/blob/6d8f7ac14ff14757b87977cc0b177ef0274e3759/docs/keyboard-protocol.rst#L553)),
  and functional keys without a legacy encoding likewise. The code agrees: in
  [`encode_key`](https://github.com/kovidgoyal/kitty/blob/6d8f7ac14ff14757b87977cc0b177ef0274e3759/kitty/key_encoding.c#L398-L428),
  a key with modifiers the legacy encoder does not handle (anything but Shift, Alt, Ctrl and
  Ctrl+Alt) falls through to `serialize(…, 'u')`. So in kitty `Cmd+a` (no kitty shortcut)
  should reach a program as `CSI 97;9u` by default. Source reading only: not pressed in kitty here,
  and whether macOS hands kitty a Cmd key with no text attached (kitty sends a key that carries
  text as that text) was not checked.
- **Ghostty on macOS does not encode text for Cmd keys in legacy mode**: "If we are on macOS,
  command+keys do not encode text" ([`key_encode.zig` lines 536-547](https://github.com/ghostty-org/ghostty/blob/108cacb17ed8990840cd14abfce43d8ed8503ca1/src/input/key_encode.zig#L536-L547)).
  Whether its kitty-protocol path reports super for an unbound Cmd key was not verified.
- **iTerm2** has a "Report modifiers using CSI u" setting the docs call "not recommended.
  Applications should use the Kitty keyboard protocol instead", and lets modifiers act as
  Hyper / Meta / Super "only when running a program that uses the Kitty keyboard protocol"
  ([Profiles > Keys](https://iterm2.com/documentation-preferences-profiles-keys.html)). So Cmd
  reaches a TUI in iTerm2 only after the person remaps it, and only under the protocol.
- **Terminal.app** has no setting that sends Cmd to a program and does not implement the kitty
  protocol (no source says it does; Claude Code's docs leave it out of the list of terminals that
  report super). Not verified beyond that absence.

### 3.2 Option as Alt on macOS

| Terminal | Setting | Default |
| --- | --- | --- |
| kitty | `macos_option_as_alt` (`no`, `left`, `right`, `both`) | `no`: Option composes characters (`Option+h` types `˙`) ([definition.py line 3524](https://github.com/kovidgoyal/kitty/blob/6d8f7ac14ff14757b87977cc0b177ef0274e3759/kitty/options/definition.py#L3524)); the driver's `kitty.conf` does not set it |
| Ghostty | `macos-option-as-alt` | unset, which means `true` on the U.S. Standard and U.S. International layouts and `false` otherwise ([Config.zig line 3529](https://github.com/ghostty-org/ghostty/blob/108cacb17ed8990840cd14abfce43d8ed8503ca1/src/config/Config.zig#L3529)) |
| iTerm2 | Profiles > Keys > Left / Right Option key: Normal, Meta, `Esc+` | the docs recommend `Esc+` ("For most users, selecting '+Esc' here is the right choice"); Claude Code's docs say to set it. The default being Normal was not verified. |
| Terminal.app | Profiles > Keyboard > "Use Option as Meta key" ([Apple](https://support.apple.com/guide/terminal/change-profiles-keyboard-settings-trmlkbrd/mac)) | off (Claude Code's `/terminal-setup` turns it on, which implies off; not verified from Apple) |
| WezTerm | `send_composed_key_when_left/right_alt_is_pressed` | left Option is Alt, right composes |

Claude Code's terminal page says: "On macOS, most terminals do not send Option as a modifier by
default, so these shortcuts do nothing until you enable it." One exception in its own table:
`Option+T` "Works on macOS without configuring Option as Meta", which suggests Claude Code
recognises the character Option+T composes (`†` on a U.S. layout) as the key; how it does so was
not read. The same trick would let the application take `Option+1…9` in kitty's default setting
by matching what they compose (`¡ ™ £ ¢ ∞ § ¶ • ª` on a U.S. layout, not checked here), at the
cost of those characters never being typed into a panel and of depending on the keyboard layout.

### 3.3 What crossterm gives (crossterm 0.29.0)

Source: `src/event.rs`, `src/event/sys/unix/parse.rs`, `src/terminal/sys/unix.rs` of the crate
in the cargo registry ([docs.rs source](https://docs.rs/crate/crossterm/0.29.0/source/src/event.rs)).

- `PushKeyboardEnhancementFlags(flags)` writes `CSI > flags u`; `PopKeyboardEnhancementFlags`
  undoes it. Flags: `DISAMBIGUATE_ESCAPE_CODES`, `REPORT_EVENT_TYPES`, `REPORT_ALTERNATE_KEYS`,
  `REPORT_ALL_KEYS_AS_ESCAPE_CODES`; associated text (flag 16) "is not yet supported". Its doc
  lists kitty, foot, WezTerm, alacritty and others as supporting the protocol; on Windows the
  command returns `Unsupported`.
- `supports_keyboard_enhancement()` sends `CSI ? u` then `CSI c` and returns true only if the
  first is answered.
- `KeyModifiers` carries `SHIFT`, `CONTROL`, `ALT`, `SUPER`, `HYPER`, `META`, parsed from `CSI u`.
- **Measured** with the probe (bytes written by the private tmux, crossterm parsing them), the
  same with and without pushing flag 1 (tmux 3.8 did not answer `CSI ? u`, so the push had no
  effect there):

  | Bytes in | crossterm event |
  | --- | --- |
  | `ESC h` | `Char('h')` + `ALT` |
  | `˙` (what kitty sends for Option+h by default) | `Char('˙')`, no modifier |
  | `CSI 97;9u` / `CSI 104;9u` | `Char('a')` / `Char('h')` + `SUPER` |
  | `CSI 49;5u` | `Char('1')` + `CONTROL` |
  | `ESC` alone (legacy `Ctrl+3`) | `Esc` |
  | NUL (legacy `Ctrl+2`, `Ctrl+Space`) | `Char(' ')` + `CONTROL` |
  | `0x1d` (legacy `Ctrl+]`, `Ctrl+5`) | `Char('5')` + `CONTROL` |
  | `CSI 9;5u` | `Tab` + `CONTROL` |
  | `CSI 1;5A` | `Up` + `CONTROL` |
  | `ESC 1` | `Char('1')` + `ALT` |
  | `CSI <0;10;5M`, `CSI <64;10;5M`, `CSI <4;10;5M` | `Down(Left)`, `ScrollUp`, `Down(Left)` + `SHIFT` at column 9, row 4 |

  So crossterm parses `CSI u` keys, super included, whether or not the application pushed any
  flags; what the push changes is what the terminal sends.

### 3.4 Which key families arrive, per terminal

"Yes" means the program receives a distinct key with the modifier, with that terminal's defaults
and no protocol pushed; "proto" means only once the program pushes the kitty protocol. Rows on
macOS. Claims for Ghostty, iTerm2 and Terminal.app come from their docs and source, not from
pressing keys.

| Family | kitty | Ghostty | iTerm2 | Terminal.app |
| --- | --- | --- | --- | --- |
| Alt+letter | no (composes) unless `macos_option_as_alt` | yes on U.S. layouts | no unless Option is `Esc+` | no unless "Use Option as Meta" |
| Alt+digit | as Alt+letter | as Alt+letter | as Alt+letter | as Alt+letter |
| Alt+arrows | yes (arrows carry the modifier) | rewritten to `ESC b` / `ESC f` (left / right) | yes (not verified) | not verified |
| Ctrl+arrows | yes, if macOS does not take them first (Mission Control uses Ctrl+Left/Right/Up/Down by default; not verified on Apple's pages) | same | same | same |
| Ctrl+digit | only `1` and `9` as digits; `2…8` turn into NUL, ESC and control bytes; proto: yes | same | same | same, no proto |
| Ctrl+Tab | taken by kitty (next tab) | taken by Ghostty | taken by iTerm2 (not verified) | taken (not verified) |
| Shift+arrows | yes | yes | yes | yes |
| F1-F12 | yes, but macOS sends media keys unless Fn is held or the system setting is on | same | same | same |
| Cmd+key (super) | yes as `CSI u` for keys kitty does not bind (source) | not in legacy mode; with proto not verified | only after remapping Cmd to Super, and only under proto | never |
| Ctrl+letter (as a prefix) | yes | yes | yes | yes |

## 4. Collision table

Who already uses each family by default. Sources: Claude Code's
[keybindings](https://code.claude.com/docs/en/keybindings.md); opencode's
[`keybind.ts`](https://github.com/anomalyco/opencode/blob/33123b1c42722bc81bcbfc4e4396d035c34a6647/packages/tui/src/config/keybind.ts)
at 33123b1 (its leader is `ctrl+x`, line 41); LazyVim's
[`keymaps.lua`](https://github.com/LazyVim/LazyVim/blob/5c26fac21fd3896024a23f2dbc62b722f9a731db/lua/lazyvim/config/keymaps.lua)
and [`editor.lua`](https://github.com/LazyVim/LazyVim/blob/5c26fac21fd3896024a23f2dbc62b722f9a731db/lua/lazyvim/plugins/editor.lua)
at 5c26fac; Neovim 0.12.6's `:help index`; fish 4.9.3's `bind` and
[`interactive.rst`](https://github.com/fish-shell/fish-shell/blob/4da2a20801914288ded72e2614dd8e0b6e2ccd1a/doc_src/interactive.rst);
bash's [readline numeric arguments](https://www.gnu.org/software/bash/manual/html_node/Numeric-Arguments.html).

| Family | claude-code | opencode | Neovim / LazyVim | readline / fish / zsh | Arrives (kitty, Ghostty, iTerm2, Terminal.app) |
| --- | --- | --- | --- | --- | --- |
| Alt+letter | `B F D Y P T O`, `Option+Enter` | `A E F B D`, `alt+shift+…` | LazyVim `A-j` / `A-k` move lines; plugins' pickers `a-c`, `a-t` | readline `B F D Y U C T`…; fish `B C D E F H L O P S T U V W Y` | only with Option as Alt (Ghostty on by default on U.S. layouts) |
| Alt+h/j/k/l | none | none | `A-j`, `A-k` | fish `alt-h` man page, `alt-l` list directory | as above |
| Alt+digit | none found | none found | none in LazyVim's keymaps | readline digit-argument (`M-0`…`M-9`) in bash and zsh's emacs keys; fish: none bound | as above; Ghostty on Linux takes them for tabs |
| Alt+arrows | `Meta+Up/Down` (diff list, agent groups) | `alt+up/down` session tabs, diff files; `alt+left/right` words | none | fish history token search, word moves | yes (Ghostty rewrites left / right) |
| Ctrl+arrows | `Ctrl+Up/Down` (diff list, agent groups) | `ctrl+left/right` words | LazyVim resize windows; Neovim word moves | fish token moves | yes, unless macOS takes them |
| Ctrl+digit | none | `ctrl+1…0` session tabs | none | none | proto only |
| Ctrl+Tab | none | next session tab | `<C-Tab>` (`g<Tab>`) | none | taken by the terminals |
| Shift+arrows | extend selection (fullscreen) | select in input | word moves, page up / down | fish big-word moves | yes |
| F-keys | none found | `F2` / `Shift+F2` cycle recent models | `F1` help | fish `F1` man page | yes, with Fn on a Mac keyboard |
| Cmd / super | `Cmd+K`, `Cmd+C`, `Cmd+V` (where reported) | `super+z`, `super+shift+z`, `super+a` | none by default | none | kitty only, by default |
| Ctrl+a prefix (today) | start of line | start of line, the provider list in its picker of models, new worktree in a dialog | increment number | start of line | yes |
| Ctrl+b | background | background | page up | back one character | yes |
| Ctrl+g | external editor | first message | file info | abort | yes |
| Ctrl+Space | none found | none found | LazyVim (flash) treesitter selection | readline set-mark | yes (as NUL) |
| Ctrl+] | none found | none found | jump to tag / definition | bash character search | yes (crossterm reads it as `Ctrl+5`) |
| Ctrl+\\ | none found | none found | `Ctrl-\ Ctrl-n` leaves terminal mode | SIGQUIT in cooked mode | yes (raw mode only) |

What is left with nothing inside a panel using it: **Cmd letters kitty leaves free**
(`a`, `b`, `d`, `e`, `g`, `i`, `j`, `o`, `p`, `s`, `u`, `x`, `y`, `z`; but opencode uses
`super+a` and `super+z`, so `b d e g i j o p s u x y`), **Alt+digit** outside bash and zsh's
digit argument, and the **F-keys from F3**. None of them arrives in all four terminals.

## 5. Mouse

### 5.1 What crossterm reports

`EnableMouseCapture` turns on modes 1000 (press and release), 1002 (drag), 1003 (any motion),
1015 and 1006 (SGR) in one go. Events are `Down`, `Up`, `Drag` of Left / Middle / Right, `Moved`,
`ScrollUp/Down/Left/Right`, with column, row and `SHIFT` / `ALT` / `CONTROL` modifiers; there is
no super bit in the mouse protocol. Its doc warns that some terminals report no button on `Up` or
`Drag`, and that macOS reports Ctrl+click as a right click. The probe parsed an SGR left click,
wheel and Shift+click as expected (3.3).

The application does not enable it today, so the terminal handles every click and drag: a drag
selects text across the side menu and every panel at once.

### 5.2 What the panels' programs expect

- **claude-code**, in fullscreen rendering, captures the mouse: click to place the cursor, click
  options and collapsed results, drag to select (it copies on release), wheel to scroll
  ([fullscreen, "Use the mouse"](https://code.claude.com/docs/en/fullscreen.md)). The earlier
  research recorded claude 2.1.319 and opencode 2.2.5 switching on `?1000h ?1002h ?1003h ?1006h`
  (`docs/research/terminal-application.md` on the branch `research/terminal-application`,
  section 1.1). `CLAUDE_CODE_DISABLE_MOUSE=1` turns it off.
- **Neovim**: `'mouse'` defaults to `nvi`; "To temporarily disable mouse support, hold the shift
  key while using the mouse" (`:help 'mouse'`, 0.12.6).
- **The panel's emulator already tracks it**: `vt100` 0.18.1 exposes `mouse_protocol_mode()`
  (`None`, `Press`, `PressRelease`, `ButtonMotion`, `AnyMotion`) and `mouse_protocol_encoding()`
  per screen, so the application knows, per panel, whether to forward a click and how to encode
  it.

### 5.3 How hosts decide who gets a click or a scroll

- **tmux** (root mouse bindings, 3.8): a click on a split selects it and is also sent to the
  program (`send-keys -M` after selecting it). A drag goes to the program when it asked for the
  mouse (`mouse_any_flag`), else starts tmux's copy mode. The wheel goes to the program when it
  is on the alternate screen or asked for the mouse, else enters copy mode and scrolls tmux's
  history. Claude Code's docs say wheel scrolling needs `set -g mouse on` in tmux.
- **zellij**: with `app_first=true` (the default for clicks and scrolls), a click on the focused
  split goes to a program that asked for mouse events "instead of to the binding", and scroll
  bindings send the wheel to programs that asked for it or use the full screen
  ([mouse-bindings.md](https://github.com/zellij-org/zellij-org.github.io/blob/e91418a6026cc7b02daed85f4392cb7ebe03758e/docs/src/mouse-bindings.md)).
  `mouse_click_through` (default false): the first click on another split only focuses it.
  `Super` "cannot be used, because terminals do not report it for mouse events".
- **Selecting text past a program that captures the mouse**: zellij lets the terminal handle it
  while Shift is held ([compatibility.md](https://github.com/zellij-org/zellij-org.github.io/blob/e91418a6026cc7b02daed85f4392cb7ebe03758e/docs/src/compatibility.md));
  kitty's default `mouse_map shift+left press grabbed mouse_selection normal` ("Start selecting
  text even when grabbed"); Ghostty's `mouse-shift-capture = false` ("the shift key is not sent
  with the mouse protocol and will extend the selection",
  [line 973](https://github.com/ghostty-org/ghostty/blob/108cacb17ed8990840cd14abfce43d8ed8503ca1/src/config/Config.zig#L973));
  WezTerm "You can bypass the mouse reporting capture by holding down the SHIFT key"
  ([mouse.md](https://github.com/wezterm/wezterm/blob/458a100c1bf4134d36cb5c42dd90e4c8266d9e1a/docs/config/mouse.md));
  iTerm2: mouse reporting "can be temporarily disabled by holding down Option"
  ([Profiles > Terminal](https://iterm2.com/documentation-preferences-profiles-terminal.html));
  Terminal.app: `Fn` (Claude Code's fullscreen page, "Keep native text selection").
- The terminal's native selection does not know the application's panels: a Shift-drag across
  the side menu and a panel selects both. kitty's rectangle selection (`ctrl+alt` drag) limits
  that to columns.

## 6. What this means for the tickets

These are options, not rulings; each ticket's grilling decides.

### #199: which keys, prefix or not

| Option | Fluid | Works in all four terminals | Collisions | Notes |
| --- | --- | --- | --- | --- |
| A. Keep `Ctrl-a` as is | no (the driver's complaint) | yes | start of line in Claude Code, opencode, readline, fish; increment in Neovim | today's scheme |
| B. A prefix that is free, plus a sticky mode (zellij `unlock-first`) | better: one key, then several single keys until `Esc` | yes | no `Ctrl` key is free in all four programs (table above) | still a prefix |
| C. Alt family as root keys (zellij's default) | yes | only with Option as Alt (kitty, iTerm2, Terminal.app need a setting) | `Alt+h/j/k/l` collide with fish and LazyVim; Alt letters with readline, Claude Code, opencode | |
| D. Cmd family via kitty | yes | kitty only (Ghostty not verified; iTerm2 only remapped; Terminal.app never) | none inside the panels for the free letters; macOS and kitty own the others | needs a fallback |
| E. Bare keys on the application's regions, one reserved key to leave a panel | yes, Claude Code's shape | depends on the reserved key | only the reserved key | the reserved key can be D with B as fallback |

**Recommended: E, with the reserved keys taken from D in kitty and from `Alt+digit` plus a
fallback prefix elsewhere.** Concretely, to prototype in #203: in the side menu and the right
column every key is the application's (as today, plus `1`…`9` and `?`); inside a panel only
`Cmd+…` (kitty) or the fallback leave it. Probe at startup, with
`supports_keyboard_enhancement()`, whether the terminal speaks the kitty protocol, and show in
the status bar which keys work in this terminal. Two things to measure before ruling: that kitty
really sends `CSI u` with super for the free Cmd letters without the protocol pushed, and what
Ghostty sends for an unbound Cmd key once the protocol is pushed. A side effect to weigh: pushing
the protocol changes what the application receives for every key, and `keys::encode` would then
have to re-encode keys the program inside did or did not ask for in that form.

### #200: focus stops and jumps

- **Option 1, three stops** (side menu, centre, right column) and a split's halves inside the
  centre; `Enter` or a click enters a panel; the reserved key leaves it to the side menu. Mirrors
  Claude Code's `Ctrl+X Tab` / `Esc`.
- **Option 2, every panel a stop in a cycle** (like zellij's `Alt` focus keys or kitty's
  next-window). More keys to reserve (a direction or a cycle).
- **Jumps**: digits on the side menu (yazi's `1`…`9`, opencode's `<leader>1`…), `1` / `2` / `3`
  for the `agent` / `nvim` / `shell` tab, and the worktree picker (`w`), which is Claude Code's
  picker pattern.

Recommended: option 1, since it needs one reserved key and keeps every other key bare.

### #201: mouse

- **Option 1, no capture** (today): native selection everywhere, but no click to focus, and a
  drag selects across panels.
- **Option 2, capture, focus on click, forward to programs that asked** (tmux's and zellij's
  rule): click focuses (and, like tmux, is also delivered, or like zellij's default, only
  focuses); the wheel goes to a panel whose program asked for the mouse or is on the alternate
  screen; Shift-drag (Option in iTerm2, Fn in Terminal.app) stays the terminal's native
  selection. The application must re-encode each event for the panel's own mouse mode and
  encoding (vt100 tracks both) and translate coordinates.
- **Option 3, capture and do our own selection** per panel, copied with OSC 52 or `pbcopy` (Claude
  Code's way). Clean selection per panel, at the cost of building selection.

Recommended: option 2, with focus-only on the first click (zellij's default) so a click never
lands by surprise in the agent; option 3 later if selecting across panels proves a nuisance.

## Not verified

- Which bytes kitty 0.50.0 sends for an unbound `Cmd+letter` without the protocol pushed (read in
  the spec and source; not pressed: that would need the person's kitty).
- What Ghostty sends for an unbound Cmd key once the kitty protocol is pushed.
- iTerm2's default Option setting ("Normal" assumed) and whether `Ctrl+Tab` and `Alt+arrows`
  reach a program there; Terminal.app's defaults for the same.
- That macOS takes `Ctrl+arrows` for Mission Control and Spaces by default (not found on Apple's
  own pages; third-party pages say so).
- Neovim's behaviour for an unmapped `<M-digit>`.
- How Claude Code recognises `Option+T` without Option as Meta, and the characters Option+digit
  composes on each layout.
- The layout of Claude Code's `?` help panel (not opened, to keep the person's Claude state
  untouched).
- zellij, Ghostty, WezTerm and iTerm2 were not run; their claims rest on source and docs only.
