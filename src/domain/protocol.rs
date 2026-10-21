//! Domain: what travels between a holder and its clients over `agent.sock`, and the scrollback a
//! holder keeps for replay (ADR 0009), and which clients a holder serves (#61).
//!
//! One frame is a tag byte, a 4-byte big-endian payload length and the payload, in both
//! directions. A client sends input (`i`), its terminal size (`r`) and a stop (`s`); the holder
//! sends output (`o`, the first one after a connection being the whole replay) and the agent's end
//! (`x`). Detaching needs no frame: the client closes. The adapters only move bytes; the encoding
//! and the incremental decoding live here.

use std::collections::VecDeque;

use crate::domain::agent::Size;

/// The most bytes a frame may carry: more than the replay can ever be.
pub(crate) const MAX_PAYLOAD: usize = 16 * 1024 * 1024;

/// How much of the agent's output a holder keeps for replay.
pub(crate) const SCROLLBACK_CAP: usize = 8 * 1024 * 1024;

const TAG_INPUT: u8 = b'i';
const TAG_RESIZE: u8 = b'r';
const TAG_STOP: u8 = b's';
const TAG_OUTPUT: u8 = b'o';
const TAG_EXIT: u8 = b'x';

/// One message of the protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Frame {
    /// Client to holder: bytes for the agent's terminal.
    Input(Vec<u8>),
    /// Client to holder: the client's terminal size.
    Resize(Size),
    /// Client to holder: end the agent.
    Stop,
    /// Holder to client: bytes the agent's terminal printed.
    Output(Vec<u8>),
    /// Holder to client: the agent ended, as the text says (`exit 7`, `signal 9`); the holder
    /// closes the connection next.
    Exit(String),
}

impl Frame {
    /// The frame's bytes on the wire.
    pub(crate) fn encode(&self) -> Vec<u8> {
        let (tag, payload): (u8, Vec<u8>) = match self {
            Self::Input(bytes) => (TAG_INPUT, bytes.clone()),
            Self::Resize(size) => {
                let mut payload = size.rows.to_be_bytes().to_vec();
                payload.extend_from_slice(&size.cols.to_be_bytes());
                (TAG_RESIZE, payload)
            }
            Self::Stop => (TAG_STOP, Vec::new()),
            Self::Output(bytes) => (TAG_OUTPUT, bytes.clone()),
            Self::Exit(text) => (TAG_EXIT, text.as_bytes().to_vec()),
        };
        let mut frame = Vec::with_capacity(5 + payload.len());
        frame.push(tag);
        frame.extend_from_slice(
            &u32::try_from(payload.len())
                .unwrap_or(u32::MAX)
                .to_be_bytes(),
        );
        frame.extend_from_slice(&payload);
        frame
    }
}

/// Why a byte stream is not the protocol.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum DecodeError {
    #[error("a frame of {0} bytes is larger than the protocol allows")]
    TooLarge(usize),
    #[error("unknown frame tag {0:#04x}")]
    UnknownTag(u8),
    #[error("a resize frame must carry exactly 4 bytes, not {0}")]
    BadResize(usize),
}

/// Turns the bytes read from a connection, in whatever chunks they arrive, back into frames.
#[derive(Debug, Default)]
pub(crate) struct Decoder {
    pending: Vec<u8>,
}

impl Decoder {
    /// Adds bytes read from the connection.
    pub(crate) fn push(&mut self, bytes: &[u8]) {
        self.pending.extend_from_slice(bytes);
    }

    /// The next whole frame, or `None` until more bytes arrive.
    pub(crate) fn next_frame(&mut self) -> Result<Option<Frame>, DecodeError> {
        if self.pending.len() < 5 {
            return Ok(None);
        }
        let header: [u8; 4] = [
            self.pending[1],
            self.pending[2],
            self.pending[3],
            self.pending[4],
        ];
        let len = u32::from_be_bytes(header) as usize;
        if len > MAX_PAYLOAD {
            return Err(DecodeError::TooLarge(len));
        }
        if self.pending.len() < 5 + len {
            return Ok(None);
        }
        let tag = self.pending[0];
        let payload: Vec<u8> = self.pending.drain(..5 + len).skip(5).collect();
        decode(tag, payload).map(Some)
    }
}

