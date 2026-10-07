# The minimum contract of a CLI agent: claude-code and opencode

Research for issue #4. Facts and their sources only. The contract design belongs to the tickets
this one blocks.

**Scope.** Only `claude-code` and `opencode`. No other agent was investigated.

**Method and trust levels.** Every claim below is tagged:

- **[local]** — verified by running the binary on this machine, or by reading state it wrote.
  Environment: macOS (Darwin 25.5.0), `claude` 2.1.293 at `/Users/marivaldocavalheiro/.local/bin/claude`,
  `opencode` 1.18.0 at `/opt/homebrew/bin/opencode`. Checked 2026-10-07.
- **[doc]** — stated in first-party documentation. URL given per group.
- **[src]** — read in a third-party repository's source. Treated as evidence of what that project
  does, not as a recommendation.

Where neither held, the line says **not found** rather than inferring.

---

## 1. Prior art: verification result

Issue #4 carried three claims attributed to a research subagent. All three were checked at the
source. All three hold.

> Orca drives agents through a PTY.

**Confirmed [src].** `stablyai/orca` (TypeScript, public) contains an extensive `node-pty` layer:
`src/main/pty/node-pty-pts-name.ts`, `src/shared/node-pty-spawn-helper.ts`,
`src/main/orcad/node-pty-precondition.ts`, `src/main/daemon/node-pty-error-hints.ts`,
`src/relay/node-pty-binding-survey.ts`, `src/main/ssh/ssh-relay-node-pty-repair.ts`, and others.

> Orca reads a native transcript per agent: JSONL for claude, SQLite for opencode.

**Confirmed [src].** Separate reader paths exist per agent family:

- Claude: `src/main/native-chat/transcript-reader.ts`, `src/main/native-chat/session-file-resolver.ts`.
  The resolver's own comment states the home resolution rule: *"a structured Claude session pins its
  account home to `CLAUDE_CONFIG_DIR || ~/.claude` (claude-accounts/runtime-paths.ts), and the CLI
  writes its transcript under whatever home it was given."*
- opencode: `src/main/ai-vault/session-scanner-opencode-sqlite-open.ts`,
  `src/main/opencode-usage/opencode-database-discovery.ts`, `src/shared/opencode-database-name.ts`,
  `src/main/opencode-usage/schema-helpers.ts`.

> Orca detects one-shot invocations such as `claude --print` specifically so as not to treat them
> as agent sessions.

**Confirmed [src], and the mechanism is more specific than the claim.** The detection is a
command-line parse of the PTY's foreground process, in `src/shared/agent-process-recognition.ts`.
Verbatim from that file:

```ts
export function recognizeAgentProcessFromCommandLine(
  commandLine: string | null | undefined,
  // Why: TUI consumers (status hooks, shell shadows) filter out headless
  // one-shots (`claude -p …`); non-interactivity guards include them — a
  // one-shot agent can't answer a prompt either.
  options?: { includeHeadlessOneShot?: boolean }
): RecognizedAgentProcess | null
```

The filter is `filterHeadlessOneShotAgentCommand` from `src/shared/agent-headless-command.ts`, and
the distinction is a *two-sided* one, not a single exclusion: TUI consumers exclude one-shots,
while non-interactivity guards include them, because "a one-shot agent can't answer a prompt
either". Test files per agent family exercise it
(`src/shared/agent-process-recognition.test.ts`, `src/shared/dsb-native-one-shot-recognition.test.ts`,
`src/shared/qoder-agent.test.ts`), covering argument forms such as `-p`, `--print`,
`-ptask`, `-cp task`, `-cp--print`, `--prompt-file`, `--prompt-json`, and the same flags behind
`node --import … /bin/x.js` wrappers and `--` separators.

**One nuance the prior art omitted.** Orca does contain ACP tooling:
`config/scripts/acp/generate-protocol.mjs`. So "PTY + native transcript" is not the whole of what
that project does with agent protocols. Nothing was read about how that generated protocol is used.

---

## 2. claude-code (v2.1.293)

### 2.1 Prompt injection

**One-shot. [local][doc]** `claude -p "<prompt>"` (alias `--print`). Exit code 0 on success,
non-zero on failure; `-p` reads stdin, capped at 10MB. `--input-format stream-json` accepts
realtime streaming input for multi-turn drive, and only works with `--print`. A one-shot can
continue an existing conversation: `claude -p --resume <session-id> "<prompt>"` or `-p --continue`.
`--bare` is documented as "the recommended mode for scripted and SDK calls" and skips hook, skill,
plugin, MCP and `CLAUDE.md` auto-discovery.

**Into a live session — a real, documented path exists, with sharp edges. [doc]** For a session
that is *running in the background*, `claude --resume <session> "check the tests too"` delivers the
prompt as that session's next turn, printing
`Sent your prompt to the background session (<id>); opening it…` before attaching.
`claude -p --resume <session> "prompt"` typed at a terminal does the same, and `-p` does **not**
keep the run non-interactive in that case. The prompt is **not** sent when the command line has
any of:

- piped or redirected input or output;
- session-configuring flags (`--permission-mode`, `--model`, `--settings`);
- output-reading flags (`--output-format json`, `--json-schema`);
- run-limiting flags (`--max-turns`, `--max-budget-usd`).

In those cases Claude Code "sends nothing and exits with status 1". Also not sent: a prompt
starting with `/` or `!`, and any prompt while the session is waiting on an answer to a question —
the message then includes `Your prompt was not sent to it` with the reason.

