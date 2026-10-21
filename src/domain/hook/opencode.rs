//! Domain: the lines opencode's plugin writes (`domain/launch/opencode/orca-term.js`), each read as
//! the payload claude's hook of the same meaning carries (#35), so `hook::apply` makes the same
//! transitions for both agents:
//!
//! | line         | claude's event                      | what it does to the turn            |
//! |--------------|-------------------------------------|-------------------------------------|
//! | `start`      | `SessionStart` (`startup`)          | closes a turn left open, interrupted |
//! | `prompt`     | `UserPromptSubmit`                  | opens a turn, or queues the prompt   |
//! | `tool`       | `PreToolUse`                        | working, on that tool                |
//! | `permission` | `PermissionRequest`                 | waiting, for that tool               |
//! | `question`   | `PermissionRequest` of a question   | waiting, showing the question        |
//! | `replied`    | `PostToolUse`                       | working again                        |
//! | `idle`       | `Stop`, with the agent's last words | finished, its end observed           |
//! | `error`      | `StopFailure`                       | failed, with the error               |
//! | `end`        | `SessionEnd`                        | closes a turn left open, interrupted |
//!
//! The prompt's id is its session's: opencode folds a message sent while a turn runs into that
//! turn, as claude does with a queued prompt. Every other line carries no id, so it belongs to the
//! open turn and never opens one. A session made for a subagent (`parent` names the session it was
//! made for) is reported as claude reports a subagent: under the person's session, with an agent
//! id, so only its permissions and questions make the turn wait.
//!
//! claude has no event for Esc. opencode reports it as the error `MessageAbortedError`, which is
//! read as `SessionEnd`, the event that closes the open turn as interrupted with its end not
//! observed. `ContextOverflowError` is ignored: opencode compacts the session and carries on.

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};

use crate::domain::hook::Payload;

/// The error opencode reports when the person stops a turn.
const ABORTED: &str = "MessageAbortedError";

/// The error opencode reports before it compacts the session and carries on.
const CONTEXT_OVERFLOW: &str = "ContextOverflowError";

/// The tool claude asks its questions with: a question makes the turn wait, showing the question.
const QUESTION_TOOL: &str = "AskUserQuestion";

/// The payload the line `bytes` reports, or `None` for a line that changes nothing: a subagent's
/// prompt, a message with no text of the person's, an error opencode recovers from, a line of no
/// session, a line of a type this core does not know. Fails when the line is not a JSON object
/// with a `type`.
pub(crate) fn payload(bytes: &[u8]) -> Result<Option<Payload>> {
    let value: Value =
        serde_json::from_slice(bytes).context("opencode's event is not a JSON line")?;
    let Some(kind) = value.get("type").and_then(Value::as_str) else {
        bail!("opencode's event has no type");
    };
    let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
    match kind {
        "start" => Ok(Some(Payload {
            source: Some("startup".to_owned()),
            ..Payload::named("SessionStart")
        })),
        "end" => Ok(Some(Payload::named("SessionEnd"))),
        _ => {
            let Some(session) = text("session") else {
                return Ok(None);
            };
            let parent = text("parent");
            Ok(session_event(kind, &value, &session, parent.as_deref()))
        }
    }
}