fn decode(tag: u8, payload: Vec<u8>) -> Result<Frame, DecodeError> {
    Ok(match tag {
        TAG_INPUT => Frame::Input(payload),
        TAG_RESIZE => match payload.as_slice() {
            [r0, r1, c0, c1] => Frame::Resize(Size {
                rows: u16::from_be_bytes([*r0, *r1]),
                cols: u16::from_be_bytes([*c0, *c1]),
            }),
            other => return Err(DecodeError::BadResize(other.len())),
        },
        TAG_STOP => Frame::Stop,
        TAG_OUTPUT => Frame::Output(payload),
        TAG_EXIT => Frame::Exit(String::from_utf8_lossy(&payload).into_owned()),
        other => return Err(DecodeError::UnknownTag(other)),
    })
}

/// How an ended agent is described in an exit frame: `exit <code>` or `signal <n>`.
pub(crate) fn exit_text(code: Option<i32>, signal: Option<i32>) -> String {
    match (code, signal) {
        (Some(code), _) => format!("exit {code}"),
        (None, Some(signal)) => format!("signal {signal}"),
        (None, None) => "ended".to_owned(),
    }
}

/// The kinds of namespace the holder compares: the network one and the user one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(any(target_os = "linux", test)),
    expect(
        dead_code,
        reason = "only the holder on Linux reads its clients' namespaces"
    )
)]
pub(crate) enum NsKind {
    Net,
    User,
}

#[cfg_attr(
    not(any(target_os = "linux", test)),
    expect(
        dead_code,
        reason = "only the holder on Linux reads its clients' namespaces"
    )
)]
impl NsKind {
    /// The name of the kind, as in `/proc/<pid>/ns/<name>` and in the link's text: `net`, `user`.
    pub(crate) const fn name(self) -> &'static str {
        match self {
            Self::Net => "net",
            Self::User => "user",
        }
    }
}

/// A namespace, as the kernel names it in `/proc/<pid>/ns/<kind>`: `net:[<number>]`,
/// `user:[<number>]`. Two processes share a namespace exactly when their numbers are equal. On
/// Linux the holder compares its own with each client's: commands in an agent's sandbox run in
/// namespaces of their own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(any(target_os = "linux", test)),
    expect(
        dead_code,
        reason = "only the holder on Linux reads its clients' namespaces"
    )
)]
pub(crate) struct NsId(u64);

#[cfg_attr(
    not(any(target_os = "linux", test)),
    expect(
        dead_code,
        reason = "only the holder on Linux reads its clients' namespaces"
    )
)]
impl NsId {
    /// The namespace of `kind` a `/proc/<pid>/ns/<kind>` link names, or `None` when `link` is not
    /// of the form `<kind>:[<number>]`.
    pub(crate) fn parse(kind: NsKind, link: &str) -> Option<Self> {
        let number = link
            .strip_prefix(kind.name())?
            .strip_prefix(":[")?
            .strip_suffix(']')?;
        number.parse().ok().map(Self)
    }
}

/// The namespaces of a process that matter to a holder, each `None` when it cannot be read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(any(target_os = "linux", test)),
    expect(
        dead_code,
        reason = "only the holder on Linux reads its clients' namespaces"
    )
)]
pub(crate) struct Namespaces {
    pub(crate) net: Option<NsId>,
    pub(crate) user: Option<NsId>,
}

/// Whether the holder serves a client: only when its own network and user namespaces and the
/// client's could all be read, and the client shares both. claude's sandbox runs commands in a
/// network namespace of their own; opencode's keeps the network, so a command it runs shares the
/// holder's network namespace, but it runs in a user namespace of its own (#35). A client whose
/// namespaces cannot be read is refused.
#[cfg_attr(
    not(any(target_os = "linux", test)),
    expect(
        dead_code,
        reason = "only the holder on Linux checks its clients' namespaces"
    )
)]
pub(crate) fn admit(own: Namespaces, peer: Namespaces) -> bool {
    same(own.net, peer.net) && same(own.user, peer.user)
}