This matters for an orchestrator: **the documented injection path is mutually exclusive with the
documented structured-output path.** You cannot inject into a live session *and* ask for
`--output-format json` in the same invocation.

A separate path exists for cloud sessions: `claude --cloud <session-id> -p` "queues a message into
that cloud session and exits". Not relevant to local worktrees.

**Into a foreground interactive session the orchestrator did not start as a background session:
not found.** No documented non-PTY delivery mechanism was found. See §2.6 for what the daemon
roster suggests but which was not tested.

### 2.2 Turn-end / waiting / failed, without parsing the TUI

Three non-TUI channels exist, and they do not overlap in coverage.

**(a) `claude agents --json` — a live roster. [local]** Documented as "Print active sessions
(interactive and background) as a JSON array and exit (for scripting; does not require a TTY)".
Verified output shape, one element per live session:

```json
{ "pid": 11501, "cwd": "/Users/…/dotfiles", "kind": "interactive",
  "startedAt": 1784381561646, "sessionId": "bc74bb37-44ac-45f7-83b1-554d052db0f8",
  "name": "dotfiles-8d", "status": "idle" }
```

Observed `status` values across the live roster: `idle` and `busy`. Observed `kind`: `interactive`.
`background` was not observed because no background session was running; the flag's own help text
names both. **This is the single highest-value primitive found for claude-code**: busy/idle for
*interactive* sessions, by session id, with no TTY and no TUI scraping. Caveats: the full enum of
`status` is **not found** (whether a distinct "waiting for input" or "error" status exists was not
observed and is not documented on the pages read); `--all` is needed to "also include completed
background sessions".

**(b) Hooks — per-event push. [doc]** `Stop` fires "when Claude finishes responding" and carries
`last_assistant_message` (the final assistant text of the turn). `SubagentStop` carries `agent_id`,
`agent_type`, `last_assistant_message`. `SessionEnd` carries `reason`
(`clear` | `resume` | `logout` | `prompt_input_exit` | `other`). `Notification` carries
`notification_type` and `message`. `UserPromptSubmit` carries `prompt_text`. `PostToolUse` carries
`tool_name`, `tool_input`, `tool_output`, `tool_use_id`. Every hook event receives common fields:

```json
{ "session_id": "abc123", "prompt_id": "550e8400-e29b-41d4-a716-446655440000",
  "transcript_path": "/home/user/.claude/projects/.../transcript.jsonl",
  "cwd": "/home/user/my-project", "scratchpad_dir": "/tmp/claude-1000/...",
  "permission_mode": "default", "hook_event_name": "EventName" }
```

Hooks are the only channel found that pushes turn-end for a *foreground interactive* session.

**(c) `stream-json` — per-event stream, `-p` only. [doc][local]** Documented events include
`system/init` (session metadata; carries an optional `capabilities` array of protocol-behavior
strings such as `interrupt_receipt_v1`, intended for feature detection "instead of comparing
version strings"), `system/api_retry` (with `attempt`, `max_retries`, `retry_delay_ms`,
`error_status`, and an `error` category enum: `authentication_failed`, `oauth_org_not_allowed`,
`account_on_hold`, `billing_error`, `rate_limit`, `overloaded`, `invalid_request`,
`model_not_found`, `server_error`, `max_output_tokens`, `cloud_credential_error`, `unknown`),
`system/plugin_install`, and `permission_denied` system messages. "The last line of the stream is a
`result` message."

The `result` envelope, captured verbatim from a live `claude -p --output-format json` run on this
machine [local] — keys only, elided:

```
type:"result"  subtype:"success"  is_error:false  stop_reason:"end_turn"
terminal_reason:"completed"  num_turns:1  result:"pong"
session_id:"11111111-2222-4333-8444-555555555555"
uuid:"d9dbb006-5b91-42c7-a63d-7e0b6a54e25a"
duration_ms, duration_api_ms, ttft_ms, usage{…}, modelUsage{…}, total_cost_usd,
permission_denials:[], safety_stops:0, subagent_stats{…}, api_error_status:null,
queued_turn_count:0, result_index:0
```

So for a one-shot, turn outcome is fully structured: `is_error`, `subtype`, `stop_reason`,
`terminal_reason`, `api_error_status`, `permission_denials`, `num_turns`.

**Failure outside the stream. [doc]** Invalid flags go to stderr before the run starts; a failure
*inside* the run (e.g. missing auth) "prints the failure as the result on stdout". SIGTERM exits
with code 143, leaves the in-progress turn unfinished and "records no result for it"; SIGINT ends
the turn instead.

### 2.3 Capturing what the agent did without destroying its TUI

**Background sessions. [local][doc]** `claude logs <id|name>` — "Print a background session's
recent terminal output". `claude attach <id|name>` opens it in this terminal, and "the session
keeps running either way". Read-only output capture therefore needs no PTY takeover.

**Foreground interactive sessions. [doc]** `transcript_path` delivered to every hook invocation
and to status-line commands. The docs name exactly this: "React to session events: read the
`transcript_path` field that hooks and status line commands receive as input. A `SessionEnd` hook
can archive the transcript when a session ends."

**Explicit documented warning. [doc]** "The `transcript_path` field may lag the in-memory
conversation since it writes asynchronously. For events like `Stop` and `SubagentStop`, use the
`last_assistant_message` field instead of reading the transcript file for the final assistant
text."

