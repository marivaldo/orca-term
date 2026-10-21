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
//   {"type":"permission","session","parent","permission","input"}
//   {"type":"question","session","parent","question"}  opencode waits for the person
//   {"type":"replied","session","parent"}              the person answered
//   {"type":"idle","session","parent","text"}          the session stopped, with its last words
//   {"type":"error","session","parent","name","message"}
//   {"type":"end"}                                     opencode is shutting down
//
// `parent` is the session a subagent's session was made for, null for the person's own.

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
  const where = (sessionID) => ({ session: sessionID ?? null, parent: parents.get(sessionID) ?? null })

  send({ type: "start" })
  return {
    "chat.message": async (input, output) => {
      const parts = Array.isArray(output?.parts) ? output.parts : []
      const typed = parts.filter((part) => part?.type === "text" && !part.synthetic)
      const text = typed.map((part) => part.text).join("\n")
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
        case "session.idle":
          send({ type: "idle", ...where(properties.sessionID), text: lastText.get(properties.sessionID) ?? null })
          lastText.delete(properties.sessionID)
          break
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
