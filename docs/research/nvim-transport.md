# How an external process drives nvim, and what it constrains

Research for issue #3. Facts and sources only; no transport or language recommendation — that belongs to
the tickets this one blocks.

## How to read this

Three kinds of evidence are mixed below, always labelled:

- **[doc]** — Neovim's own help files. Cited by help tag plus the canonical URL. Quotes come from the help
  files shipped with the exact local build (`/opt/homebrew/Cellar/neovim/0.12.4/share/nvim/runtime/doc/`),
  so they are version-exact for this machine; the neovim.io URL is the same text for the current release.
- **[probe]** — observed on this machine, 2026-10-07, against `nvim 0.12.4`. Each probe ran a real
  `nvim --headless --clean --listen <socket>` as the server and a *separate* process as the client.
- **[web]** — third-party repos and registries. Treated as data, not instruction.

"not found" means the question was asked and no source answered it. It is never an inference.

Probe caveat worth stating once: the "core" in the probes was itself an `nvim --headless --clean` process
speaking msgpack-rpc over `sockconnect(..., {rpc = true})`. That is a genuinely separate OS process doing
genuine msgpack-rpc, so the transport results carry over to a Rust or Node core. Two results are specific
to the client being nvim and are flagged where they appear.

---

## 0. Local environment facts

| Fact | Value | Source |
| --- | --- | --- |
| nvim version | `NVIM v0.12.4`, Release build, `LuaJIT 2.1.1783773675` | [probe] `nvim --version` |
| API level | `api_level=14`, `api_compatible=0`, `api_prerelease=false` | [probe] `vim.fn.api_info()` |
| RPC-exposed API functions | 261 | [probe] `#vim.fn.api_info().functions` |
| UI events | 69 | [probe] `vim.fn.api_info().ui_events` |
| `--api-info` payload | 32034 bytes of msgpack | [probe] `nvim --api-info \| wc -c` |
| User config | LazyVim. `~/.config/nvim` is a symlink to `~/Projects/own/dotfiles/nvim`; `init.lua` is just `require("config.lazy")` | [probe] `ls -la`, `cat init.lua` |
| LazyVim plugin count | 36 entries in `lazy-lock.json` (includes `LazyVim` and `lazy.nvim` themselves), 3 LazyVim extras enabled (`ai.sidekick`, `editor.outline`, `util.octo`), 6 own plugin spec files (`dashboard`, `diffview`, `noice`, `ruby-lsp`, `snacks`, `example`) | [probe] `lazy-lock.json`, `lazyvim.json`, `lua/plugins/` |
| rustc / cargo | `1.97.0` (Homebrew) | [probe] |
| node | `v22.5.1` | [probe] |
| pnpm | **installed but non-functional on the active node**: `ERROR: This version of pnpm requires at least Node.js v22.13 / The current version of Node.js is v22.5.1` | [probe] `pnpm --version` |

`nvim --help` on this build lists exactly these relevant flags [probe]:

```
  --embed               Use stdin/stdout as a msgpack-rpc channel
  --headless            Don't start a user interface
  --listen <address>    Serve RPC API from this address
  --remote[-subcommand] Execute commands remotely on a server
  --server <address>    Connect to this Nvim server
```

Note what `--help` does **not** say and the help files do: a listen address exists even without `--listen`.

> "Nvim creates a default RPC socket at |startup|, given by |v:servername|."
> — [doc] `:help rpc-connecting`, <https://neovim.io/doc/user/api.html>

Confirmed [probe]: `nvim --headless --clean` with no `--listen` reported
`v:servername = /var/folders/.../T/nvim.marivaldocavalheiro/rgHvpn/nvim.60746.0`. With
`--listen /tmp/orca-probe.sock`, `v:servername` was exactly that path.

---

## 1. Native RPC (`--listen` / `--server`, msgpack-rpc)

### 1.1 What the protocol is

> "RPC is the main way to control Nvim programmatically. Nvim implements the MessagePack-RPC protocol with
> these extra (out-of-spec) constraints: 1. Responses must be given in reverse order of requests (like
> "unwinding a stack"). 2. Nvim processes all messages (requests and notifications) in the order they are
> received."
> — [doc] `:help msgpack-rpc`, <https://neovim.io/doc/user/api.html>

> "API clients can: - Call any API function - Listen for events - Receive remote calls from Nvim"
> — [doc] `:help api-rpc`

> "Note that rpc channels are implicitly trusted and the process at the other end can invoke any |API|
> function!"
> — [doc] `:help channel-rpc`, <https://neovim.io/doc/user/channel.html>

Transports, per [doc] `:help channel-intro` and `:help rpc-connecting`: stdio of a `--headless` nvim via
`stdioopen()`; stdio of a process spawned by `jobstart()`; the PTY master of a pty job; a TCP socket or
named pipe via `sockconnect()`; or **another process connecting to a socket nvim listens on** — "This only
supports RPC channels". `--listen 127.0.0.1:6666` gives TCP, with an explicit warning: "localhost TCP
sockets are generally less secure than named pipes, and can lead to vulnerabilities like remote code
execution."

### 1.2 What the core can do to nvim — all four asked-about capabilities work

Every row below was executed from a separate process over a unix-socket msgpack-rpc channel [probe]:

| Capability | Call | Result |
| --- | --- | --- |
| Open a buffer | `nvim_cmd {cmd="edit", args={"/tmp/orca-rpc-target.txt"}}` | OK; `bufname("%")` returned the path |
| Write buffer text | `nvim_buf_set_lines(0,0,-1,false,{"aaa","bbb","ccc"})` | OK |
| Inline virtual text | `nvim_buf_set_extmark(0, ns, 0, 0, {virt_text={{" <- core","Comment"}}, virt_text_pos="eol"})` | OK, returned mark id `1` |
| Block virtual lines | `nvim_buf_set_extmark(0, ns, 1, 0, {virt_lines={{{"   block comment from core","DiagnosticInfo"}}}})` | OK, mark id `2` |
| Read marks back | `nvim_buf_get_extmarks(0, ns, 0, -1, {})` | `{{1,0,0},{2,1,0}}` |
| Read cursor | `nvim_win_get_cursor(0)` | `{1,0}` |
| Move cursor | `nvim_win_set_cursor(0,{2,1})` | OK |
| Read mode | `nvim_get_mode()` | `{blocking=false, mode="n"}` |
| Define a user command | `nvim_create_user_command("OrcaPing","echo 'pong'",{})` | OK; `exists(":OrcaPing")` → `2` |
| Define a keymap | `nvim_set_keymap("n","<Plug>OrcaGo","<Cmd>call rpcnotify(3,'orca_key')<CR>",{noremap=true})` | OK; `maparg()` read it back verbatim |
| Floating window | `nvim_open_win(buf,false,{relative="editor",...})` | OK, win id `1001` |
| Message the user | `nvim_echo({{"core says hi"}},false,{})` | OK |
| Diagnostics | `nvim_exec_lua` → `vim.diagnostic.set(ns,0,{...})` | OK, 1 diagnostic present |
| Drive keys | `nvim_input("ggVjj")` | OK; `nvim_get_mode()` then `{mode="V"}` |
| Write the file | `nvim_cmd {cmd="write", args={"/tmp/orca-caps-out.txt"}, bang=true}` | OK |