/// The payload of a line of the session `session`, made for a subagent when `parent` names the
/// session it was made for.
fn session_event(kind: &str, line: &Value, session: &str, parent: Option<&str>) -> Option<Payload> {
    let text = |key: &str| line.get(key).and_then(Value::as_str).map(str::to_owned);
    let subagent = parent.is_some();
    // The person's session, and the subagent's own as its agent id, as claude reports a subagent.
    let base = |event: &str| Payload {
        session_id: Some(parent.unwrap_or(session).to_owned()),
        agent_id: subagent.then(|| session.to_owned()),
        ..Payload::named(event)
    };
    match kind {
        "prompt" => {
            let prompt = text("text").filter(|prompt| !prompt.trim().is_empty())?;
            if subagent {
                return None;
            }
            Some(Payload {
                prompt_id: Some(session.to_owned()),
                prompt: Some(prompt),
                ..base("UserPromptSubmit")
            })
        }
        "tool" => Some(Payload {
            tool_name: text("tool").map(|tool| tool_name(&tool)),
            tool_input: tool_input(line.get("input")),
            ..base("PreToolUse")
        }),
        "permission" => Some(Payload {
            tool_name: text("permission").map(|tool| tool_name(&tool)),
            tool_input: tool_input(line.get("input")),
            ..base("PermissionRequest")
        }),
        "question" => Some(Payload {
            tool_name: Some(QUESTION_TOOL.to_owned()),
            tool_input: json!({"questions": [{"question": text("question").unwrap_or_default()}]}),
            ..base("PermissionRequest")
        }),
        "replied" => Some(base("PostToolUse")),
        "idle" => Some(Payload {
            last_message: text("text"),
            ..base("Stop")
        }),
        "error" => error_event(text("name"), text("message"), base),
        _ => None,
    }
}

/// The payload of an error named `name`, saying `message`: the end of an interrupted turn when
/// the person stopped it, nothing when opencode carries on, a failed turn otherwise.
fn error_event(
    name: Option<String>,
    message: Option<String>,
    base: impl Fn(&str) -> Payload,
) -> Option<Payload> {
    match name.as_deref() {
        Some(ABORTED) => Some(base("SessionEnd")),
        Some(CONTEXT_OVERFLOW) => None,
        _ => Some(Payload {
            last_message: message,
            error: name,
            ..base("StopFailure")
        }),
    }
}

/// claude's name for the opencode tool (or permission) `tool`, so the activity reads the same for
/// both agents (`Bash: npm test`); any other tool keeps its own name.
fn tool_name(tool: &str) -> String {
    let claudes = match tool {
        "bash" => "Bash",
        "read" => "Read",
        "edit" => "Edit",
        "write" => "Write",
        "glob" => "Glob",
        "grep" => "Grep",
        "webfetch" => "WebFetch",
        "websearch" => "WebSearch",
        other => other,
    };
    claudes.to_owned()
}