**Not for scripts. [doc]** `/export` "produces a rendered transcript for a person to read" — the
docs direct scripts to the structured interfaces instead.

### 2.4 Structured transcript on disk

**Location and format. [local][doc]** `~/.claude/projects/<project>/<session-id>.jsonl`, where
`<project>` is the working-directory path with non-alphanumeric characters replaced by `-`. If the
converted name exceeds 200 characters, it is truncated to 200 and a hash of the full path is
appended. "Each line is a JSON object for a message, tool use, or metadata entry."

Verified on this machine: `~/.claude/projects/-Users-marivaldocavalheiro-Projects-own-orca-term/<uuid>.jsonl`,
and a git worktree gets its own directory
(`-Users-marivaldocavalheiro-Projects-own-orca-term--claude-worktrees-agent-<id>/`) — relevant for a
worktree-per-agent design, since the transcript follows the worktree path, not the repo root.

Configuration knobs [doc]: `CLAUDE_CONFIG_DIR` moves storage off `~/.claude`;
`CLAUDE_CODE_PROJECT_DIR_NAME` names the `<project>` directory (requires `CLAUDE_CONFIG_DIR` too,
1–64 chars of letters/digits/hyphens/underscores, read once at startup from the shell environment,
so a settings-file `env` block cannot set it; v2.1.234+); `cleanupPeriodDays` changes the 30-day
retention; `CLAUDE_CODE_TRANSCRIPT_LOCAL_GC` caps a `-p`/SDK transcript's growth;
`CLAUDE_CODE_SKIP_PROMPT_HISTORY` suppresses transcript writes in all modes;
`--no-session-persistence` suppresses them for one non-interactive run.

**Is it stable enough to depend on? The docs say no, in as many words. [doc]**

> "The entry format is internal to Claude Code and changes between versions, so scripts that parse
> these files directly can break on any release. To build on session data, use `/export` or the
> script interfaces instead."

This is a direct, first-party contradiction of a design that reads the JSONL as its primary
channel. The observed record shapes support the warning [local]: across one session file, 20+
distinct top-level key sets appear, with version-flavoured fields such as `wireToolInputs`,
`wireIngestContext`, `serverClassifierRequest`, `atis`, `advisorModel`, `apiBlockIndex`,
`perTurnEffort`, `attributionPlugin`, `attributionSkill`. Observed `type` values in one session:
`attachment`, `assistant`, `user`, `permission-mode`, `mode`, `last-prompt`, `atis-latch`,
`ai-title`, `system`, `queue-operation`, `file-history-snapshot`. Every record carries a `version`
field (the Claude Code version that wrote it), which at least makes a parser's drift detectable.

**A `-p` run does write a transcript. [local]** A one-shot `claude -p --session-id <uuid>` produced
`<uuid>.jsonl` (30 lines) under the worktree's project directory. So "one-shot" does not mean
"leaves no transcript". But [doc]: `-p`/SDK sessions are kept out of the session picker and out of
`claude --continue`, and are resumable only by explicit `--resume <session-id>`.

**Discriminating one-shot from interactive, from the transcript itself. [local]** The `entrypoint`
field differs:

| Invocation | `entrypoint` |
| --- | --- |
| interactive session | `cli` |
| `claude -p` | `sdk-cli` |

Verified both ways on this machine (328 records `cli` in an interactive session; 22 records
`sdk-cli` in the `-p` run, 8 records with no `entrypoint`). This is an undocumented internal field
on an explicitly unstable format — usable as a signal, not as a contract. Note it is a *different*
and independent discriminator from Orca's command-line parse (§1), and it works after the fact
rather than on a live process.

### 2.5 Structured protocol (ACP or similar)

**No ACP. [local]** `claude` 2.1.293's full `--help` (311 lines, all subcommands and flags) contains
zero case-insensitive matches for `acp` or `agent client protocol`. Subcommand list in full:
`agents`, `attach`, `auth`, `auto-mode`, `doctor`, `gateway`, `import`, `install`, `logs`, `mcp`,
`plugin`, `purge`, `respawn`, `rm`, `setup-token`, `stop`/`kill`, `ultrareview`, `update`.
No `acp`.

**No ACP in the docs either. [doc]** `https://code.claude.com/docs/llms.txt` (the complete
documentation index, 411 lines) contains zero matches for `acp` or `agent client protocol`.

**What it speaks instead. [doc]** Its own JSON protocols: `--output-format json`, `--output-format
stream-json` with `--input-format stream-json` for bidirectional streaming, `--json-schema` for
schema-validated structured output (returned in a `structured_output` field), and the Agent SDK
(TypeScript / Python) over the same wire. It is an MCP *client* (`--mcp-config`,
`--strict-mcp-config`, `claude mcp`), which is a different protocol for a different purpose.

**ACP via third-party adapter:** secondary sources describe adapters that bridge Claude Code to
ACP clients. Not verified at a primary source; not treated as a fact here.

### 2.6 What identifies a turn

Candidate identifiers, in descending order of how well they are documented:

