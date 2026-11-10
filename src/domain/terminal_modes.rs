//! Domain: the terminal modes an agent switched on (the alternate screen, the mouse, bracketed
//! paste, the keyboard protocol...), read from what it printed (#174).
//!
//! A holder replays the last bytes the agent printed to every client that attaches. Most agents
//! switch their modes once, at startup, so once the scrollback has dropped those first bytes a
//! client attaching late would draw the screen on the main screen and type in the wrong encoding.
//! The holder therefore reads the bytes the scrollback drops with [`ModeReader`], and puts the
//! modes they left switched on back in front of the replay ([`Modes::restoring`]).

use std::collections::BTreeMap;

/// The DEC private modes a client must have to draw the agent's screen and type into it:
/// application cursor keys (1), the cursor shown (25), the mouse reports and their encodings
/// (1000, 1002, 1003, 1005, 1006, 1015, 1016), focus reports (1004) and bracketed paste (2004).
const TRACKED: &[u16] = &[1, 25, 1000, 1002, 1003, 1004, 1005, 1006, 1015, 1016, 2004];

/// The DEC private modes that switch to the alternate screen, all restored as 1049.
const ALTERNATE: &[u16] = &[47, 1047, 1049];

/// The most keyboard flags kept on the kitty keyboard protocol's stack; older ones are dropped,
/// so an agent that only pushes cannot grow it without end.
const KITTY_STACK_CAP: usize = 16;

/// The modes a terminal is in, as far as they matter to a client of the holder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Modes {
    /// Whether the alternate screen is on.
    alternate: bool,
    /// Each tracked DEC private mode the output set or reset, and how it left it.
    private: BTreeMap<u16, bool>,
    /// Whether the keypad sends application sequences (`ESC =`), not numbers (`ESC >`).
    keypad: bool,
    /// The kitty keyboard protocol's stack of flags, oldest first.
    kitty: Vec<u16>,
    /// xterm's modifyOtherKeys level, when the output set one.
    modify_other_keys: Option<u16>,
}

impl Modes {
    /// The bytes that put a terminal fresh from a reset into these modes: what a holder writes in
    /// front of a replay. Empty when every mode is at its default.
    pub(crate) fn restoring(&self) -> Vec<u8> {
        let mut out: Vec<String> = Vec::new();
        if self.alternate {
            out.push("\x1b[?1049h".to_owned());
        }
        for (&mode, &on) in &self.private {
            if on != default_of(mode) {
                out.push(format!("\x1b[?{mode}{}", if on { 'h' } else { 'l' }));
            }
        }
        if self.keypad {
            out.push("\x1b=".to_owned());
        }
        out.extend(self.kitty.iter().map(|flags| format!("\x1b[>{flags}u")));
        if let Some(level) = self.modify_other_keys {
            out.push(format!("\x1b[>4;{level}m"));
        }
        out.concat().into_bytes()
    }

    /// A DEC private mode set (`CSI ? <modes> h`) or reset (`... l`).
    fn set_private(&mut self, modes: impl Iterator<Item = u16>, on: bool) {
        for mode in modes {
            if ALTERNATE.contains(&mode) {
                self.alternate = on;
            } else if TRACKED.contains(&mode) {
                self.private.insert(mode, on);
            }
        }
    }

    /// The kitty keyboard protocol: push (`CSI > flags u`), pop (`CSI < count u`) and set the
    /// flags on top (`CSI = flags ; how u`: 1 replaces them, 2 adds, 3 takes away).
    fn kitty_keyboard(&mut self, marker: u8, first: Option<u16>, second: Option<u16>) {
        match marker {
            b'>' => {
                self.kitty.push(first.unwrap_or(0));
                if self.kitty.len() > KITTY_STACK_CAP {
                    self.kitty.remove(0);
                }
            }
            b'<' => {
                let count = usize::from(first.unwrap_or(1).max(1));
                self.kitty.truncate(self.kitty.len().saturating_sub(count));
            }
            b'=' => {
                let flags = first.unwrap_or(0);
                if self.kitty.is_empty() {
                    self.kitty.push(0);
                }
                if let Some(top) = self.kitty.last_mut() {
                    *top = match second.unwrap_or(1) {
                        2 => *top | flags,
                        3 => *top & !flags,
                        _ => flags,
                    };
                }
            }
            _ => {}
        }
    }
}

/// A tracked mode's value after a reset: only the cursor is shown.
fn default_of(mode: u16) -> bool {
    mode == 25
}

/// Reads output as it comes, in chunks of any size, and keeps the modes it switches.
#[derive(Default)]
pub(crate) struct ModeReader {
    parser: vte::Parser,
    modes: Modes,
}

impl std::fmt::Debug for ModeReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModeReader")
            .field("modes", &self.modes)
            .finish_non_exhaustive()
    }
}