/// Whether two namespaces were both read and are the same one.
#[cfg_attr(
    not(any(target_os = "linux", test)),
    expect(
        dead_code,
        reason = "only the holder on Linux checks its clients' namespaces"
    )
)]
fn same(own: Option<NsId>, peer: Option<NsId>) -> bool {
    match (own, peer) {
        (Some(own), Some(peer)) => own == peer,
        _ => false,
    }
}

/// The last bytes the agent printed, at most [`SCROLLBACK_CAP`] of them, replayed to every
/// client that attaches.
#[derive(Debug)]
pub(crate) struct Scrollback {
    bytes: VecDeque<u8>,
    cap: usize,
}

impl Scrollback {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            bytes: VecDeque::new(),
            cap,
        }
    }

    /// Keeps `chunk`, dropping the oldest bytes beyond the cap.
    pub(crate) fn push(&mut self, chunk: &[u8]) {
        self.bytes.extend(chunk);
        let over = self.bytes.len().saturating_sub(self.cap);
        self.bytes.drain(..over);
    }

    /// Everything kept, oldest first.
    pub(crate) fn snapshot(&self) -> Vec<u8> {
        let (a, b) = self.bytes.as_slices();
        let mut all = Vec::with_capacity(a.len() + b.len());
        all.extend_from_slice(a);
        all.extend_from_slice(b);
        all
    }

    /// The last line kept that is not blank, with its escape sequences and control bytes dropped,
    /// for a report.
    pub(crate) fn last_line(&self) -> Option<String> {
        let text = String::from_utf8_lossy(&self.snapshot()).into_owned();
        text.lines()
            .map(|line| strip_escapes(line).trim().to_owned())
            .rfind(|line| !line.is_empty())
    }
}

/// `line` without its `ESC [ ... <letter>` sequences (colours, cursor moves) and control bytes.
fn strip_escapes(line: &str) -> String {
    let mut plain = String::new();
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next();
            skip_csi(&mut chars);
        } else if !c.is_control() {
            plain.push(c);
        }
    }
    plain
}