| Identifier | Scope | Source | Where you get it |
| --- | --- | --- | --- |
| `session_id` / `sessionId` | conversation | [doc][local] | `result` envelope, `agents --json`, every hook, every transcript line |
| `prompt_id` | **one turn** | [doc] | hook common input fields |
| `uuid` | one record / one result | [local] | transcript lines, `result` envelope |
| `parentUuid` | record chain | [local] | transcript lines — threads records into a tree |
| `tool_use_id` | one tool call | [doc] | `PostToolUse` hook |
| `parent_tool_use_id` | subagent attribution | [doc] | `stream-json` messages; `null` for the main conversation |
| `agent_id` | one subagent run | [doc] | `SubagentStop` hook |
| `requestId` | one API request | [local] | transcript lines |
| `gitBranch`, `cwd` | worktree binding | [local] | every transcript line |

**`prompt_id` is the documented turn identifier**, and it arrives on every hook event — so a
`UserPromptSubmit` → `PostToolUse`* → `Stop` sequence sharing one `prompt_id` is a complete turn
with its tool calls, pushed live, no transcript parsing needed. `gitBranch` and `cwd` on every
transcript line give the worktree binding needed to tie a turn to a diff.

**Turn → diff. Nothing native was found.** No field was found, in docs or in the transcript, that
records the commit or patch a turn produced. The materials for deriving it exist (`cwd`,
`gitBranch`, `prompt_id`, `tool_use_id`, the `file-history-snapshot` transcript record type seen
[local], and `~/.claude/file-history/`), but no documented turn→diff mapping: **not found**. Compare
opencode, which has one (§3.6).

Session-identity edge cases worth knowing [doc]: `/branch` and `--fork-session` create new session
IDs; `claude --resume <session-id>` resolves across projects on the machine but "only when exactly
one other project holds a transcript with messages for it"; resuming the same session in two
terminals without forking "interleaves messages from both into one transcript" — which would
corrupt turn attribution if an orchestrator and a human both drive one session.

---

## 3. opencode (v1.18.0)

### 3.1 Prompt injection