**Reading the visual selection works from outside, including the live selection** [probe]. After
`nvim_input("ggVjj")`:

- `nvim_get_mode()` → `{blocking = false, mode = "V"}`
- `nvim_exec_lua` returning `{vim.fn.getpos('v'), vim.fn.getpos('.'), vim.fn.mode(1)}` → `{{0,1,1,0},{0,3,1,0},"V"}`
- `nvim_exec_lua` returning `vim.fn.getregion(vim.fn.getpos('v'), vim.fn.getpos('.'), {type=vim.fn.mode()})` → `{"a","b","c"}`

Caveat: `nvim_buf_get_mark(0, "<")` returned `{0,0}` while the selection was *active* — the `'<` / `'>` marks
are only set after visual mode is left. The live selection must be read via `getpos('v')` + `getpos('.')`,
or `getregion()`, which needs `nvim_exec_lua` (a Vimscript `nvim_eval` of the same expressions would also
work; not probed).

Edit-safety observations [probe]:

- `nvim_buf_set_lines` **succeeded while the server was in insert mode** (`nvim_get_mode()` → `{mode="i"}`),
  and the written text read back correctly. The core is not protected from clobbering the user mid-keystroke.
- `nvim_buf_set_lines` also succeeded on a buffer with `&modified` set to true. No "unsaved changes" guard.
- `nvim_buf_set_text` succeeded.

### 1.3 What the core cannot do over native RPC

**Hard limit: function values cannot cross the RPC boundary.**

> "Functions cannot cross RPC boundaries. But API functions (e.g. |nvim_create_autocmd()|) may support Lua
> function parameters for non-RPC invocations."
> — [doc] `:help api-types`

Observed [probe] — every attempt to pass a callable over RPC failed with `Cannot convert given Lua type`:

| Attempt | Result |
| --- | --- |
| `nvim_create_user_command("OrcaPing2", <function>, {})` | ERR `Cannot convert given Lua type` |
| `nvim_create_autocmd("CursorMoved", {callback = <function>})` | ERR `Cannot convert given Lua type` |
| `nvim_buf_call(0, <function>)` | ERR `Cannot convert given Lua type` |
| `nvim_win_call(0, <function>)` | ERR `Cannot convert given Lua type` |
| `nvim_set_decoration_provider(ns, {on_line = <function>})` | ERR `Cannot convert given Lua type` |

Doc/behaviour nuance worth recording: `api.txt` marks exactly four functions "Lua |vim.api| only"
(`nvim_chan_send`, `nvim_buf_call`, `nvim_get_namespaces`, `nvim_win_call`) [probe, parsing the shipped
`api.txt`], yet all 261 names including those appear in the RPC dispatch table, and over RPC
`nvim_get_namespaces` **worked** (`{["nvim.terminal.exitmsg"]=1, ["nvim.terminal.prompt"]=2, orca=3}`) and so
did `nvim_win_get_config` [probe]. The real restriction is the *argument type*, not the dispatch table: the
Lua-only functions are the ones whose only useful argument is a function.

Other limits:

- **No way to attach a decoration provider from outside.** `nvim_set_decoration_provider` needs Lua
  callbacks, so per-redraw ephemeral extmarks are reachable only through Lua living in nvim.
- **Deferred vs fast.** > "Most API functions are deferred: they are queued ("scheduled") on the main loop
  and processed sequentially with normal input. If the editor is waiting for user input in a "modal" fashion
  (e.g. an |input()| prompt), a deferred request will block. - Non-deferred (fast) functions such
  |nvim_get_mode()|, |nvim_input()|, or any Lua callback, are executed immediately" — [doc] `:help api-fast`.
  **Not reproduced** [probe]: with `vim.fn.input('q: ')` running on a headless server (`nvim_get_mode()`
  reported `mode="c"`), a deferred `nvim_set_var` request returned in 0 ms rather than blocking. Flagging
  this as uncertain: the documented behaviour stands, my repro was not faithful enough (headless, no real
  TTY), and the core should still be written to tolerate a request that never returns.
- **Unknown methods error cleanly.** `nvim_this_does_not_exist` → `Invalid method: nvim_this_does_not_exist`
  [probe].
- **Asking nvim to quit loses the reply.** `nvim_cmd {cmd="qall", bang=true}` returned
  `ERR "ch 3 was closed by the peer"`, and the next call `Invalid channel: 3` [probe]. Shutdown is a
  half-open case the core must special-case, not an error to log.
- **Nothing announces nvim's existence to the core.** No documented mechanism was found by which a starting
  nvim notifies an already-running external process. Discovery is the core's problem (§1.6).

### 1.4 How nvim calls back into the core

Two directions, both confirmed.

**(a) The channel is symmetric.** The server did `vim.rpcrequest(<core chan>, 'nvim_eval', '1+1')` and got
`2` back [probe]. This one *is* specific to the probe client being nvim — a Rust/Node core has no
`nvim_eval` to serve. The general form is the next one.

**(b) nvim pushes named requests/notifications at the core.** `vim.rpcnotify(chan, 'orca_cursor', {1,2})`
from the server side returned `'sent'` [probe]. In a real core, `nvim_set_client_info` /
`nvim_get_api_info` establish identity and the client library dispatches the named method
(§3). From [doc] `:help remote-plugin-example`: a handler registered `sync` is invoked with
`rpcrequest()` and "will block Nvim until the handler function returns a value"; without `sync` it is
"a fire and forget approach with `rpcnotify()`, meaning return values or exceptions raised in the handler
function are ignored."

