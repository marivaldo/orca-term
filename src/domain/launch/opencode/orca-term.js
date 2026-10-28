// orca-term's opencode plugin (#35). orca-term writes it into the worktree's own opencode config
// directory, which OPENCODE_CONFIG_DIR names, before each launch: nothing goes into opencode's
// global config.
//
// It reports what opencode does to orca-term, one JSON line per event, written to the pipe the
// worktree's holder hands opencode as the file descriptor ORCA_TERM_EVENTS_FD. The holder runs
// `orca-term hook opencode` outside the sandbox for each line, which turns it into the worktree's
// status and turns exactly as claude's hooks do (`src/domain/hook/opencode.rs` reads the lines).
//
// It imports only Node's built-in fs, so opencode installs nothing for it, and it never throws
// into opencode: a line that cannot be written is dropped. Every line is one of:
//
//   {"type":"start"}                                   opencode started
//   {"type":"prompt","session","parent","text"}        the person sent a message
//   {"type":"tool","session","parent","tool","input"}  a tool is about to run
//   {"type":"tool-error","session","parent","tool","input","call","error"}
//                                                      a tool call ended in an error
//   {"type":"permission","session","parent","permission","input"}
//   {"type":"question","session","parent","question"}  opencode waits for the person
//   {"type":"replied","session","parent"}              the person answered
//   {"type":"idle","session","parent","text","finish","error"}
//                                                      the session stopped
//   {"type":"error","session","parent","name","message"}
//   {"type":"end"}                                     opencode is shutting down
//
// `parent` is the session a subagent's session was made for, null for the person's own. An idle
// line carries the agent's last words, and how its latest message since the person's prompt
// ended: its `finish` reason (null when it has none) and its `error`'s name (null when none).
// opencode says nothing else when the person presses Esc while it waits to retry its provider
// (measured on 1.18.0: no `session.error`, the message completed with neither), so this is how
// orca-term tells an interrupted turn from a finished one.
//
// A tool-error line carries the call's id (`call`) and opencode's error text, which orca-term reads
// to tell a deny rule's refusal from any other failure. opencode has no other signal for it (read
// in 1.18.0's source): a `deny` rule in its `permission` config refuses the call inside the tool,
// after `tool.execute.before` has fired, with a `PermissionDeniedError` and no `permission.asked`
// (packages/opencode/src/permission/index.ts:72-79); `tool.execute.after` never fires and the
// `permission.ask` hook is never called; the tool part just ends with `state.status` "error" and
// `state.error` the error's message (packages/opencode/src/session/processor.ts:186-205), "The user
// has specified a rule which prevents you from using this specific tool call. ..."
// (packages/core/src/v1/permission.ts:21-27). The person's Reject at a permission prompt ends the
// same way with "The user rejected permission to use this specific tool call." (:7-11), after a
// `permission.asked` and its reply; a command the sandbox stopped is not even an error, just a
// command that exited non-zero (packages/opencode/src/tool/shell.ts:586-594).

import { writeSync } from "node:fs"

/** The pipe's file descriptor; anything but a number above the standard streams writes nothing. */
const FD = Number(process.env.ORCA_TERM_EVENTS_FD)

/** The most characters of any text a line carries; orca-term keeps far fewer. */
const MAX_TEXT = 2000

/** The fields of a tool's input, or of a permission's details, that say what it works on. */
const INPUT_FIELDS = ["command", "filePath", "filepath", "pattern", "url", "query"]

/** Writes `line` as one JSON line to the pipe, whole, or drops it. */
function send(line) {
  if (!Number.isInteger(FD) || FD < 3) return
  try {
    const bytes = Buffer.from(JSON.stringify(line) + "\n")
    let written = 0
    while (written < bytes.length) written += writeSync(FD, bytes, written)
  } catch {
    // The pipe is gone or full: the line is lost, opencode carries on.
  }
}

/** `text` cut to MAX_TEXT characters, or null when it is not text. */
function cut(text) {
  return typeof text === "string" ? text.slice(0, MAX_TEXT) : null
}

/** The text fields of `input` that say what a tool works on, each cut. */
function pick(input) {
  const picked = {}
  for (const field of INPUT_FIELDS) {
    if (typeof input?.[field] === "string") picked[field] = cut(input[field])
  }
  return picked
}

export const OrcaTerm = async () => {
  /** Each subagent's session, by its id, with the session it was made for. */
  const parents = new Map()
  /** What the agent said last in each session, by its id. */
  const lastText = new Map()
  /** How the agent's latest message ended in each session, by its id, since the person's prompt. */
  const lastEnd = new Map()
  const where = (sessionID) => ({ session: sessionID ?? null, parent: parents.get(sessionID) ?? null })

  send({ type: "start" })
  return {
    "chat.message": async (input, output) => {
      const parts = Array.isArray(output?.parts) ? output.parts : []
      const typed = parts.filter((part) => part?.type === "text" && !part.synthetic)
      const text = typed.map((part) => part.text).join("\n")
      lastEnd.delete(input?.sessionID)
      send({ type: "prompt", ...where(input?.sessionID), text: cut(text) ?? "" })
    },
    "tool.execute.before": async (input, output) => {
      send({ type: "tool", ...where(input?.sessionID), tool: cut(input?.tool), input: pick(output?.args) })
    },
    "experimental.text.complete": async (input, output) => {
      if (typeof output?.text === "string" && output.text.trim() !== "") {
        lastText.set(input?.sessionID, cut(output.text))
      }
    },
    event: async ({ event }) => {
      const properties = event?.properties ?? {}
      switch (event?.type) {
        case "session.created":
        case "session.updated":
          if (properties.info?.parentID) parents.set(properties.info.id, properties.info.parentID)
          break
        case "message.updated":
          if (properties.info?.role === "assistant") {
            lastEnd.set(properties.info.sessionID, {
              finish: cut(properties.info.finish),
              error: cut(properties.info.error?.name),
            })
          }
          break
        case "message.part.updated": {
          const part = properties.part
          if (part?.type === "tool" && part.state?.status === "error") {
            send({
              type: "tool-error",
              ...where(part.sessionID),
              tool: cut(part.tool),
              input: pick(part.state.input),
              call: cut(part.callID),
              error: cut(part.state.error),
            })
          }
          break
        }
        case "permission.asked":
          send({
            type: "permission",
            ...where(properties.sessionID),
            permission: cut(properties.permission),
            input: pick(properties.metadata),
          })
          break
        case "question.asked":
          send({ type: "question", ...where(properties.sessionID), question: cut(properties.questions?.[0]?.question) })
          break
        case "permission.replied":
        case "question.replied":
        case "question.rejected":
          send({ type: "replied", ...where(properties.sessionID) })
          break
        case "session.idle": {
          const sessionID = properties.sessionID
          const end = lastEnd.get(sessionID)
          send({
            type: "idle",
            ...where(sessionID),
            text: lastText.get(sessionID) ?? null,
            finish: end?.finish ?? null,
            error: end?.error ?? null,
          })
          lastText.delete(sessionID)
          lastEnd.delete(sessionID)
          break
        }
        case "session.error":
          send({
            type: "error",
            ...where(properties.sessionID),
            name: cut(properties.error?.name),
            message: cut(properties.error?.data?.message),
          })
          break
      }
    },
    dispose: async () => {
      send({ type: "end" })
    },
  }
}