**One-shot. [local]** `opencode run [message..]`, with `--format default|json` ("raw JSON
events"), `-c/--continue`, `-s/--session <id>`, `--fork`, `--agent`, `--model`, `-f/--file`,
`--title`, `--auto` (auto-approve), `--command`, and `--attach <url>` to run against an already
running server. Top-level `--prompt` also exists on the default TUI command.

**Into a live session — first-class, over HTTP. [local]** `opencode serve` starts a headless HTTP
server (default port 4096, hostname 127.0.0.1 [doc]; `--port`/`--hostname`/`--cors` and
`OPENCODE_SERVER_PASSWORD` basic auth). Verified by starting it and reading its own OpenAPI
document from `GET /doc`. Relevant endpoints:

| Endpoint | Purpose |
| --- | --- |
| `POST /api/session/{sessionID}/prompt` | `v2.session.prompt` — "Send message" (waits) |
| `POST /session/{sessionID}/prompt_async` | `session.prompt_async` — "Send async message" |
| `POST /session/{sessionID}/command` | run a slash command |
| `POST /api/session/{sessionID}/interrupt`, `POST /session/{sessionID}/abort` | stop the turn |
| `POST /api/session/{sessionID}/wait` | `v2.session.wait` — "Wait for a session agent loo…" (204 No Content) |
| `POST /session/{sessionID}/fork` | fork the conversation |
| `POST /api/session/{sessionID}/question/{requestID}/reply` \| `/reject` | answer a question |
| `POST /api/session/{sessionID}/permission/{requestID}/reply` | answer a permission request |

**Into a live TUI, without keystrokes. [local]** The server exposes TUI control endpoints —
verified present in the OpenAPI document of a running `opencode serve`:

```
POST /tui/append-prompt      POST /tui/submit-prompt      POST /tui/clear-prompt
POST /tui/execute-command    POST /tui/show-toast         POST /tui/select-session
POST /tui/publish            POST /tui/open-sessions      POST /tui/open-models
POST /tui/open-themes        POST /tui/open-help
GET  /tui/control/next       POST /tui/control/response
```

Matching bus events exist as first-class schemas: `tui.prompt.append`, `tui.command.execute`,
`tui.session.select`, `tui.toast.show`. So `append-prompt` + `submit-prompt` is a supported,
documented-in-schema way to put a prompt into a human's live opencode TUI and send it, without
PTY keystroke injection and without taking the TUI away. No equivalent was found for claude-code.

`opencode attach <url>` attaches to a running server; `GET /api/session/active` and
`GET /session/status` exist for discovering what is live. opencode also manages PTYs itself:
`GET/POST /pty`, `/pty/{ptyID}`, `/pty/{ptyID}/connect`, `/pty/{ptyID}/connect-token`,
`/pty/shells`, with `pty.created` / `pty.updated` / `pty.exited` / `pty.deleted` events.

**ACP. [local]** `opencode acp` — see §3.5. Prompt injection there is `session/prompt`.

### 3.2 Turn-end / waiting / failed, without parsing the TUI

**This is opencode's strongest area.** The SSE event bus is the contract. `GET /event` ("First
event is `server.connected`, then bus events") [doc], plus `GET /global/event` and a per-session
`GET /api/session/{sessionID}/event` [local]. Every event is
`{ id: "evt_…", type: "<dotted.name>", properties: { … } }`.

Verified literal event types and payloads, read from the running server's OpenAPI document [local]:

```jsonc
// turn finished
{"type":"session.idle",   "properties":{"sessionID":"ses…"}}
// coarse status, with a typed state machine
{"type":"session.status", "properties":{"sessionID":"ses…","status":<SessionStatus>}}
// turn failed, with a typed error union
{"type":"session.error",  "properties":{"sessionID":"ses…","error":
   ProviderAuthError | UnknownError | MessageOutputLengthError | MessageAbortedError |
   StructuredOutputError | ContextOverflowError | ContentFilterError | APIError }}
// waiting for a human
{"type":"question.asked", "properties":{"id":"que…","sessionID":"ses…","questions":[…],"tool":…}}
{"type":"permission.asked", …}   // also permission.v2.asked / .replied, question.v2.*
```

`SessionStatus` is a discriminated union; observed variants include `{"type":"idle"}` and
`{"type":"retry", "attempt":…, "message":…, "action":{reason,provider,title,message,label,link}, "next":…}`.
The full variant list was not enumerated.

So the three states the issue asks about map to three distinct, typed, first-party events:
**finished = `session.idle`**, **waiting for input = `question.asked` / `permission.asked`**,
**failed = `session.error`** with a discriminated error type. No TUI parsing, no heuristics.

Fine-grained turn progress is also available, as a `session.next.*` family [local] — verified
literal names: `session.next.prompted`, `session.next.prompt.admitted`, `session.next.step.started`,
`session.next.step.ended`, `session.next.step.failed`, `session.next.text.started` / `.delta` /
`.ended`, `session.next.reasoning.started` / `.delta` / `.ended`,
`session.next.tool.input.started` / `.delta` / `.ended`, `session.next.tool.called`,
`session.next.tool.progress`, `session.next.tool.success`, `session.next.tool.failed`,
`session.next.shell.started` / `.ended`, `session.next.retried`, `session.next.synthetic`,
`session.next.agent.switched`, `session.next.model.switched`, `session.next.moved`,
`session.next.context.updated`, `session.next.compaction.started` / `.delta` / `.ended`,
`session.next.revert.staged` / `.cleared` / `.committed`. Plus `message.updated`,
`message.part.updated`, `message.part.delta`, `message.removed`, `session.created`,
`session.updated`, `session.deleted`, `session.compacted`, `session.diff`, `file.edited`,
`vcs.branch.updated`, `workspace.ready` / `.failed` / `.status`, `worktree.ready` / `.failed`.

The `session.next.*` naming is itself a signal: it reads as a v2 event family coexisting with the
older flat names (`permission.asked` alongside `permission.v2.asked`; `question.*` alongside
`question.v2.*`). Treat the `next`/`v2` split as in-flight API evolution.

**Blocking alternative to the event stream. [local]** `POST /api/session/{sessionID}/wait`
("Wait for a session agent loo…", 204 on completion, with typed 400/401/404/503) lets an
orchestrator await turn end without holding an SSE subscription.

### 3.3 Capturing what the agent did without destroying its TUI

**[local]** All of these are read paths that leave the TUI alone:

- `GET /session/{sessionID}/message`, `GET /session/{sessionID}/message/{messageID}`,
  `GET /session/{sessionID}/message/{messageID}/part/{partID}` — the structured conversation.
- `GET /session/{sessionID}/diff` → `SnapshotFileDiff[]` — the working-tree diff, from the server.
- `GET /api/session/{sessionID}/history`, `/context`, `/children`.
- `opencode export [sessionID]` — "export session data as JSON", with `--sanitize` to "redact
  sensitive transcript and file data". `opencode import <file>` is the inverse.
- `opencode session list` / `opencode session delete <sessionID>`, `opencode stats`.
- `opencode db [query] --format json|tsv` — read the SQLite store directly (§3.4).

A TUI driven by `opencode serve` is just one more client of the same server, so reading is
observation, not interception.

### 3.4 Structured transcript on disk

**Location and format. [local]** SQLite. `opencode db path` printed
`/Users/marivaldocavalheiro/.local/share/opencode/opencode.db` — a single global database, not
per-project. Note this is the **native CLI location**; Orca's `opencode-database-discovery.ts`
[src] implies the path is discovered rather than assumed, so do not hardcode it.

Verified tables (`.tables`): `__drizzle_migrations`, `account`, `account_state`, `control_account`,
`credential`, `data_migration`, `event`, `event_sequence`, `message`, `migration`, `part`,
`permission`, `project`, `project_directory`, `session`, `session_context_epoch`,
`session_input`, `session_message`, `session_share`, `todo`, `workspace`.

The parts that matter for an orchestrator, from `.schema` [local]:

- `session(id, project_id, parent_id, slug, directory, title, version, share_url,
  summary_additions, summary_deletions, summary_files, summary_diffs, revert, permission,
  time_created, time_updated, time_compacting, time_archived, workspace_id, path, agent, model,
  cost, tokens_input, tokens_output, tokens_reasoning, tokens_cache_read, tokens_cache_write,
  metadata)` — note `directory`, `path`, `agent`, `model`, and the `summary_*` diff rollup
  (`summary_diffs` is a text column) living on the session row.
- `message(id, session_id, time_created, time_updated, data)` and
  `part(id, message_id, session_id, time_created, time_updated, data)` — the conversation. The
  payload is an opaque JSON `data` blob in both; the SQL schema does **not** type the message
  content, so reading it still means parsing JSON whose shape the OpenAPI document describes.
- `session_message(id, session_id, type, time_created, time_updated, data, seq)` with
  `UNIQUE (session_id, seq)` — a newer, strictly sequenced message log alongside `message`. Two
  message tables coexisting is itself evidence of migration in progress.
- `session_input(id, session_id, prompt, delivery, admitted_seq, promoted_seq, time_created)` —
  **queued inbound prompts, with a `delivery` column and an admitted/promoted sequence pair.** This
  is the injection queue made durable; it pairs with the `session.next.prompt.admitted` event.
- `event(id, aggregate_id, seq, type, data)` + `event_sequence(aggregate_id, seq, owner_id)`, with
  `UNIQUE (aggregate_id, seq)` — an event-sourcing log.
- `workspace(id, type, name, branch, directory, extra, project_id, time_used)` and
  `project_directory(project_id, directory, type, strategy, …)` — worktree/branch awareness is
  modelled in the schema.

**Is it stable enough to depend on?** More stable than claude's JSONL in form, less stable than it
looks in practice, and **no first-party stability promise was found**.

Evidence that it moves [local]: `__drizzle_migrations` holds **20 applied migrations**; the most
recent eight are `20260511000411_data_migration_state`, `20260510033149_session_usage`,
`20260507164347_add_workspace_time`, `20260504145000_add_sync_owner`, `20260501142318_next_venus`,
`20260427172553_slow_nightmare`, `20260428004200_add_session_path`,
`20260423070820_add_icon_url_override`. Several are recent and additive-but-renaming
(`add_session_path`, `session_usage`, `next_venus`). A `data_migration` table records content
migrations too (`session_usage_from_messages`). Many columns are visibly bolted on after the fact
(the trailing `, workspace_id text, path text, agent text, model text, cost real DEFAULT 0 …` on
`session`), and `CREATE TABLE IF NOT EXISTS "workspace"` / `"session_input"` / `"migration"` /
`"project_directory"` indicate tables recreated by migration.

Mitigations that exist and do not for claude: the schema is migration-versioned and the version is
queryable (`SELECT name FROM __drizzle_migrations`), so a reader can gate on it; `opencode export`
and `opencode db --format json` are first-party read paths; and the HTTP API (§3.3) is a documented
interface over the same data, which is the stable surface. **Nothing in opencode's documentation
was found that either blesses or forbids reading `opencode.db` directly: not found.**

### 3.5 Structured protocol (ACP)

**Yes, natively, today. [local]** `opencode acp` is a first-class subcommand: "start ACP (Agent
Client Protocol) server". Documented as starting "opencode as an ACP-compatible subprocess that
communicates with your editor over JSON-RPC via stdio" [doc].