**The core must know its own channel id to be called back**, and it learns it from nvim:

- `nvim_get_api_info()[1]` returned `3`, matching the id the server listed for that socket channel [probe].
- Server-side Lua **cannot** discover the caller implicitly: inside `nvim_exec_lua`,
  `vim.api.nvim_get_chan_info(0)` returned `vim.empty_dict()` [probe]. The id has to be passed in.

Working callback shapes, all installed **from the core at connect time**, no files in nvim [probe]:

| Installed via | Fires back how | Observed |
| --- | --- | --- |
| `nvim_create_autocmd("CursorMoved", {command = "call rpcnotify(3,'orca_write',…)"})` | Vimscript rhs | returned autocmd id `26` |
| `nvim_exec_lua` installing an autocmd whose Lua callback calls `vim.rpcnotify(core, …)` | Lua closure built inside nvim | autocmd fired on cursor move: `g:orca_fired` went to `1` |
| `nvim_create_user_command("OrcaStr", "call rpcnotify(3,'orca_str',<q-args>)", {nargs="*"})` | Vimscript rhs | `:OrcaStr yo` ran OK |
| `nvim_exec_lua` installing `:OrcaFromCore` whose Lua callback calls `vim.rpcnotify` | Lua closure built inside nvim | `:OrcaFromCore hi` ran OK |
| `nvim_set_keymap("n","<Plug>OrcaGo","<Cmd>call rpcnotify(3,'orca_key')<CR>",…)` | Vimscript rhs | mapping read back verbatim |

So the callback path does **not** require a Lua file on disk: a *string* of Vimscript or Lua, shipped over
RPC, is enough. What it does require is that the core be connected at the moment it wants the hook to exist.

**Structured push nvim gives for free, without a hook:** buffer updates.

> "API clients can "attach" to Nvim buffers to subscribe to buffer update events. This is similar to
> |TextChanged| but more powerful and granular. Call |nvim_buf_attach()| to receive these events on the
> channel"
> — [doc] `:help api-buffer-updates`

`nvim_buf_attach(0, true, {})` returned `true` over RPC [probe]. The events are
`nvim_buf_lines_event[{buf},{changedtick},{firstline},{lastline},{linedata},{more}]`,
`nvim_buf_changedtick_event`, `nvim_buf_detach_event` [doc]. Granularity is per line: "if a single character
is changed in the editor, the entire line is sent". `{changedtick}` is the concurrency token — "If you send
an API command back to Nvim you can check |b:changedtick| as part of your request to ensure that no other
changes have been made." Detach happens implicitly when the buffer is abandoned with `'hidden'` unset,
reloaded by `:edit` / `:checktime` / `'autoread'`, or unloaded.

Also free: `nvim_error_event({type},{msg})`, "Emitted on the client channel if an async API request responds
with an error", marked `|RPC| only` [doc]. Clients "should handle |nvim_error_event| notifications" [doc]
`:help dev-api-client`.

And `nvim_ui_attach(width, height, options)` — "Activates UI events on the channel. Entry point of all UI
clients", also `|RPC| only` [doc]. 69 UI events exist [probe]. Caveat from the same entry: "If multiple UI
clients are attached, the global screen dimensions degrade to the smallest client."

`nvim_subscribe` / `nvim_unsubscribe` exist in the dispatch table and `nvim_subscribe("orca_cursor")`
returned OK [probe]; they were **not** needed for `rpcnotify` from Lua to reach the client. What they
actually gate is **not found** in the shipped help (no `:help` entry for `nvim_subscribe` was located in
`api.txt`); do not rely on them without checking the source.

### 1.5 Can the core push something unsolicited?

Yes, unconditionally, in both message kinds:

- **Request** (expects a reply): every row in §1.2 was an unsolicited `rpcrequest` from the core. nvim never
  asked for any of it.
- **Notification** (fire-and-forget): `vim.rpcnotify(chan, "nvim_echo", {{"pushed from core"}}, true, {})`
  from the core, with no request/response cycle, put `pushed from core` on the server's stderr [probe].

There is no handshake, no subscription, and no permission step. The only gate is reaching the socket:
"rpc channels are implicitly trusted and the process at the other end can invoke any |API| function"
[doc] `:help channel-rpc`. For the orca-term threat model that is the whole access-control story — filesystem
permissions on the pipe, and nothing else.

### 1.6 Finding the socket (the bootstrap problem)

Three documented routes, all verified:

1. **`$NVIM`, when the core is a child of nvim.** > "$NVIM is set to v:servername by |terminal| and
   |jobstart()|, and is thus a hint that the current environment is a child (direct subprocess) of Nvim."
   — [doc] `:help v:servername`, <https://neovim.io/doc/user/vvars.html>.
   Confirmed [probe], and with a sharp edge:
   - `jobstart({"sh","-c",'echo "NVIM=[$NVIM]"'})` → `NVIM=[/tmp/orca-env.sock]`
   - pty job (what `:terminal` uses) → `TERM_NVIM=[/tmp/orca-env.sock]`
   - **`vim.fn.system()` → `NVIM=[]`.** The variable is *not* propagated by `system()`.
2. **Glob the default socket directory.** [doc] `:help serverstart()` gives the layout
   `stdpath("run").."/{name}.{pid}.{counter}"` and the shell one-liner
   `ls ${XDG_RUNTIME_DIR:-${TMPDIR}nvim.${USER}}/*/nvim.*.0`. Confirmed [probe] on macOS: `stdpath('run')`
   is `/var/folders/.../T/nvim.marivaldocavalheiro/pFX88R`, and the glob
   `$TMPDIR/nvim.$USER/*/nvim.*.0` returned 8 paths for the live and recently-killed instances.
   **Stale entries are included** — several of those 8 belonged to instances this probe had already
   `SIGKILL`ed (§1.7). The core must connect to tell live from dead.
3. **`serverlist({peer: true})`, from inside nvim.** > "peer : If |TRUE|, servers not started by
   |serverstart()| will also be returned. (default: |FALSE|) Not supported on Windows yet."
   — [doc] `:help serverlist()`. Confirmed [probe]: returned the own address plus 7 peers. This is an
   *inside-nvim* call, so it is a discovery tool for a Lua shim, not for the core.

