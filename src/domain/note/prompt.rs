//! Domain: the prompt `note send` pastes into the worktree's agent, and the bytes that paste it.
//! Both copy Orca's exactly (stablyai/orca at 59b9bde): the prompt is
//! `src/shared/diff-comments-format.ts`, and the paste is `sendPromptWithGuardedPasteAndEnter` in
//! `src/renderer/src/lib/active-agent-note-send-delivery.ts`.
//!
//! The prompt has one block per note, separated by a blank line, with no preamble, no code
//! snippet and no newline at the end:
//!
//! ```text
//! File: src/app.rs
//! Line: 10                      (or `Lines: 7-10`, or `Scope: file`)
//! User comment: "Needs validation"
//! ```
//!
//! The body is quoted, with `\`, `"`, carriage returns and newlines escaped (in that order), so it
//! can never break out of its three lines. The anchor is the one translated to the file now.

use std::time::Duration;

use crate::domain::note::{Anchor, NotePath};

/// Starts a bracketed paste: what a terminal sends before pasted text.
const PASTE_START: &[u8] = b"\x1b[200~";

/// Ends a bracketed paste.
const PASTE_END: &[u8] = b"\x1b[201~";

/// What Orca puts in place of an escape character inside a paste (U+241B, `␛`), so pasted text can
/// never end the paste early and run the rest as keystrokes.
const ESCAPE_STAND_IN: &str = "\u{241b}";

/// The Enter that submits the pasted prompt, sent on its own after the paste.
pub(crate) const SUBMIT: &[u8] = b"\r";

/// How long to wait between the paste and the Enter: claude can leave a prompt as editable text
/// when the paste's end and Enter arrive in the same write. Orca's
/// `AGENT_PROMPT_POST_PASTE_SUBMIT_DELAY_MS`.
pub(crate) const PASTE_SUBMIT_PAUSE: Duration = Duration::from_millis(50);

/// One note to send: its path, its anchor in the file now, and its body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Block<'a> {
    pub(crate) path: &'a NotePath,
    pub(crate) anchor: Anchor,
    pub(crate) body: &'a str,
}

/// The prompt for `blocks`, in order: Orca's `formatDiffComments`.
pub(crate) fn format(blocks: &[Block<'_>]) -> String {
    blocks
        .iter()
        .map(format_block)
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// One note's three lines: Orca's `formatDiffComment`.
fn format_block(block: &Block<'_>) -> String {
    let location = match block.anchor {
        Anchor::Line(line) => format!("Line: {line}"),
        Anchor::Lines(range) => format!("Lines: {}-{}", range.start(), range.end()),
        Anchor::File => "Scope: file".to_owned(),
    };
    [
        format!("File: {}", block.path),
        location,
        format!("User comment: \"{}\"", escape(block.body)),
    ]
    .join("\n")
}

/// The body with `\`, `"`, `\r` and `\n` escaped, in Orca's order.
fn escape(body: &str) -> String {
    body.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\r', "\\r")
        .replace('\n', "\\n")
}

/// The bytes that paste `prompt` into a terminal: the prompt trimmed, every escape character
/// replaced by `␛`, between the bracketed paste's start and end. The Enter follows on its own.
pub(crate) fn paste(prompt: &str) -> Vec<u8> {
    let text = prompt.trim().replace('\x1b', ESCAPE_STAND_IN);
    let mut bytes = PASTE_START.to_vec();
    bytes.extend_from_slice(text.as_bytes());
    bytes.extend_from_slice(PASTE_END);
    bytes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::note::LineRange;

    fn path() -> NotePath {
        NotePath::parse("src/app.ts").unwrap()
    }

    fn block<'a>(path: &'a NotePath, anchor: Anchor, body: &'a str) -> Block<'a> {
        Block { path, anchor, body }
    }

    // The cases of Orca's own tests (`diff-comments-format.test.ts`), with their expected text.

    #[test]
    fn a_note_is_the_fixed_three_line_structure() {
        let path = path();
        assert_eq!(
            format(&[block(&path, Anchor::Line(10), "Needs validation")]),
            "File: src/app.ts\nLine: 10\nUser comment: \"Needs validation\""
        );
    }

    #[test]
    fn a_range_and_a_file_note_name_their_scope() {
        let path = path();
        let range = Anchor::Lines(LineRange::new(7, 10).unwrap());
        assert_eq!(
            format(&[block(&path, range, "Needs validation")]),
            "File: src/app.ts\nLines: 7-10\nUser comment: \"Needs validation\""
        );
        assert_eq!(
            format(&[block(&path, Anchor::File, "Needs validation")]),
            "File: src/app.ts\nScope: file\nUser comment: \"Needs validation\""
        );
    }

    #[test]
    fn quotes_backslashes_and_line_breaks_are_escaped() {
        let path = path();
        let body = |body| format(&[block(&path, Anchor::Line(1), body)]);
        assert!(body("why \"this\" path?").ends_with("User comment: \"why \\\"this\\\" path?\""));
        assert!(
            body("path\\to\\\"thing\"").ends_with("User comment: \"path\\\\to\\\\\\\"thing\\\"\"")
        );
        let lines = body("first\nsecond\r\nthird");
        assert!(lines.ends_with("User comment: \"first\\nsecond\\r\\nthird\""));
        assert_eq!(lines.split('\n').count(), 3);
    }

    #[test]
    fn notes_are_joined_by_a_blank_line_with_nothing_around() {
        let path = path();
        assert_eq!(
            format(&[
                block(&path, Anchor::Line(1), "first"),
                block(&path, Anchor::Line(2), "second"),
            ]),
            "File: src/app.ts\nLine: 1\nUser comment: \"first\"\n\n\
             File: src/app.ts\nLine: 2\nUser comment: \"second\""
        );
        assert_eq!(format(&[]), "");
    }

    #[test]
    fn the_paste_is_bracketed_and_cannot_be_ended_by_its_text() {
        assert_eq!(
            paste("File: a\nLine: 1"),
            b"\x1b[200~File: a\nLine: 1\x1b[201~"
        );
        assert_eq!(
            paste("say \x1b[201~ rm"),
            "\x1b[200~say \u{241b}[201~ rm\x1b[201~".as_bytes()
        );
        assert_eq!(paste("  padded \n"), b"\x1b[200~padded\x1b[201~");
        assert_eq!(SUBMIT, b"\r");
        assert_eq!(PASTE_SUBMIT_PAUSE, Duration::from_millis(50));
    }
}