Verified by sending a real `initialize` request over stdio and capturing the reply — exact
response, unedited [local]:

```json
{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,
 "agentCapabilities":{"loadSession":true,
   "mcpCapabilities":{"http":true,"sse":true},
   "promptCapabilities":{"embeddedContext":true,"image":true},
   "sessionCapabilities":{"close":{},"fork":{},"list":{},"resume":{}}},
 "authMethods":[{"description":"Run `opencode auth login` in the terminal",
   "name":"Login with opencode","id":"opencode-login"}],
 "agentInfo":{"name":"OpenCode","version":"1.18.0"}}}
```

So: **ACP protocol version 1**, with `loadSession`, and session `close`/`fork`/`list`/`resume`.
`fork` and `resume` over ACP matter for an orchestrator that wants branch-per-attempt.

**What ACP gives you, from the spec. [doc]** A turn starts with a `session/prompt` request carrying
`sessionId` and a `prompt` array of `ContentBlock`s. During the turn the agent pushes
`session/update` notifications, discriminated by a `sessionUpdate` field, with these variants:
`user_message_chunk`, `agent_message_chunk`, `agent_thought_chunk`, `tool_call`,
`tool_call_update`, `plan`, `available_commands_update`, `current_mode_update`,
`config_option_update`, `session_info_update`, `usage_update`. Chunks carry an optional `messageId`,
and "identical IDs indicate chunks belonging to the same message". The turn ends with a `StopReason`:

| `StopReason` | Meaning |
| --- | --- |
| `end_turn` | model finished without requesting tools |
| `max_tokens` | token limit reached |
| `max_turn_requests` | model request limit exceeded |
| `refusal` | agent declines to continue |
| `cancelled` | client cancelled the turn |

Cancellation is a `session/cancel` notification; the spec requires the agent to return the
`cancelled` stop reason rather than an error response, and says the client "SHOULD preemptively
mark all non-finished tool calls pertaining to the current turn as `cancelled`".
`session/new` returns a `sessionId`, "Unique identifier for the created session. Used in all
subsequent requests for this conversation." `session/list`, `session/resume` and `session/load` are
capability-gated.

**ACP limitations in opencode. [doc]** "Some built-in slash commands like `/undo` and `/redo` are
currently unsupported." Everything else is stated to work identically to terminal usage, including
built-in tools, custom tools, MCP servers, project rules, formatters, and the agents/permissions
system. Known ACP clients named in opencode's docs: Zed (via the Zed ACP Registry), JetBrains IDEs
(via `acp.json`), **Avante.nvim and CodeCompanion.nvim** — directly relevant to a Neovim-first
design, because an ACP path means not being the first Neovim client to drive opencode.

