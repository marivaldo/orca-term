//! App: a key the person pressed, as crossterm read it, turned back into the bytes a terminal
//! program reads for it, the way xterm sends them. The application reads keys as events to find
//! its own (the prefix), so the agent's panel gets each other key encoded again.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// The control code Ctrl and `c` send.
pub(crate) fn ctrl_byte(c: char) -> Option<u8> {
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        'A'..='Z' => Some(c as u8 - b'A' + 1),
        ' ' | '@' | '2' => Some(0x00),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '7' | '/' => Some(0x1f),
        '?' | '8' => Some(0x7f),
        _ => None,
    }
}

/// xterm's modifier parameter: 1, plus 1 for shift, 2 for alt and 4 for ctrl.
fn modifier_parameter(mods: KeyModifiers) -> u8 {
    1 + u8::from(mods.contains(KeyModifiers::SHIFT))
        + 2 * u8::from(mods.contains(KeyModifiers::ALT))
        + 4 * u8::from(mods.contains(KeyModifiers::CONTROL))
}

/// The bytes of `key`. `application_cursor` is the program's cursor keys mode (DECCKM), which
/// changes what the arrows, Home and End send. A key with no encoding gives nothing.
pub(crate) fn encode(key: KeyEvent, application_cursor: bool) -> Vec<u8> {
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let mut out = Vec::new();
    if alt
        && matches!(
            key.code,
            KeyCode::Char(_) | KeyCode::Enter | KeyCode::Tab | KeyCode::Backspace | KeyCode::Esc
        )
    {
        out.push(0x1b);
    }
    match key.code {
        KeyCode::Char(c) => match ctrl.then(|| ctrl_byte(c)).flatten() {
            Some(byte) => out.push(byte),
            None => out.extend(c.encode_utf8(&mut [0; 4]).bytes()),
        },
        KeyCode::Enter => out.push(b'\r'),
        KeyCode::Tab if key.modifiers.contains(KeyModifiers::SHIFT) => out.extend(b"\x1b[Z"),
        KeyCode::BackTab => out.extend(b"\x1b[Z"),
        KeyCode::Tab => out.push(b'\t'),
        KeyCode::Backspace => out.push(if ctrl { 0x08 } else { 0x7f }),
        KeyCode::Esc => out.push(0x1b),
        code => out.extend(special(
            code,
            modifier_parameter(key.modifiers),
            application_cursor,
        )),
    }
    out
}

/// The `ESC` sequences of the arrows, the page keys and the function keys, with modifiers `m`.
fn special(code: KeyCode, m: u8, application_cursor: bool) -> Vec<u8> {
    // `ESC [ <letter>` keys (or `ESC O <letter>` in application mode), `ESC [ 1 ; m <letter>`
    // with modifiers.
    let letter = |letter: u8, ss3: bool| -> Vec<u8> {
        if m == 1 {
            vec![0x1b, if ss3 { b'O' } else { b'[' }, letter]
        } else {
            let mut out = format!("\x1b[1;{m}").into_bytes();
            out.push(letter);
            out
        }
    };
    // `ESC [ <number> ~` keys, `ESC [ <number> ; m ~` with modifiers.
    let tilde = |number: u8| -> Vec<u8> {
        if m == 1 {
            format!("\x1b[{number}~").into_bytes()
        } else {
            format!("\x1b[{number};{m}~").into_bytes()
        }
    };
    match code {
        KeyCode::Up => letter(b'A', application_cursor),
        KeyCode::Down => letter(b'B', application_cursor),
        KeyCode::Right => letter(b'C', application_cursor),
        KeyCode::Left => letter(b'D', application_cursor),
        KeyCode::Home => letter(b'H', application_cursor),
        KeyCode::End => letter(b'F', application_cursor),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => letter(b'P' + n - 1, true),
        KeyCode::F(n @ 5..=12) => tilde([15, 17, 18, 19, 20, 21, 23, 24][usize::from(n - 5)]),
        _ => Vec::new(),
    }
}

/// Pasted text as the program wants it: inside the bracketed-paste marks when it switched them
/// on, else with its line ends as Enter.
pub(crate) fn paste(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.replace("\r\n", "\r").replace('\n', "\r").into_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn letters_controls_and_alt_are_sent_as_xterm_sends_them() {
        let none = KeyModifiers::NONE;
        assert_eq!(encode(key(KeyCode::Char('x'), none), false), b"x");
        assert_eq!(encode(key(KeyCode::Char('é'), none), false), "é".as_bytes());
        assert_eq!(
            encode(key(KeyCode::Char('a'), KeyModifiers::CONTROL), false),
            [0x01]
        );
        assert_eq!(
            encode(key(KeyCode::Char('x'), KeyModifiers::ALT), false),
            b"\x1bx"
        );
        assert_eq!(encode(key(KeyCode::Enter, none), false), b"\r");
        assert_eq!(encode(key(KeyCode::Backspace, none), false), [0x7f]);
        assert_eq!(encode(key(KeyCode::Esc, none), false), [0x1b]);
        assert_eq!(
            encode(key(KeyCode::BackTab, KeyModifiers::SHIFT), false),
            b"\x1b[Z"
        );
    }

    #[test]
    fn arrows_follow_the_cursor_keys_mode_and_carry_modifiers() {
        let none = KeyModifiers::NONE;
        assert_eq!(encode(key(KeyCode::Up, none), false), b"\x1b[A");
        assert_eq!(encode(key(KeyCode::Up, none), true), b"\x1bOA");
        assert_eq!(
            encode(key(KeyCode::Left, KeyModifiers::CONTROL), true),
            b"\x1b[1;5D"
        );
        assert_eq!(encode(key(KeyCode::PageDown, none), false), b"\x1b[6~");
        assert_eq!(
            encode(key(KeyCode::Delete, KeyModifiers::SHIFT), false),
            b"\x1b[3;2~"
        );
        assert_eq!(encode(key(KeyCode::F(1), none), false), b"\x1bOP");
        assert_eq!(encode(key(KeyCode::F(5), none), false), b"\x1b[15~");
    }

    #[test]
    fn a_paste_is_bracketed_only_when_asked() {
        assert_eq!(paste("a\nb", true), b"\x1b[200~a\nb\x1b[201~");
        assert_eq!(paste("a\nb\r\nc", false), b"a\rb\rc");
    }
}