`serverstart()` / `serverstop()` let nvim open and close additional endpoints at runtime, and
"If |v:servername| is stopped it is set to the next available address in |serverlist()|" [doc].

### 1.7 Lifecycle

**nvim exits, core keeps running** [probe]: the core survives; the channel is immediately invalid.

```
sanity rpcrequest: 2
rpcrequest AFTER nvim exited: ok=false err=Invalid channel: 3
rpcnotify  AFTER nvim exited: ok=false err=Invalid channel: 3
chanclose: ok=true res=1
core process itself still running: yes
```

Note that even `rpcnotify` — fire and forget — raises, so loss of the editor is detectable on the next
message without needing a reply.

**Core exits, nvim keeps running** [probe]: nvim survives, reaps the channel, and **keeps everything the
core wrote**.

```
nvim(server) alive: yes
chans BEFORE core SIGKILL: ['2:bytes:stderr', '3:rpc:socket', '4:rpc:socket']
chans AFTER  core SIGKILL: ['2:bytes:stderr', '5:rpc:socket']       <- chan 3 gone
g:orca_mark              -> written-by-core
extmark count in ns      -> 1
exists(":OrcaDead")      -> 2
```

Calling the dead channel id afterwards raises, differently for the two message kinds:

```
rpcnotify(dead)  -> Vim(call):E475: Invalid argument: Channel doesn't exist
rpcrequest(dead) -> Vim(call):Invoking 'ping' on channel 3: Invalid channel: 3
```

Consequence, with no recommendation attached: a core that dies leaves orphaned virtual text, orphaned user
commands and orphaned keymaps in a live editor, and any Lua hook that still calls `rpcnotify` will throw on
the next trigger unless it is `pcall`-wrapped. Cleanup is somebody's job; nvim does not do it.

**Socket file hygiene** [probe]:

| Exit | Socket file left on disk? |
| --- | --- |
| clean `:qa!` | no — removed |
| `SIGKILL` | **yes — stale file remains** |

Connecting to a stale socket from outside: `E247: Failed to connect to '/tmp/orca-lc-b.sock': connection
refused. Send expression failed.`, exit code `2` [probe]. From inside nvim,
`sockconnect("pipe", <absent>, {rpc=true})` raised `Vim:connection failed: connection refused` [probe].
So "the file exists" is never evidence that an nvim is alive.

### 1.8 The `--remote*` family (RPC without a client library)

[doc] `:help clientserver`, <https://neovim.io/doc/user/remote.html>. `--server {addr}` plus
`--remote`, `--remote-silent`, `--remote-tab`, `--remote-tab-silent`, `--remote-send {keys}`,
`--remote-expr {expr}`, `--remote-ui`. Explicitly **not** implemented in Nvim: all the `-wait` variants,
`--servername`, `--serverlist`.

`--remote-expr` is a complete zero-dependency core-to-nvim path and was used for much of §1.7 [probe]:

```
nvim --server $SOCK --remote-expr 'nvim_buf_set_extmark(0,3,0,0,{"virt_text":[["<- from core","Comment"]]})'
nvim --server $SOCK --remote-expr 'string(nvim_win_get_cursor(0))'   # -> [1, 0]
nvim --server $SOCK --remote-expr 'exists(":OrcaPing")'              # -> 2
```

Its limits, from the same probe: one expression per process spawn, result printed to stdout as a string,
no persistent channel, therefore **no callbacks and no `nvim_buf_attach`** — each invocation opens and
closes its own channel (channel ids `4`, `5`, `6` appeared and vanished in the `nvim_list_chans` output
across successive calls).

---

## 2. A socket of our own

The question "what does this constrain about the core" has a short answer and a long one.

Short: nvim will happily speak a protocol you invent, over a unix socket or TCP, in either direction — but
**only Lua running inside nvim can speak it**, because the socket primitives are Lua/Vimscript functions.
Choosing this transport does not free the core from Lua; it moves *all* of the editor-side logic into Lua
and gives the core no API access at all beyond what that Lua chooses to expose.

### 2.1 nvim as the client of our socket

[doc] `:help sockconnect()`, <https://neovim.io/doc/user/vimfn.html>:

> "Connect a socket to an address. If {mode} is "pipe" then {address} should be the path of a local domain
> socket (on unix) or named pipe (on Windows). If {mode} is "tcp" then {address} should be of the form
> "host:port" … Returns a |channel| ID. Close the socket with |chanclose()|. Use |chansend()| to send data
> over a bytes socket, and |rpcrequest()| and |rpcnotify()| to communicate with a RPC socket."

The `rpc` key is optional: **with it you get msgpack-rpc, without it you get raw bytes** [doc]
`:help channel-bytes`. Callbacks are `on_data` / `data_buffered`, and [doc] warns about framing:

> "Stream event handlers receive data as it becomes available from the OS, thus the first and last items in
> the {data} list may be partial lines."

i.e. a custom protocol has to do its own framing, or opt into `channel-buffered` mode and only get the data
at EOF.