**ACP spec stability: not found.** The spec's schema page, as read, states no versioning-stability
or deprecation policy; it only describes negotiation ("The agent should respond with its supported
protocol version and capabilities" — either the client's version if supported, "or the latest
protocol version supported by the agent"). The spec's own `PROTOCOL_VERSION` constant was not
found on the page read; opencode advertises `1` [local].

### 3.6 What identifies a turn

**[local], from the OpenAPI document of the running server.** Opaque prefixed IDs, with the prefix
enforced in the schema as a regex:

| Prefix | Entity |
| --- | --- |
| `ses…` | session (`^ses`) |
| `msg…` | message (`^msg`) |
| `prt…` | message part (`^prt`) |
| `evt_…` | bus event (`^evt_`) |
| `que…` | question (`^que`) |

`AssistantMessage` fields: `id` (`^msg`), `sessionID`, `parentID` (`^msg` — messages chain),
`agent`, `mode`, `modelID`, `providerID`, `variant`, `role`, `cost`, `tokens`, `time`, `finish`
(a bare `string`; the enum is **not found** in the schema), `error`, `structured`, `summary`, and
`path: {cwd, root}` — so **each assistant message carries the working directory and repo root it
ran in**, which is the worktree binding, per message rather than per session.

**Turn → diff. This is the one native, structured answer found in either agent. [local]** Two part
types bind a tree state to a specific message:

```jsonc
// PatchPart
{"id":"prt…","sessionID":"ses…","messageID":"msg…","type":"patch",
 "hash":"<string>","files":["<path>", …]}            // all required

// SnapshotPart
{"id":"prt…","sessionID":"ses…","messageID":"msg…","type":"snapshot",
 "snapshot":"<string>"}                               // all required
```

`StepFinishPart` also carries `snapshot` alongside `cost`, `tokens`, `reason`, `messageID`,
`sessionID`. And `session.diff` events / `GET /session/{sessionID}/diff` return
`SnapshotFileDiff { file, patch, additions, deletions, status: added|deleted|modified }`.

So for opencode the chain **review comment → file+line → `PatchPart.files` + `hash` → `messageID`
→ `sessionID`** is available from first-party structured data. Full part type list [local]:
`AgentPart`, `CompactionPart`, `FilePart`, `PatchPart`, `ReasoningPart`, `RetryPart`,
`SnapshotPart`, `StepFinishPart`, `StepStartPart`, `SubtaskPart`, `TextPart`, `ToolPart`.

Over ACP [doc], the identifier available is the `sessionId` plus `messageId` on update chunks. Note
these are ACP's own identifiers; whether they are the same `ses…`/`msg…` values opencode uses
internally was **not tested**.

---

## 4. Side by side

| Contract element | claude-code 2.1.293 | opencode 1.18.0 |
| --- | --- | --- |
| One-shot prompt | `claude -p`, `--output-format json\|stream-json` | `opencode run`, `--format json` |
| Streaming input | `--input-format stream-json` (with `-p` only) | HTTP `prompt` / `prompt_async`; ACP `session/prompt` |
| Inject into live session | `claude --resume <id> "prompt"`, **background sessions only**, mutually exclusive with output-format flags | `POST /api/session/{id}/prompt` or `/prompt_async` |
| Inject into live **TUI** | **not found** (PTY keystrokes only) | `POST /tui/append-prompt` + `/tui/submit-prompt` |
| Turn finished | `Stop` hook (`last_assistant_message`); `result` message in `-p`; `status:"idle"` in `agents --json` | `session.idle` event; `POST /api/session/{id}/wait` |
| Waiting for input | `Notification` hook; **no distinct roster status found** | `question.asked`, `permission.asked` (+ `.v2.`) |
| Failed | `is_error` / `subtype` / `api_error_status` in `result`; `system/api_retry` with error-category enum | `session.error` with 8-variant typed error union |
| Live process list | `claude agents --json` (pid, cwd, kind, sessionId, name, status) | `GET /api/session/active`, `GET /session/status` |
| Read output, TUI intact | `claude logs <id>` (background); `transcript_path` via hooks | whole HTTP read API; `opencode export`; `opencode db` |
| On-disk transcript | JSONL, `~/.claude/projects/<project>/<session-id>.jsonl` | SQLite, `~/.local/share/opencode/opencode.db` (global) |
| Stability of that store | **Docs say it breaks on any release** | 20 migrations applied; **no stability statement found** |
| Structured protocol | **No ACP.** Own JSON/stream-json + Agent SDK; MCP client | **ACP v1 native** via `opencode acp` over stdio; plus full HTTP+SSE API and OpenAPI at `GET /doc` |
| Turn id | `prompt_id` (hooks); `session_id`; `uuid`/`parentUuid` | `msg…` + `ses…`; `parentID` chain; `prt…` parts |
| Turn → diff | **not found** | `PatchPart{messageID, hash, files[]}`, `SnapshotPart`, `session.diff` |
| Worktree binding | `cwd` + `gitBranch` on transcript lines; project dir per worktree | `AssistantMessage.path{cwd,root}`; `workspace.branch`; `session.directory` |

---

## 5. Explicit "not found"

1. A documented way to inject a prompt into a **foreground interactive** `claude` session that the
   orchestrator did not start as a background session, other than PTY keystrokes.
2. The full enum of `status` in `claude agents --json`. Only `idle` and `busy` were observed; no
   distinct "waiting for input" or "errored" value was seen or documented. The full enum of `kind`
   likewise — only `interactive` observed.
3. Any native claude-code mapping from a turn to the diff it produced.
4. Whether `~/.claude/daemon/roster.json` is a supported interface. It is suggestive: worker entries
   carry `pid`, `procStart`, `sessionId`, `cwd`, `worktreePath`, `cliVersion`, `startedAt`,
   `attempt`, `dispatch`, `replPid`, and — notably — `rendezvousSock`, `ptySock`, `rvAuth`,
   `ptyAuth` [local]. That is Claude Code's own daemon owning PTYs over authed unix sockets. It is
   undocumented, unversioned beyond a `proto` field, and was **not** exercised.
5. Any first-party opencode statement blessing or forbidding direct reads of `opencode.db`.
6. The ACP spec's stability/deprecation policy, and its `PROTOCOL_VERSION` constant, on the schema
   page read. opencode advertises `1`.
7. The enum of `AssistantMessage.finish` in opencode (typed as a bare `string`), and the full
   variant list of `SessionStatus` (only `idle` and `retry` were read).
8. Whether ACP's `sessionId`/`messageId` as seen by an ACP client are the same `ses…`/`msg…` values
   in opencode's own store.
9. A first-party list of ACP agents/clients. The spec's introduction page names none; the lists used
   here come from opencode's own ACP page.
10. Whether a native ACP bridge for claude-code exists. Secondary sources describe third-party
    adapters; not verified at a primary source.

---

## 6. Sources

**Local verification** (commands run on this machine, 2026-10-07): `claude --version`,
`claude --help`, `claude agents --help`, `claude agents --json`, `claude attach --help`,
`claude -p --session-id <uuid> --output-format json`; `opencode --version`, `opencode --help`,
`opencode acp --help`, `opencode run --help`, `opencode serve --help`, `opencode session --help`,
`opencode export --help`, `opencode db --help`, `opencode db path`, `opencode acp` driven with a
JSON-RPC `initialize` over stdio, `opencode serve --port 47331` followed by `GET /doc`;
`sqlite3 ~/.local/share/opencode/opencode.db ".tables"` and `".schema"`, `SELECT` on
`__drizzle_migrations` and `data_migration`; `jq` over
`~/.claude/projects/<project>/<session-id>.jsonl` (key names and `type`/`entrypoint` values only —
no conversation content was read or reproduced); directory listings under `~/.claude/`.

**Claude Code documentation** (all first-party, `code.claude.com`):

- Non-interactive / headless mode, `-p`, output formats, `stream-json` event shapes, `--bare`,
  SIGTERM behaviour, `--continue` / `--resume`: https://code.claude.com/docs/en/headless
- Hook events and common input fields (`session_id`, `prompt_id`, `transcript_path`, `cwd`),
  `Stop` / `SubagentStop` / `SessionEnd` / `PostToolUse` / `UserPromptSubmit` / `Notification`,
  and the `transcript_path` async-lag warning: https://code.claude.com/docs/en/hooks
- Transcript location and format, the "internal … can break on any release" statement, storage
  env vars, session naming/branching/resume semantics, sending a prompt to a running background
  session, script interfaces: https://code.claude.com/docs/en/sessions
- Documentation index, used as the negative evidence for ACP (0 matches in 411 lines):
  https://code.claude.com/docs/llms.txt

**opencode documentation** (first-party, `opencode.ai`):

- Headless server, `opencode serve`, endpoint list, SSE `/event` and `server.connected`,
  default port 4096 and `OPENCODE_SERVER_PASSWORD`: https://opencode.ai/docs/server/
- ACP support, `opencode acp` over JSON-RPC/stdio, client list (Zed, JetBrains, Avante.nvim,
  CodeCompanion.nvim), `/undo`+`/redo` limitation: https://opencode.ai/docs/acp/
- SDK event subscription (`client.event.subscribe()`, `{type, properties}`); does **not** enumerate
  event types — those were taken from the running server's OpenAPI document instead:
  https://opencode.ai/docs/sdk/
- Machine-readable and authoritative for §3: the OpenAPI document served by the running instance at
  `GET http://127.0.0.1:<port>/doc` (`info.title` "opencode", `info.version` "1.0.0").

**Agent Client Protocol specification** (`agentclientprotocol.com`):

- Prompt turn lifecycle, `session/prompt`, the `sessionUpdate` variants, the `StopReason` enum,
  `session/cancel` semantics: https://agentclientprotocol.com/protocol/prompt-turn
- Schema, `initialize` version negotiation, `session/new` → `sessionId`,
  `session/list` / `session/resume` / `session/load` capability gating:
  https://agentclientprotocol.com/protocol/schema
- Introduction, checked for an agent/client list and found to contain none:
  https://agentclientprotocol.com/overview/introduction

**Third-party source, read as evidence of prior art only** (untrusted data; no instruction in it
was followed): `stablyai/orca`, TypeScript, public, 87k stars, pushed 2026-10-07 —
`src/shared/agent-process-recognition.ts` (+ `.test.ts`),
`src/shared/dsb-native-one-shot-recognition.test.ts`, `src/shared/agent-headless-command.ts`,
`src/main/native-chat/transcript-reader.ts`, `src/main/native-chat/session-file-resolver.ts`,
`src/main/ai-vault/session-scanner-opencode-sqlite-open.ts`,
`src/main/opencode-usage/opencode-database-discovery.ts`, `src/shared/opencode-database-name.ts`,
the `node-pty` modules listed in §1, and `config/scripts/acp/generate-protocol.mjs`.
