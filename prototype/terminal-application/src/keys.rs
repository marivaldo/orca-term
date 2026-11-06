//! Turns what crossterm read (a key, a mouse event) into the bytes a terminal program expects.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEventKind};

/// The control code Ctrl plus `c` sends, the way xterm maps it.
pub fn ctrl_byte(c: char) -> Option<u8> {
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

/// xterm's modifier parameter: 1 plus shift (1), alt (2) and ctrl (4).
fn modifier_parameter(mods: KeyModifiers) -> u8 {
    1 + u8::from(mods.contains(KeyModifiers::SHIFT))
        + 2 * u8::from(mods.contains(KeyModifiers::ALT))
        + 4 * u8::from(mods.contains(KeyModifiers::CONTROL))
}

/// A key as the bytes the child reads. `application_cursor` is the child's DECCKM mode, which
/// changes what the arrows, Home and End send.
pub fn encode_key(key: KeyEvent, application_cursor: bool) -> Vec<u8> {
    let mods = key.modifiers;
    let alt = mods.contains(KeyModifiers::ALT);
    let ctrl = mods.contains(KeyModifiers::CONTROL);
    let shift = mods.contains(KeyModifiers::SHIFT);
    let m = modifier_parameter(mods);
    let mut out = Vec::new();

    // `ESC [ <letter>` keys: the arrows, Home, End and F1 to F4.
    let letter = |out: &mut Vec<u8>, letter: u8, ss3: bool| {
        if m == 1 {
            out.extend([0x1b, if ss3 { b'O' } else { b'[' }, letter]);
        } else {
            out.extend(format!("\x1b[1;{m}").bytes());
            out.push(letter);
        }
    };
    // `ESC [ <number> ~` keys: Insert, Delete, the page keys and F5 to F12.
    let tilde = |out: &mut Vec<u8>, number: u8| {
        if m == 1 {
            out.extend(format!("\x1b[{number}~").bytes());
        } else {
            out.extend(format!("\x1b[{number};{m}~").bytes());
        }
    };

    match key.code {
        KeyCode::Char(c) => {
            if alt {
                out.push(0x1b);
            }
            match ctrl.then(|| ctrl_byte(c)).flatten() {
                Some(byte) => out.push(byte),
                None => out.extend(c.encode_utf8(&mut [0; 4]).bytes()),
            }
        }
        KeyCode::Enter => {
            if alt {
                out.push(0x1b);
            }
            out.push(b'\r');
        }
        KeyCode::Tab if shift => out.extend(b"\x1b[Z"),
        KeyCode::BackTab => out.extend(b"\x1b[Z"),
        KeyCode::Tab => {
            if alt {
                out.push(0x1b);
            }
            out.push(b'\t');
        }
        KeyCode::Backspace => {
            if alt {
                out.push(0x1b);
            }
            out.push(if ctrl { 0x08 } else { 0x7f });
        }
        KeyCode::Esc => {
            if alt {
                out.push(0x1b);
            }
            out.push(0x1b);
        }
        KeyCode::Up => letter(&mut out, b'A', application_cursor),
        KeyCode::Down => letter(&mut out, b'B', application_cursor),
        KeyCode::Right => letter(&mut out, b'C', application_cursor),
        KeyCode::Left => letter(&mut out, b'D', application_cursor),
        KeyCode::Home => letter(&mut out, b'H', application_cursor),
        KeyCode::End => letter(&mut out, b'F', application_cursor),
        KeyCode::Insert => tilde(&mut out, 2),
        KeyCode::Delete => tilde(&mut out, 3),
        KeyCode::PageUp => tilde(&mut out, 5),
        KeyCode::PageDown => tilde(&mut out, 6),
        KeyCode::F(n @ 1..=4) => letter(&mut out, b'P' + n - 1, true),
        KeyCode::F(n @ 5..=12) => {
            let numbers = [15, 17, 18, 19, 20, 21, 23, 24];
            tilde(&mut out, numbers[usize::from(n) - 5]);
        }
        _ => {}
    }
    out
}

/// A mouse event at a cell of the child's screen (0-based), as the child's mouse protocol reads
/// it: SGR (`ESC [ < b ; x ; y M`) or the old single-byte form.
pub fn encode_mouse(
    kind: MouseEventKind,
    mods: KeyModifiers,
    col: u16,
    row: u16,
    sgr: bool,
) -> Option<Vec<u8>> {
    let button = |b: MouseButton| match b {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let (mut code, release) = match kind {
        MouseEventKind::Down(b) => (button(b), false),
        MouseEventKind::Up(b) => (button(b), true),
        MouseEventKind::Drag(b) => (button(b) + 32, false),
        MouseEventKind::Moved => (35, false),
        MouseEventKind::ScrollUp => (64, false),
        MouseEventKind::ScrollDown => (65, false),
        MouseEventKind::ScrollLeft => (66, false),
        MouseEventKind::ScrollRight => (67, false),
    };
    let held = 4 * u16::from(mods.contains(KeyModifiers::SHIFT))
        + 8 * u16::from(mods.contains(KeyModifiers::ALT))
        + 16 * u16::from(mods.contains(KeyModifiers::CONTROL));
    if sgr {
        code += held;
        let end = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{};{}{end}", col + 1, row + 1).into_bytes());
    }
    if release {
        code = 3;
    }
    code += held;
    let byte = |n: u16| u8::try_from(n + 32).ok();
    Some(vec![
        0x1b,
        b'[',
        b'M',
        byte(code)?,
        byte(col + 1)?,
        byte(row + 1)?,
    ])
}