`jobstart(…, {rpc: true})` is the sibling: "Use |msgpack-rpc| to communicate with the job over stdio. Then
`on_stdout` is ignored, but `on_stderr` can still be used" [doc] `:help jobstart()`. And `stdioopen({rpc:
true})` for a `--headless` nvim's own stdio, channel id "always 1" [doc].

### 2.2 nvim as the server of our socket

Raw libuv is exposed to Lua:

> "`vim.uv` exposes the "luv" Lua bindings for the libUV library that Nvim uses for networking, filesystem,
> and process management, see |luvref.txt|."
> — [doc] `:help vim.uv`, <https://neovim.io/doc/user/lua.html>

Confirmed end to end [probe]: Lua in nvim bound a unix pipe with `vim.uv.new_pipe(false)` + `bind` +
`listen(16, …)`, an external `printf 'hello-core' | nc -U /tmp/orca-own.sock` connected, and the read
callback received `recv:hello-core`.

### 2.3 The constraint that bites: fast-event context

Every `vim.uv` callback runs in a fast context, where the deferred API is forbidden.

> "It is an error to directly invoke `vim.api` functions (except |api-fast|) in `vim.uv` callbacks."
> — [doc] `:help lua-loop-callbacks` (`E5560`)

Confirmed [probe], from a `vim.uv` timer callback:

```
nvim_command (deferred)   -> ok=false  E5560: nvim_command must not be called in a fast event context
nvim_get_mode (api-fast)  -> ok=true   {blocking=false, mode="n"}
vim.in_fast_event()       -> true
```

And from the custom-socket read callback in §2.2 [probe]:

```
deferred api from socket cb ok=false err=E5560: nvim_buf_set_lines must not be called in a fast event context
```

So a custom-socket shim must wrap in `vim.schedule_wrap` / `vim.defer_fn` ([doc] `:help schedule`) before
touching buffers. The native-RPC path does not have this problem: requests arriving on an RPC channel are
queued on the main loop by nvim itself.

Secondary constraint: threads. "Each thread has its own separate Lua interpreter state, with no access to
Lua globals on the main thread. Neither can the editor state (buffers, windows, etc) be directly accessed
from threads." A subset of the stdlib is available in threads, including `vim.uv`, `vim.mpack` and
`vim.json` [doc] `:help lua-loop-threading`.

### 2.4 Capability and callback summary for a custom socket

- **What the core can do to nvim:** nothing directly. Exactly the operations the Lua shim implements. The
  union of what is *achievable* is the same as native RPC — the shim can call the whole `vim.api` and more
  (it can also call the Lua-only functions the RPC path cannot reach: `nvim_buf_call`, `nvim_win_call`,
  `nvim_set_decoration_provider`). The cost is that every operation must be hand-written on the Lua side.
- **How nvim calls back:** whatever the shim writes into the socket (`chansend()` for a `sockconnect`
  channel, `uv_pipe:write()` for a `vim.uv` one). Fully in your control, fully your maintenance burden.
- **Unsolicited push from the core:** yes, trivially — bytes on a socket. But the shim must be running and
  connected, and the editor-side effect is bounded by the shim's own dispatch table.
- **Client libraries:** irrelevant on the nvim side; on the core side, any socket library in any language.
  This is the one transport that imposes **zero** language constraint on the core.
- **Lifecycle:** not special-cased by nvim. `sockconnect` to an absent address raises
  `Vim:connection failed: connection refused` [probe]. A `vim.uv` listener must be closed explicitly —
  "Always close handles to avoid leaks" [doc] — and whether nvim unlinks a `vim.uv`-bound socket path at
  exit was **not probed**; assume it does not.

### 2.5 Why this is strictly additive work, not a replacement

Mixed into one sentence of fact: the native RPC server is **already listening** by default
(`v:servername` exists without `--listen`, §0), so a custom socket is a second channel that must be built,
bootstrapped and secured, while the first one cannot be turned off short of `serverstop()`.

---

## 3. File plus watch

### 3.1 Core writes, nvim notices

Two layers.

**(a) libuv fs_event, driven by Lua.** [doc] `:help watch-file` ships a worked example using
`vim.uv.new_fs_event()` whose `on_change` calls `:checktime`, with an explicit debounce
(`w:stop()` then restart) baked into the example.

Confirmed [probe], watching `/tmp/orca-watch-target.json` while an external `sh` process wrote it twice
200 ms apart:

```
fs_event count=2
  fs_event[1] err=nil fname=orca-watch-target.json status={ change = true } dt_ms=215.3
  fs_event[2] err=nil fname=orca-watch-target.json status={ change = true } dt_ms=215.3
```

Both callbacks reported the **same** elapsed time, i.e. they were delivered in the same loop iteration
despite the writes being 200 ms apart. Read that as: event delivery is coalesced/batched, the count is not a
reliable change count, and a watcher must treat an event as "re-read the file" rather than "one change
happened". (Flagged as an observation about this run, not a documented guarantee — the batching may be an
artefact of `vim.wait` pumping the loop.)

With an atomic rename write (`> file.tmp; mv file.tmp file`) followed by a plain write [probe]:

```
after atomic rename, fs_event count=2
  fs_event2[1] status={ change = true }
  fs_event2[2] status={ rename = true }
