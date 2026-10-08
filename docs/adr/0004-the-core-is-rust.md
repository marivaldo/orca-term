---
status: accepted
---

# The core is Rust, blocking, with no async runtime

A reader who knows the nvim ecosystem would expect Node: `neovim/node-client` is the upstream
reference client and doubles as an rplugin host. We chose Rust instead, written as blocking code
with no tokio, because the core is **ephemeral**. Per
[ADR 0001](0001-the-core-owns-no-agent-process.md), it is invoked once per turn. It is also invoked
once per claude hook event during takeover, and once per fleet view refresh. So process start time
is paid constantly, and nothing is ever long-running enough to need an async runtime.

Decided in [issue #12](https://github.com/marivaldo/orca-term/issues/12), which holds the
measurements.

## Considered options

Warm process start was measured on this machine, averaged over 50 runs, with `/usr/bin/true` at
about 3.4 ms as the floor:

- **Rust**: about 5 ms. `File::lock` is in std, verified on rustc 1.97 (a second `try_lock` fails
  while the lock is held). It builds one binary, and the toolchain is already installed. Chosen.
- **Python**: about 33 ms. It has `fcntl.flock` and the official `pynvim` client. Rejected because
  it is six times slower to start and needs a runtime and its version on the installing machine.
- **Node/TypeScript**: about 64 ms. It has the official client and native fetch and streams.
  Rejected because it is the slowest to start, has **no `flock` without a native addon**
  (`fs.flock` is `undefined` on v22), and would need a node upgrade first, since pnpm refuses
  v22.5.1. Its rplugin host no longer buys anything: after ADR 0001, bootstrap in the nvim-to-core
  direction is an ordinary `jobstart`.
- **Go**: one binary, fast start, `syscall.Flock`. It was never measured because it is not
  installed, and `neovim/go-client` has had no release since 2022. It would be equivalent on most
  axes, but nothing tipped the choice to it.

On async: **tokio was rejected**. Each invocation does one thing, such as reading `claude -p`'s
stream-json lines or consuming one opencode SSE stream, then writes the disk and notifies nvim.
That fits in `std::process`, a blocking HTTP client and one extra thread at most. tokio would add
compile time, binary size and a runtime start to every hook, for concurrency the process does not
have.

## Consequences

- **No nvim client library.** `nvim-rs` describes its API as unstable and is built on tokio. The
  core speaks msgpack-RPC on the socket directly, which
  [the nvim transport research](https://github.com/marivaldo/orca-term/issues/3) showed is enough,
  and it needs little more than notifications.
- **SSE is parsed by hand**, as `data:` lines over a blocking HTTP response.
- **The nvim client is thin Lua.** It renders `lane ls --json` and holds no logic of its own. The
  map's rule forbids the *core* being Lua inside nvim, not the client.
- The first exec of a freshly built, ad-hoc-signed binary costs about 25 ms on macOS. Later runs
  do not pay it.