/// Skips a CSI sequence, which ends at its first byte in `@`..=`~`.
fn skip_csi(chars: &mut impl Iterator<Item = char>) {
    for c in chars {
        if ('@'..='~').contains(&c) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_frame_survives_the_codec() {
        let frames = [
            Frame::Input(b"ls\n".to_vec()),
            Frame::Resize(Size {
                rows: 50,
                cols: 132,
            }),
            Frame::Stop,
            Frame::Output(b"$ ".to_vec()),
            Frame::Exit("exit 7".to_owned()),
        ];
        let mut decoder = Decoder::default();
        for frame in &frames {
            decoder.push(&frame.encode());
        }
        for frame in &frames {
            assert_eq!(decoder.next_frame().unwrap().as_ref(), Some(frame));
        }
        assert_eq!(decoder.next_frame().unwrap(), None);
    }

    #[test]
    fn a_resize_frame_is_rows_then_cols_big_endian() {
        assert_eq!(
            Frame::Resize(Size {
                rows: 24,
                cols: 300
            })
            .encode(),
            [b'r', 0, 0, 0, 4, 0, 24, 1, 44]
        );
    }

    #[test]
    fn frames_arrive_in_any_chunks() {
        let bytes = Frame::Output(b"hello".to_vec()).encode();
        let mut decoder = Decoder::default();
        for byte in &bytes[..bytes.len() - 1] {
            decoder.push(&[*byte]);
            assert_eq!(decoder.next_frame().unwrap(), None);
        }
        decoder.push(&bytes[bytes.len() - 1..]);
        assert_eq!(
            decoder.next_frame().unwrap(),
            Some(Frame::Output(b"hello".to_vec()))
        );
    }

    #[test]
    fn malformed_frames_are_refused() {
        let mut decoder = Decoder::default();
        decoder.push(&[b'o', 0xff, 0xff, 0xff, 0xff]);
        assert!(matches!(
            decoder.next_frame(),
            Err(DecodeError::TooLarge(0xffff_ffff))
        ));
        let mut decoder = Decoder::default();
        decoder.push(&[b'z', 0, 0, 0, 0]);
        assert_eq!(decoder.next_frame(), Err(DecodeError::UnknownTag(b'z')));
        let mut decoder = Decoder::default();
        decoder.push(&[b'r', 0, 0, 0, 1, 9]);
        assert_eq!(decoder.next_frame(), Err(DecodeError::BadResize(1)));
    }

    #[test]
    fn an_exit_is_described_by_code_or_signal() {
        assert_eq!(exit_text(Some(7), None), "exit 7");
        assert_eq!(exit_text(None, Some(9)), "signal 9");
        assert_eq!(exit_text(None, None), "ended");
    }

    #[test]
    fn a_namespace_is_read_from_its_proc_link() {
        let net = |link| NsId::parse(NsKind::Net, link);
        assert_eq!(net("net:[4026531840]"), Some(NsId(4_026_531_840)));
        assert_eq!(net("net:[]"), None);
        assert_eq!(net("net:[12"), None);
        assert_eq!(net("mnt:[4026531840]"), None);
        assert_eq!(net("user:[4026531837]"), None, "another kind");
        assert_eq!(net("net:[x]"), None);
        assert_eq!(
            NsId::parse(NsKind::User, "user:[4026531837]"),
            Some(NsId(4_026_531_837))
        );
        assert_eq!(NsId::parse(NsKind::User, "net:[4026531840]"), None);
    }

    fn namespaces(net: &str, user: &str) -> Namespaces {
        Namespaces {
            net: NsId::parse(NsKind::Net, net),
            user: NsId::parse(NsKind::User, user),
        }
    }

    #[test]
    fn a_client_is_served_only_from_the_holders_own_network_namespace() {
        let own = namespaces("net:[4026531840]", "user:[4026531837]");
        let claudes_sandbox = namespaces("net:[4026532999]", "user:[4026532998]");
        assert!(admit(own, own));
        assert!(!admit(own, claudes_sandbox));
        let unknown_net = Namespaces { net: None, ..own };
        assert!(
            !admit(own, unknown_net),
            "a client whose namespace cannot be read is refused"
        );
        assert!(
            !admit(unknown_net, own),
            "a holder that cannot read its own refuses everyone"
        );
        assert!(!admit(unknown_net, unknown_net));
    }

    #[test]
    fn a_client_is_served_only_from_the_holders_own_user_namespace() {
        let own = namespaces("net:[4026531840]", "user:[4026531837]");
        // opencode's sandbox keeps the network but has a user namespace of its own.
        let opencodes_sandbox = namespaces("net:[4026531840]", "user:[4026533001]");
        assert!(!admit(own, opencodes_sandbox));
        let unknown_user = Namespaces { user: None, ..own };
        assert!(!admit(own, unknown_user));
        assert!(!admit(unknown_user, own));
    }

    #[test]
    fn the_scrollback_keeps_the_newest_bytes_up_to_its_cap() {
        let mut scrollback = Scrollback::new(8);
        scrollback.push(b"abcdef");
        scrollback.push(b"ghij");
        assert_eq!(scrollback.snapshot(), b"cdefghij");
        scrollback.push(b"0123456789");
        assert_eq!(scrollback.snapshot(), b"23456789");
    }

    #[test]
    fn the_last_line_skips_blank_lines_and_control_bytes() {
        let mut scrollback = Scrollback::new(1024);
        scrollback.push(b"starting\r\n\x1b[31mError: bad settings\x1b[0m\x07\r\n\r\n");
        assert_eq!(
            scrollback.last_line(),
            Some("Error: bad settings".to_owned())
        );
        assert_eq!(Scrollback::new(8).last_line(), None);
    }
}