```

The watch kept delivering across the rename here, and libuv distinguished `change` from `rename`. Whether a
path watch *always* survives replacement-by-rename is platform-specific and was **not** established —
treat it as uncertain and re-verify per platform.

Platform note from [doc] `:help inotify-limitations`: on Linux the `fs.inotify.max_user_watches` default
"can be too low", and "a watch can take up to 1KB of space". No equivalent caveat is documented for macOS
(FSEvents/kqueue); the macOS watch limits are **not found** in the nvim docs.

**(b) Buffer reload, no Lua needed for the mechanism itself.**
[doc] `:help :checktime`, <https://neovim.io/doc/user/editing.html>: "Check if any buffers were changed
outside of Vim. … If there are no changes in the buffer and 'autoread' is set, the buffer is reloaded.
Otherwise, you are offered the choice of reloading the file."
`'autoread'` is **default on** in nvim [doc] `:help 'autoread'`, <https://neovim.io/doc/user/options.html>.

`FileChangedShell` fires "When Vim notices that the modification time of a file has changed since editing
started. Also when the file attributes of the file change or when the size of the file changes", and it is
triggered "after: executing a shell command, |:checktime|, |FocusGained|" [doc] `:help FileChangedShell`,
<https://neovim.io/doc/user/autocmd.html>. Constraints in the same entry: "Not used when 'autoread' is set
and the buffer was not changed"; `v:fcs_reason` / `v:fcs_choice` control the outcome; the current buffer `%`
is *not* the target buffer (`<afile>`/`<abuf>`); "Cannot switch, jump to or delete buffers" (`E246`,
`E811`); non-recursive. `FileChangedShellPost` fires after handling.

The decisive point: **nvim does not poll.** Without `:checktime` being called — by `FocusGained`, by a shell
command, or by something on a timer — a file changed by the core is not noticed at all. There is no
documented background file-poll in nvim. So "file plus watch" always needs a trigger, and the trigger needs
Lua or Vimscript in nvim.

### 3.2 nvim writes, core notices

Symmetric and unconstrained: the core watches with its own platform facility (inotify / FSEvents / kqueue,
or any library). nvim's side is just `:write` / `writefile()` / `nvim_buf_set_lines` + `:write`. Nothing in
nvim restricts this, and nothing in nvim helps either — nvim will not tell the core *which* write
corresponds to *which* editor action; that must be encoded in the file's content.

### 3.3 Capability and callback summary for file plus watch

- **What the core can do to nvim:** nothing by writing a file. Opening a buffer, placing virtual text,
  reading the cursor, reading the visual selection and defining a command are **all** out of reach of a file
  write by itself. Each one requires Lua in nvim that reads the file and performs the action. Reading the
  cursor and the live visual selection is particularly poorly served: both are volatile editor state with no
  file representation, so the Lua side would have to *write them out* on some trigger, and the core would
  read a snapshot that is already stale.
- **How nvim calls back:** by writing another file, which the core watches. Latency is
  write + watch-delivery + debounce on each hop.
- **Unsolicited push from the core:** the core can write at any time, but **nothing happens** until nvim's
  watcher fires, and nvim has no watcher unless Lua installed one.
- **Client libraries:** none needed; zero language constraint on the core.
- **Lifecycle:** the most forgiving of the three in one narrow sense — files outlive both processes, so
  state survives a restart of either side. And the most dangerous in another: **neither side can tell
  whether the other is alive.** A core that died leaves a file that looks current; an nvim that died leaves
  a file the core will keep writing to. Liveness needs a separate mechanism (pidfile, heartbeat timestamp,
  lock), which is **not** provided by anything in nvim.
- Stale/partial reads: no framing, no atomicity guarantee from nvim's side. `vim.json` / `vim.mpack` are
  available in Lua [doc] `:help lua-loop-threading` for encoding, but torn reads are the writer's problem.

---

## 4. Client libraries and what they do to the core's language

### 4.1 nvim's own position

> "API clients wrap the Nvim |API| to provide idiomatic "SDKs" for their respective platforms … List of API
> clients: https://github.com/neovim/neovim/wiki/Related-projects#api-clients"
> — [doc] `:help api-client` / `:help dev-api-client`, <https://neovim.io/doc/user/develop.html>

> "These clients can be considered the "reference implementation" for API clients:
> - https://github.com/neovim/node-client
> - https://github.com/neovim/pynvim"
> — [doc] `:help node-client` / `:help pynvim`

Expected client behaviour, from the same section: "API clients exist to hide msgpack-rpc details. The
wrappers can be automatically generated by reading the |api-metadata| from Nvim"; "Clients should call
|nvim_set_client_info()| after connecting, so users and plugins can detect the client by handling the
|ChanInfo| event"; "Clients should handle |nvim_error_event| notifications". `nvim_set_client_info` over RPC
worked and showed up in the server's channel list as
`client = {name = "orca-core-probe", type = "remote", version = {major=0, minor=1}, …}` [probe].

Discovery paths for a statically-compiled client, from [doc] `:help api-mapping`: call
`nvim_get_api_info()` at runtime, or use `--api-info` at build time (32 KB of msgpack here, §0), or
`api_info()` from inside nvim.

API stability, [doc] `:help api-contract`: "Function signatures will NOT CHANGE after release" except for
additive extensions; "Deprecated functions will not be removed until Nvim 2.0". "Private" interfaces —
undocumented functions and `nvim__x` double-underscore names — are not covered.

### 4.2 The full list of API clients

Verbatim from [web] <https://github.com/neovim/neovim/wiki/Related-projects#api-clients> (the list
`:help api-client` points to), treated as data:

| Language | Project |
| --- | --- |
| C# | neovim/nvim.net |
| C++ | DaikiMaekawa/neovim.cpp |
| C++/Qt5 | equalsraf/neovim-qt |
| C++/ncurses | splinterofchaos/neovim-cpp-client-experiment |
| C++/Magnum | Squareys/magnum-neovim-api |
| Clojure | jebberjeb/neovim-client |
| Common Lisp | adolenc/cl-neovim |
| Dart | smolck/dart-nvim-api |
| Elixir | awetzel/neovim-elixir |
| Filesystem | fmoralesc/nvimfs |
| Go | neovim/go-client |
| Haskell | neovimhaskell/nvim-hs |
| Java | fdinoff/neovim-java-client, esensar/neovim-java |
| Julia | bfredl/Neovim.jl |
| **Node.js** | **neovim/node-client**, neoclide/neovim |
| OCaml | janestreet/vcaml |
| Perl | jacquesg/Neovim-Ext, yanick/Neovim-RPC |
| Python | neovim/pynvim |
| R | jalvesaq/Nvim-R |
| Racket | HiPhish/neovim.rkt |
| Ruby | neovim/neovim-ruby |
| **Rust** | **noib3/nvim-oxi**, daa84/neovim-lib, KillTheMule/nvim-rs |
| Swift/Cocoa | qvacua/vimr NvimView |
| Zig | jinzhongjia/znvim |

### 4.3 The three available-locally options, checked against primary sources

**Rust — `nvim-rs` (KillTheMule/nvim-rs).** An out-of-process msgpack-rpc client.
[web] README: "Rust library for Neovim msgpack-rpc clients. Utilizes async to allow for arbitrary nesting of
requests." Status in the README: "Useable", with "The **API** is unstable, see the Roadmap for things being
planned." Runtime feature flags `use_tokio` and `use_async-std`. LGPL-3.0 for the crate; contributions dual
Apache/MIT.
Transports, from [web] <https://docs.rs/nvim-rs/latest/nvim_rs/create/tokio/index.html> — this covers all
four connection shapes the core might need:

| Function | Transport |
| --- | --- |
| `new_path()` | unix socket (Unix) / named pipe (Windows) |
| `new_tcp()` | TCP |
| `new_child()`, `new_child_cmd()`, `new_child_path()`, `new_child_handshake_cmd()` | spawn an nvim and talk to it |
| `new_parent()` | talk to the nvim that spawned this process, over stdin/stdout |

[web] crates.io API: `max_version 0.9.2`, 207 324 downloads, repo `github.com/KillTheMule/nvim-rs`.
(crates.io reports `updated_at 2025-03-23`; docs.rs shows 0.9.2 dated 26 July 2025 — the two disagree,
noted rather than resolved.) How incoming requests/notifications are handled (the handler trait) is **not
found** in the README; docs.rs has a `Handler` trait but I did not read it, so treat the callback ergonomics
as unverified.

**Rust — `neovim-lib` (daa84/neovim-lib).** [web] crates.io: `max_version 0.6.1`, `updated_at
2019-04-12`, 101 703 downloads. Last release over six years ago. No maintenance statement was read.

**Rust — `nvim-oxi` (noib3/nvim-oxi): not an RPC client, and directly relevant to the Lua question.**
[web] README: it "leverages Rust's foreign function interface (FFI) support to hook straight into the
Neovim C code", achieving "feature parity with 'in process' plugins while also avoiding the need for an
extra IO layer", and explicitly contrasts itself with RPC's need to be "(de)serializing everything to
MessagePack-encoded messages". It builds a `cdylib` loaded into the nvim process.
[web] crates.io: `max_version 0.6.0`, `updated_at 2025-05-23`, 58 101 downloads.
**This is Rust running inside nvim, not a separate core.** It is a way to write the *nvim-side shim* in Rust
instead of Lua — see §5 — not a way for a separate core to drive nvim.

**Node — `neovim` (neovim/node-client), a reference implementation per `:help node-client`.**
[web] README: the primary interface is `attach()`, which "Takes a process, socket, or pair of write/read
streams and returns a `NeovimClient` connected to an `nvim` process"; `findNvim()` "Tries to find a usable
`nvim` binary on the current system"; `NvimPlugin` exposes `registerAutocmd()`, `registerCommand()`,
`registerFunction()`. It serves both roles — attaching to an existing instance (via a socket path or
`$NVIM_LISTEN_ADDRESS`) and acting as a remote plugin host from `rplugin/node/`. README states "Node.js 16
and later is tested."
[web] npm registry: latest `5.5.0`, published 2026-09-11, `engines.node >= 14`, repo
`github.com/neovim/node-client`.
Local friction worth recording: node is `v22.5.1`, which satisfies node-client, but **pnpm on this machine
refuses to run on it** (needs `>= v22.13`, §0). A Node core would need node upgraded or a different package
manager.

**Node — `neoclide/neovim`.** Listed on the wiki. Not inspected. Its relationship to node-client (fork,
divergence, maintenance) is **not found** here.

### 4.4 What this actually constrains

- Native RPC with a **library**: Rust and Node both have one. Rust's is `nvim-rs` (self-described
  unstable API), Node's is the upstream reference implementation with the more recent release.
- Native RPC with **no library at all** is viable: the protocol is MessagePack-RPC with two documented
  extra constraints [doc] `:help msgpack-rpc`, the method table is machine-readable via `--api-info`, and
  raw msgpack codecs exist in both ecosystems. The language constraint is then "has a msgpack codec and a
  socket", which every candidate satisfies.
- `--remote-expr` subprocess calls (§1.8) constrain nothing at all — any language that can spawn a process.
- A custom socket (§2) and file-plus-watch (§3) constrain nothing on the core side, and shift the entire
  cost onto the nvim-side Lua.
- Python's `pynvim` is the other reference implementation but python3 on this machine has **no `msgpack`
  module** (`msgpack unavailable: No module named 'msgpack'`) [probe], so a Python core would need
  dependency installation that rust/node do not.

---

## 5. The unavoidable Lua floor

The constraint from the map: the core itself must not be Lua inside nvim. So: what is the minimum that must
nonetheless live inside the nvim process?

### 5.1 Floor per transport

| Transport | Minimum that must live in nvim |
| --- | --- |
| Native RPC, core connects out to `v:servername` | **Nothing, in the steady state.** Commands, autocmds and keymaps can all be installed over RPC from the core using Vimscript or Lua *strings* (§1.4, all five rows verified). Buffer updates come via `nvim_buf_attach` with no hook at all. |
| Native RPC, core must be found by nvim | A bootstrap: something in nvim must start/locate the core. |
| Custom socket | **A full shim.** All socket handling (`sockconnect` / `vim.uv`), framing, dispatch, and `vim.schedule_wrap` for every deferred API call (§2.3). |
| File plus watch | **A watcher plus a trigger.** `vim.uv.new_fs_event` or a timer, plus `:checktime` or explicit re-reads (§3.1). Nothing happens without it. |

### 5.2 The residual floor even on native RPC

Three things cannot be done from the core alone, and each one is a concrete piece of in-nvim code:

1. **Bootstrap / discovery.** No mechanism was found by which a starting nvim announces itself to an
   already-running external process. Either (a) the core discovers nvim — `$NVIM` if the core is a child of
   nvim (`:terminal` / `jobstart`, verified; **not** `vim.fn.system`), or the socket-directory glob with the
   stale-entry problem (§1.6); or (b) nvim reaches out, which needs at minimum one line in the user's config.
   For orca-term specifically, (a) via `$NVIM` is free in exactly the case where agents run in terminal
   buffers inside nvim.
2. **Hooks the core wants to exist before it connects, or that must survive a core restart.** Everything the
   core installs over RPC dies with the channel's usefulness: the command still exists after the core dies,
   but its `rpcnotify` raises `E475: ... Channel doesn't exist` (§1.7). A keymap that must work on the first
   keystroke after nvim starts, before any core has connected, must be defined in nvim.