impl ModeReader {
    /// Reads `bytes`, the next output of the terminal.
    pub(crate) fn read(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.modes, bytes);
    }

    /// The modes the output read so far left the terminal in.
    pub(crate) fn modes(&self) -> &Modes {
        &self.modes
    }
}

/// What each sequence does to the modes; everything else the parser reads changes nothing here.
impl vte::Perform for Modes {
    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        if ignore {
            return;
        }
        let mut numbers = params
            .iter()
            .map(|param| param.first().copied().unwrap_or(0));
        match (intermediates, action) {
            ([b'?'], 'h') => self.set_private(numbers, true),
            ([b'?'], 'l') => self.set_private(numbers, false),
            ([marker @ (b'>' | b'<' | b'=')], 'u') => {
                let (first, second) = (numbers.next(), numbers.next());
                self.kitty_keyboard(*marker, first, second);
            }
            // xterm's modifyOtherKeys: `CSI > 4 ; level m`, and `CSI > 4 m` to reset it.
            ([b'>'], 'm') if numbers.next() == Some(4) => {
                self.modify_other_keys = numbers.next().filter(|level| *level > 0);
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        if ignore || !intermediates.is_empty() {
            return;
        }
        match byte {
            b'=' => self.keypad = true,
            b'>' => self.keypad = false,
            // RIS, the full reset.
            b'c' => *self = Self::default(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn after(output: &[u8]) -> Vec<u8> {
        let mut reader = ModeReader::default();
        reader.read(output);
        reader.modes().restoring()
    }

    #[test]
    fn a_terminal_left_at_its_defaults_needs_nothing() {
        assert_eq!(after(b""), b"");
        assert_eq!(after(b"hello\r\n\x1b[31mred\x1b[0m"), b"");
        assert_eq!(
            after(b"\x1b[?2004h\x1b[?2004l"),
            b"",
            "switched on, then off"
        );
        assert_eq!(
            after(b"\x1b[?25l\x1b[?25h"),
            b"",
            "the cursor shown is the default"
        );
    }

    #[test]
    fn the_alternate_screen_comes_first_then_the_other_modes() {
        assert_eq!(
            after(b"\x1b[?1049h\x1b[?1000h\x1b[?1006h\x1b[?2004h\x1b[?25l"),
            b"\x1b[?1049h\x1b[?25l\x1b[?1000h\x1b[?1006h\x1b[?2004h"
        );
    }

    #[test]
    fn several_modes_in_one_sequence_and_the_older_alternate_screens_count() {
        assert_eq!(
            after(b"\x1b[?1;2004h\x1b[?47h"),
            b"\x1b[?1049h\x1b[?1h\x1b[?2004h"
        );
        assert_eq!(after(b"\x1b[?1047h\x1b[?1047l"), b"");
    }

    #[test]
    fn modes_split_across_reads_are_still_read() {
        let mut reader = ModeReader::default();
        for byte in b"\x1b[?2004h" {
            reader.read(&[*byte]);
        }
        assert_eq!(reader.modes().restoring(), b"\x1b[?2004h");
    }

    #[test]
    fn the_kitty_keyboard_stack_is_pushed_popped_and_set() {
        assert_eq!(after(b"\x1b[>5u"), b"\x1b[>5u");
        assert_eq!(after(b"\x1b[>1u\x1b[>5u\x1b[<u"), b"\x1b[>1u");
        assert_eq!(after(b"\x1b[>1u\x1b[>5u\x1b[<2u"), b"");
        assert_eq!(after(b"\x1b[<u"), b"", "popping an empty stack");
        assert_eq!(after(b"\x1b[>1u\x1b[=4;2u"), b"\x1b[>5u", "flags or-ed in");
        assert_eq!(after(b"\x1b[>5u\x1b[=4;3u"), b"\x1b[>1u", "flags taken out");
        assert_eq!(after(b"\x1b[>5u\x1b[=2u"), b"\x1b[>2u", "flags replaced");
        let pushes = b"\x1b[>1u".repeat(40);
        assert_eq!(
            after(&pushes),
            b"\x1b[>1u".repeat(16),
            "the stack is bounded"
        );
    }

    #[test]
    fn modify_other_keys_and_the_keypad_are_kept() {
        assert_eq!(after(b"\x1b[>4;1m"), b"\x1b[>4;1m");
        assert_eq!(after(b"\x1b[>4;1m\x1b[>4m"), b"");
        assert_eq!(after(b"\x1b[>4;2m\x1b[>4;0m"), b"");
        assert_eq!(after(b"\x1b="), b"\x1b=");
        assert_eq!(after(b"\x1b=\x1b>"), b"");
    }

    #[test]
    fn a_full_reset_forgets_every_mode() {
        assert_eq!(after(b"\x1b[?1049h\x1b[?2004h\x1b[>5u\x1b=\x1bc"), b"");
    }

    #[test]
    fn colours_and_other_private_modes_are_not_kept() {
        assert_eq!(after(b"\x1b[?2026h\x1b[?7l\x1b[38;2;1;2;3m"), b"");
    }
}