/// The tool's input under claude's field names, from what the plugin picked of opencode's: the
/// command, the file (`filePath`, or `filepath` in a permission's details), the pattern, the url
/// and the query, each when it is text.
fn tool_input(input: Option<&Value>) -> Value {
    let input = input.and_then(Value::as_object);
    let field = |name: &str| {
        input
            .and_then(|input| input.get(name))
            .filter(|value| value.is_string())
    };
    let mut claudes = Map::new();
    let renamed = [
        ("command", "command"),
        ("filePath", "file_path"),
        ("filepath", "file_path"),
        ("pattern", "pattern"),
        ("url", "url"),
        ("query", "query"),
    ];
    for (opencodes, name) in renamed {
        if let Some(value) = field(opencodes) {
            claudes.entry(name).or_insert_with(|| value.clone());
        }
    }
    Value::Object(claudes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::hook::{self, tool_preview};
    use crate::domain::launch::opencode::PLUGIN;
    use crate::domain::turn::{Outcome, Phase, Status};

    fn read(line: &Value) -> Option<Payload> {
        payload(line.to_string().as_bytes()).unwrap()
    }

    fn event(line: &Value) -> Payload {
        read(line).unwrap_or_else(|| panic!("{line} reports nothing"))
    }

    #[test]
    fn a_start_and_an_end_are_claudes_session_start_and_end() {
        let start = event(&json!({"type": "start"}));
        assert_eq!(start.event, "SessionStart");
        assert_eq!(start.source.as_deref(), Some("startup"));
        assert_eq!(start.session_id, None);
        assert_eq!(event(&json!({"type": "end"})).event, "SessionEnd");
    }

    #[test]
    fn a_prompt_opens_a_turn_named_after_its_session() {
        let prompt = event(&json!({
            "type": "prompt", "session": "ses_1", "parent": null, "text": "fix the login"
        }));
        assert_eq!(prompt.event, "UserPromptSubmit");
        assert_eq!(prompt.session_id.as_deref(), Some("ses_1"));
        assert_eq!(prompt.prompt_id.as_deref(), Some("ses_1"));
        assert_eq!(prompt.prompt.as_deref(), Some("fix the login"));
        assert_eq!(prompt.agent_id, None);
        assert_eq!(
            read(&json!({"type": "prompt", "session": "ses_1", "text": "  \n"})),
            None,
            "a message with no text of the person's"
        );
        assert_eq!(
            read(&json!({"type": "prompt", "session": "ses_2", "parent": "ses_1", "text": "go"})),
            None,
            "a subagent's prompt"
        );
    }

    #[test]
    fn tools_and_permissions_read_as_claudes_with_claudes_names() {
        let tool = event(&json!({
            "type": "tool", "session": "ses_1", "tool": "bash", "input": {"command": "npm test"}
        }));
        assert_eq!(tool.event, "PreToolUse");
        assert_eq!(tool.prompt_id, None, "never opens a turn");
        assert_eq!(
            tool_preview(tool.tool_name.as_deref().unwrap(), &tool.tool_input),
            "Bash: npm test"
        );
        let edit = event(&json!({
            "type": "tool", "session": "ses_1", "tool": "edit", "input": {"filePath": "/w/a.rs"}
        }));
        assert_eq!(
            tool_preview(edit.tool_name.as_deref().unwrap(), &edit.tool_input),
            "Edit: /w/a.rs"
        );
        let other = event(&json!({"type": "tool", "session": "ses_1", "tool": "todowrite"}));
        assert_eq!(other.tool_name.as_deref(), Some("todowrite"));
        let permission = event(&json!({
            "type": "permission", "session": "ses_1", "permission": "edit",
            "input": {"filepath": "/w/b.rs"}
        }));
        assert_eq!(permission.event, "PermissionRequest");
        assert_eq!(
            tool_preview(
                permission.tool_name.as_deref().unwrap(),
                &permission.tool_input
            ),
            "Edit: /w/b.rs"
        );
    }

    #[test]
    fn a_question_waits_showing_it_and_a_reply_works_again() {
        let question = event(&json!({
            "type": "question", "session": "ses_1", "question": "Which file?"
        }));
        assert_eq!(question.event, "PermissionRequest");
        assert_eq!(question.tool_name.as_deref(), Some("AskUserQuestion"));
        assert_eq!(
            question.tool_input,
            json!({"questions": [{"question": "Which file?"}]})
        );
        assert_eq!(
            event(&json!({"type": "replied", "session": "ses_1"})).event,
            "PostToolUse"
        );
    }

    #[test]
    fn idle_stops_with_the_last_words_and_errors_fail_or_interrupt() {
        let idle = event(&json!({"type": "idle", "session": "ses_1", "text": "All done."}));
        assert_eq!(idle.event, "Stop");
        assert_eq!(idle.last_message.as_deref(), Some("All done."));
        let failed = event(&json!({
            "type": "error", "session": "ses_1", "name": "APIError", "message": "rate limited"
        }));
        assert_eq!(failed.event, "StopFailure");
        assert_eq!(failed.error.as_deref(), Some("APIError"));
        assert_eq!(failed.last_message.as_deref(), Some("rate limited"));
        let aborted = event(&json!({
            "type": "error", "session": "ses_1", "name": "MessageAbortedError"
        }));
        assert_eq!(aborted.event, "SessionEnd");
        assert_eq!(
            read(&json!({"type": "error", "session": "ses_1", "name": "ContextOverflowError"})),
            None
        );
        assert_eq!(
            read(&json!({"type": "error", "session": null, "name": "UnknownError"})),
            None,
            "an error of no session, such as a plugin that failed to load"
        );
    }

    #[test]
    fn a_subagents_lines_come_under_the_persons_session_with_an_agent_id() {
        let wait = event(&json!({
            "type": "permission", "session": "ses_2", "parent": "ses_1", "permission": "bash"
        }));
        assert_eq!(wait.session_id.as_deref(), Some("ses_1"));
        assert_eq!(wait.agent_id.as_deref(), Some("ses_2"));
    }

    #[test]
    fn a_line_that_is_not_an_event_is_refused_and_an_unknown_one_ignored() {
        assert!(payload(b"not json").is_err());
        assert!(payload(br#"{"session": "ses_1"}"#).is_err());
        assert_eq!(read(&json!({"type": "later", "session": "ses_1"})), None);
    }

    /// Applies `lines` in order from an empty status, numbering turns from 1.
    fn replay(lines: &[Value]) -> (Status, Vec<String>) {
        let mut status = Status::default();
        let mut kinds = Vec::new();
        let mut next = 1;
        for (at, line) in lines.iter().enumerate() {
            let Some(payload) = read(line) else {
                continue;
            };
            let now = u64::try_from(at).unwrap();
            let applied = hook::apply(status, &payload, "opencode", now, next);
            for record in &applied.records {
                kinds.push(String::from_utf8(record.to_line().unwrap()).unwrap());
            }
            next = applied
                .status
                .turn
                .as_ref()
                .map_or(next, |turn| turn.number + 1);
            status = applied.status;
        }
        (status, kinds)
    }

    #[test]
    fn a_turn_runs_waits_and_finishes_as_claudes_does() {
        let (status, _) = replay(&[
            json!({"type": "start"}),
            json!({"type": "prompt", "session": "ses_1", "text": "fix the login"}),
            json!({"type": "tool", "session": "ses_1", "tool": "bash", "input": {"command": "ls"}}),
        ]);
        let turn = status.turn.as_ref().unwrap();
        assert_eq!(turn.phase, Phase::Working);
        assert_eq!(turn.activity, "Bash: ls");
        assert_eq!(turn.agent, "opencode");
        assert_eq!(status.session.as_deref(), Some("ses_1"));
        assert_eq!(status.agent.as_deref(), Some("opencode"));

        let (status, _) = replay(&[
            json!({"type": "prompt", "session": "ses_1", "text": "fix the login"}),
            json!({"type": "question", "session": "ses_1", "question": "Which file?"}),
        ]);
        let turn = status.turn.as_ref().unwrap();
        assert_eq!(turn.phase, Phase::Waiting);
        assert_eq!(turn.activity, "Which file?");

        let (status, _) = replay(&[
            json!({"type": "prompt", "session": "ses_1", "text": "fix the login"}),
            json!({"type": "question", "session": "ses_1", "question": "Which file?"}),
            json!({"type": "replied", "session": "ses_1"}),
            json!({"type": "idle", "session": "ses_1", "text": "All done."}),
            json!({"type": "idle", "session": "ses_1"}),
        ]);
        assert_eq!(status.turn, None);
        let latest = status.latest.as_ref().unwrap();
        assert_eq!(latest.outcome, Outcome::Finished);
        assert!(latest.end_observed);
        assert_eq!(latest.detail.as_deref(), Some("All done."));
    }

    #[test]
    fn a_message_sent_while_a_turn_runs_folds_into_it() {
        let (status, records) = replay(&[
            json!({"type": "prompt", "session": "ses_1", "text": "fix the login"}),
            json!({"type": "prompt", "session": "ses_1", "text": "and the logout"}),
            json!({"type": "idle", "session": "ses_1", "text": "Both done."}),
        ]);
        assert_eq!(records.len(), 2, "one turn, started and ended: {records:?}");
        assert_eq!(status.latest.as_ref().unwrap().outcome, Outcome::Finished);
    }

    #[test]
    fn an_error_fails_the_turn_esc_interrupts_it_and_idle_after_either_is_ignored() {
        let (status, _) = replay(&[
            json!({"type": "prompt", "session": "ses_1", "text": "fix the login"}),
            json!({"type": "error", "session": "ses_1", "name": "APIError", "message": "rate limited"}),
            json!({"type": "idle", "session": "ses_1"}),
        ]);
        let latest = status.latest.as_ref().unwrap();
        assert_eq!(latest.outcome, Outcome::Failed);
        assert_eq!(latest.error.as_deref(), Some("APIError"));
        assert_eq!(latest.message.as_deref(), Some("rate limited"));

        let (status, _) = replay(&[
            json!({"type": "prompt", "session": "ses_1", "text": "fix the login"}),
            json!({"type": "error", "session": "ses_1", "name": "MessageAbortedError"}),
            json!({"type": "idle", "session": "ses_1"}),
        ]);
        let latest = status.latest.as_ref().unwrap();
        assert_eq!(latest.outcome, Outcome::Interrupted);
        assert!(!latest.end_observed);
    }

    #[test]
    fn a_subagent_never_opens_or_closes_a_turn_but_may_make_it_wait() {
        let (status, records) = replay(&[
            json!({"type": "prompt", "session": "ses_1", "text": "look around"}),
            json!({"type": "prompt", "session": "ses_2", "parent": "ses_1", "text": "explore"}),
            json!({"type": "tool", "session": "ses_2", "parent": "ses_1", "tool": "grep"}),
            json!({"type": "permission", "session": "ses_2", "parent": "ses_1", "permission": "bash"}),
        ]);
        assert_eq!(records.len(), 1, "only the person's turn started");
        let turn = status.turn.as_ref().unwrap();
        assert_eq!(turn.phase, Phase::Waiting);
        assert_eq!(status.session.as_deref(), Some("ses_1"));
        let (status, _) = replay(&[
            json!({"type": "prompt", "session": "ses_1", "text": "look around"}),
            json!({"type": "idle", "session": "ses_2", "parent": "ses_1"}),
            json!({"type": "error", "session": "ses_2", "parent": "ses_1", "name": "APIError"}),
        ]);
        assert!(status.turn.is_some(), "the subagent's end ends nothing");
    }

    #[test]
    fn events_with_no_turn_open_open_none() {
        let (status, records) = replay(&[
            json!({"type": "tool", "session": "ses_1", "tool": "bash"}),
            json!({"type": "idle", "session": "ses_1"}),
            json!({"type": "error", "session": "ses_1", "name": "APIError"}),
        ]);
        assert!(records.is_empty(), "{records:?}");
        assert_eq!(status.turn, None);
        assert_eq!(status.latest, None);
    }

    #[test]
    fn a_start_closes_a_turn_left_open_by_an_agent_that_died() {
        let (status, _) = replay(&[
            json!({"type": "prompt", "session": "ses_1", "text": "fix the login"}),
            json!({"type": "start"}),
        ]);
        assert_eq!(status.turn, None);
        assert_eq!(
            status.latest.as_ref().unwrap().outcome,
            Outcome::Interrupted
        );
    }

    #[test]
    fn every_line_type_the_plugin_writes_is_read() {
        let mut types: Vec<&str> = PLUGIN
            .match_indices("type: \"")
            .map(|(at, matched)| {
                let rest = &PLUGIN[at + matched.len()..];
                &rest[..rest.find('"').unwrap()]
            })
            .collect();
        types.sort_unstable();
        types.dedup();
        assert_eq!(
            types,
            [
                "end",
                "error",
                "idle",
                "permission",
                "prompt",
                "question",
                "replied",
                "start",
                "tool"
            ]
        );
        for kind in types {
            let line = json!({
                "type": kind, "session": "ses_1", "text": "x", "name": "APIError",
                "tool": "bash", "permission": "bash", "question": "q"
            });
            assert!(read(&line).is_some(), "{kind} reports something");
        }
    }
}