3. **Anything needing a real Lua function value.** `nvim_set_decoration_provider` (per-redraw ephemeral
   extmarks), `nvim_buf_call`, `nvim_win_call` — all rejected over RPC with `Cannot convert given Lua type`
   (§1.3). Also any `nvim_buf_attach` Lua-callback variant: [doc] `:help api-buffer-updates-lua` describes
   `on_lines` receiving no text ("Unlike remote channel events the text contents are not passed"), which is
   the cheap in-process path the RPC path cannot have. And the `textlock` rules apply to those callbacks:
   "|textlock| prevents changing buffer contents and window layout (such operations must be |schedule|d).
   Moving the cursor is allowed, but it is restored afterwards."

So the honest floor for native RPC is: **zero lines of persistent Lua for the capability set asked about in
this ticket** (buffer, virtual text, cursor, visual selection, command — all verified from outside), plus
whatever bootstrap the deployment story needs, plus Lua only if decoration providers or pre-connect keymaps
turn out to be required.

### 5.3 Two documented ways to avoid hand-written Lua that are not "the core in Lua"

- **Remote plugins.** [doc] `:help remote-plugin`, <https://neovim.io/doc/user/remote_plugin.html>:
  "Extensibility is a primary goal of Nvim. Any programming language may be used to extend Nvim without
  changes to Nvim itself. This is achieved with remote plugins, coprocesses that have a direct communication
  channel (via |RPC|) with the Nvim process. Even though these plugins run in separate processes they can
  call, be called, and receive events just as if the plugin's code were executed in the main process."
  The boilerplate is generated: `:UpdateRemotePlugins` writes a manifest, "a special Vimscript file
  containing declarations for all Vimscript entities (commands/autocommands/functions) defined by all remote
  plugins", whose entries are "just calls to the `remote#host#RegisterPlugin` function, which takes care of
  bootstrapping the host as soon as the declared command, autocommand, or function is used for the first
  time". So the generated glue is **Vimscript, not Lua**, and the host is spawned lazily — which also solves
  bootstrap (1) above, in the direction of nvim starting the core.
  Costs, from the same page: the plugin must live in `rplugin/{host}/` on `'runtimepath'`;
  `:UpdateRemotePlugins` must be re-run "every time a remote plugin is installed, updated, or deleted"; the
  manifest goes to `$XDG_DATA_HOME/nvim/rplugin.vim` unless `$NVIM_RPLUGIN_MANIFEST` is set. Node has a host
  (`rplugin/node/`, `NvimPlugin.registerCommand/registerAutocmd/registerFunction`, §4.3). Whether a
  **Rust** remote-plugin host exists is **not found**.
