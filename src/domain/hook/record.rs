//! Domain: the recording tap of the live checks (#44). When `ORCA_TERM_RECORD` names a file,
//! `orca-term hook` appends to it every payload it receives for a worktree, as received, before
//! handling it: one line per hook, `<event> <payload>` for claude (the format of
//! `tests/fixtures/claude-hooks/`) and the plugin's line itself for opencode, whose lines the
//! holder hands to `orca-term hook` one by one. `just check-live` sets the variable for the agents
//! it starts, so it can read back what the real agents sent. It is off unless the variable is set
//! to something, and nothing orca-term launches ever sets it.

use std::ffi::OsStr;
use std::path::PathBuf;

use serde_json::Value;

use crate::domain::agent::Agent;

/// The variable naming the file the tap appends to.
pub(crate) const RECORD_VAR: &str = "ORCA_TERM_RECORD";

/// The file the tap appends to: the variable's value, or `None` (the tap is off) when it is unset
/// or empty.
pub(crate) fn target(value: Option<&OsStr>) -> Option<PathBuf> {
    value.filter(|value| !value.is_empty()).map(PathBuf::from)
}

/// The line the tap appends for one payload `received` by the hook of `agent`, newline included.
///
/// claude's line is the event, a space, then the payload: the event as the hook's command line
/// names it (`event`), else as the payload does (`hook_event_name`), else `-`. opencode's line is
/// the plugin's line. Either way the payload is kept as received but on one line: a trailing line
/// ending is dropped, and any other line break, which in JSON can only sit between two values,
/// becomes a space.
pub(crate) fn line(agent: Agent, event: Option<&str>, received: &[u8]) -> Vec<u8> {
    let payload = one_line(received);
    let mut line = match agent {
        Agent::ClaudeCode => {
            let event = event
                .map(str::to_owned)
                .or_else(|| event_of(&payload))
                .unwrap_or_else(|| "-".to_owned());
            let mut line = event.into_bytes();
            line.push(b' ');
            line.extend_from_slice(&payload);
            line
        }
        Agent::OpenCode => payload,
    };
    line.push(b'\n');
    line
}

/// `bytes` without its trailing line breaks, each other line break replaced by a space.
fn one_line(bytes: &[u8]) -> Vec<u8> {
    let end = bytes
        .iter()
        .rposition(|&byte| byte != b'\n' && byte != b'\r')
        .map_or(0, |last| last + 1);
    bytes[..end]
        .iter()
        .map(|&byte| {
            if byte == b'\n' || byte == b'\r' {
                b' '
            } else {
                byte
            }
        })
        .collect()
}

/// The event a claude payload names in `hook_event_name`, when it is JSON that names one.
fn event_of(payload: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(payload).ok()?;
    value["hook_event_name"].as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::ffi::OsString;

    use super::*;

    #[test]
    fn the_tap_is_off_unless_the_variable_names_a_file() {
        assert_eq!(target(None), None, "unset");
        assert_eq!(target(Some(&OsString::new())), None, "empty");
        assert_eq!(
            target(Some(OsStr::new("/tmp/hooks.log"))),
            Some(PathBuf::from("/tmp/hooks.log"))
        );
    }

    #[test]
    fn a_claude_payload_is_recorded_after_its_event_on_one_line() {
        let payload =
            br#"{"session_id":"s","hook_event_name":"Stop","last_assistant_message":"ok"}"#;
        let mut received = payload.to_vec();
        received.push(b'\n');
        let mut expected = b"Stop ".to_vec();
        expected.extend_from_slice(payload);
        expected.push(b'\n');
        assert_eq!(line(Agent::ClaudeCode, Some("Stop"), &received), expected);
        assert_eq!(
            line(Agent::ClaudeCode, None, payload),
            expected,
            "the event from the payload when the command line names none"
        );
        assert_eq!(
            line(Agent::ClaudeCode, None, b"not json"),
            b"- not json\n",
            "a payload that names no event is still recorded"
        );
        assert_eq!(
            line(Agent::ClaudeCode, Some("Stop"), b"{\n  \"a\": 1\r\n}\n\n"),
            b"Stop {   \"a\": 1  }\n",
            "line breaks inside the payload become spaces"
        );
    }

    #[test]
    fn an_opencode_line_is_recorded_as_received() {
        let received = b"{\"type\":\"idle\",\"session\":\"ses_1\",\"text\":\"ok\"}\n";
        assert_eq!(line(Agent::OpenCode, None, received), received);
        assert_eq!(
            line(Agent::OpenCode, None, b"{\"type\":\"end\"}"),
            b"{\"type\":\"end\"}\n",
            "the last line, cut by the end of the pipe, gets its newline"
        );
    }
}