- **`nvim-oxi`** writes the in-process shim in Rust rather than Lua (§4.3). This satisfies "no Lua" literally
  while violating "the core is a separate process" — it is a `cdylib` in nvim's address space. Listing it
  for completeness, not as an equivalent.

---

## 6. Things asked and not established

- Whether a deferred RPC request **actually** blocks during a modal prompt on this build. Documented
  (`:help api-fast`); my repro returned in 0 ms (§1.3). Uncertain.
- What `nvim_subscribe` / `nvim_unsubscribe` gate. Present in the dispatch table, accepted a call, no help
  entry located in the shipped `api.txt`, and not needed for `rpcnotify` delivery (§1.4). **not found.**
- Whether a `vim.uv` fs_event path watch always survives replace-by-rename. One run survived (§3.1);
  platform-specific; **not established.**
- macOS watch-count limits for fs_event. The nvim docs cover only Linux inotify. **not found.**
- Whether nvim unlinks a `vim.uv`-bound socket path on exit (§2.4). **not probed.**
- `nvim-rs`'s handler trait / incoming-notification ergonomics. **not found** in the README (§4.3).
- `neoclide/neovim` vs `neovim/node-client`: divergence and maintenance. **not found** (§4.3).
- Whether a Rust remote-plugin host exists (§5.3). **not found.**
- The `nvim-rs` 0.9.2 release date: crates.io says 2025-03-23, docs.rs says 26 July 2025. Unresolved.

---

## Sources

Neovim official documentation — help tags, with the canonical page. Quotes were taken from the help files
shipped with the local 0.12.4 build (`/opt/homebrew/Cellar/neovim/0.12.4/share/nvim/runtime/doc/`):

- `:help api`, `:help api-rpc`, `:help msgpack-rpc`, `:help rpc-connecting`, `:help api-types`,
  `:help api-fast`, `:help api-metadata`, `:help api-mapping`, `:help api-contract`,
  `:help api-buffer-updates`, `:help api-buffer-updates-lua`, `:help api-highlights`, `:help api-events`,
  `:help nvim_ui_attach()`, `:help nvim_get_chan_info()` — <https://neovim.io/doc/user/api.html>
- `:help channel`, `:help channel-intro`, `:help channel-bytes`, `:help channel-rpc`,
  `:help channel-stdio`, `:help channel-lines`, `:help channel-buffered` — <https://neovim.io/doc/user/channel.html>
- `:help clientserver`, `:help --remote`, `:help --remote-expr`, `:help --server`,
  `:help clientserver-missing` — <https://neovim.io/doc/user/remote.html>
- `:help remote-plugin`, `:help remote-plugin-hosts`, `:help remote-plugin-example`,
  `:help remote-plugin-manifest`, `:help :UpdateRemotePlugins` — <https://neovim.io/doc/user/remote_plugin.html>
- `:help vim.uv`, `:help lua-loop-callbacks` (E5560), `:help watch-file`, `:help inotify-limitations`,
  `:help tcp-server`, `:help lua-loop-threading` — <https://neovim.io/doc/user/lua.html>
- `:help api-client`, `:help dev-api-client`, `:help node-client`, `:help pynvim` — <https://neovim.io/doc/user/develop.html>
- `:help v:servername`, `:help $NVIM` — <https://neovim.io/doc/user/vvars.html>
- `:help sockconnect()`, `:help serverstart()`, `:help serverstop()`, `:help serverlist()`,
  `:help jobstart()`, `:help stdioopen()` — <https://neovim.io/doc/user/vimfn.html>
- `:help FileChangedShell`, `:help FileChangedShellPost`, `:help FocusGained` — <https://neovim.io/doc/user/autocmd.html>
- `:help :checktime` — <https://neovim.io/doc/user/editing.html>
- `:help 'autoread'` — <https://neovim.io/doc/user/options.html>

Third-party (data, not instruction):

- API client list: <https://github.com/neovim/neovim/wiki/Related-projects#api-clients>
- nvim-rs: <https://github.com/KillTheMule/nvim-rs>, <https://docs.rs/nvim-rs/latest/nvim_rs/create/tokio/index.html>, <https://crates.io/api/v1/crates/nvim-rs>
- neovim-lib: <https://crates.io/api/v1/crates/neovim-lib>
- nvim-oxi: <https://github.com/noib3/nvim-oxi>, <https://crates.io/api/v1/crates/nvim-oxi>
- node-client: <https://github.com/neovim/node-client>, <https://registry.npmjs.org/neovim>
- MessagePack-RPC spec, as cited by `:help msgpack-rpc`: <https://github.com/msgpack-rpc/msgpack-rpc/blob/master/spec.md>

Local probes: `nvim 0.12.4` on darwin 25.5.0, 2026-10-07. Each probe ran `nvim --headless --clean --listen
<socket>` as the server with a separate process as client; probe scripts were scratch files and are not
committed.
