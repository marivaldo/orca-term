//! Domain: the running Neovims of a repository, and what the core tells them. Each Neovim
//! registers its server address (`v:servername`, the path of a unix socket) in the registry,
//! `<git common dir>/orca-term/nvim/`, one file per Neovim; after every write to a worktree's
//! state the core sends each one a msgpack-RPC notification that fires
//! `User OrcaTermWorktreeChanged` there, with the worktree's path, so its sidebar redraws.
//!
//! A registry file is named after its address: the address's 64-bit FNV-1a hash in 16 lowercase
//! hex digits, so the name is short, the same for the same address every time (registering is
//! idempotent) and safe as a file name whatever the address holds. The file holds the address
//! itself, which is what the core connects to.
//!
//! The message is encoded by hand, since it needs only a few msgpack types: arrays, maps, strings
//! and a small positive integer.

use std::fmt;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::domain::state::ADMIN_DIR_NAME;

/// The registry's directory, inside the repository's `orca-term` directory in the git common dir.
const REGISTRY_DIR_NAME: &str = "nvim";

/// The `User` autocommand pattern the notification fires in Neovim.
pub(crate) const EVENT: &str = "OrcaTermWorktreeChanged";

/// The registry of the repository whose git common dir is `common`: one file per running Neovim.
pub(crate) fn registry_dir(common: &Path) -> PathBuf {
    common.join(ADMIN_DIR_NAME).join(REGISTRY_DIR_NAME)
}

/// A Neovim's server address as the core stores it: the absolute path of its unix socket. Only
/// such paths are accepted, so the core only ever connects to a unix socket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Address(String);

/// Why an address is refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub(crate) enum AddressError {
    #[error(
        "invalid Neovim address `{0}`: it must be the absolute path of a unix socket \
         (Neovim's v:servername)"
    )]
    NotAbsolute(String),
    #[error("invalid Neovim address {0:?}: it cannot contain a line break or a NUL byte")]
    BadCharacter(String),
}

impl FromStr for Address {
    type Err = AddressError;

    fn from_str(address: &str) -> Result<Self, Self::Err> {
        if address.contains(['\n', '\r', '\0']) {
            return Err(AddressError::BadCharacter(address.to_owned()));
        }
        if !address.starts_with('/') {
            return Err(AddressError::NotAbsolute(address.to_owned()));
        }
        Ok(Self(address.to_owned()))
    }
}

impl Address {
    /// The socket's path.
    pub(crate) fn as_path(&self) -> &Path {
        Path::new(&self.0)
    }

    /// The name of this address's file in the registry (see the module doc).
    pub(crate) fn entry_name(&self) -> String {
        format!("{:016x}", fnv1a(self.0.as_bytes()))
    }

    /// What this address's registry file holds: the address, nothing else.
    pub(crate) fn entry_bytes(&self) -> Vec<u8> {
        self.0.as_bytes().to_vec()
    }

    /// The address read back from a registry file holding `bytes`, or `None` when they are not
    /// one. Trailing whitespace is forgiven.
    pub(crate) fn from_entry(bytes: &[u8]) -> Option<Self> {
        let text = std::str::from_utf8(bytes).ok()?;
        text.trim_end().parse().ok()
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Whether `name` is the name of a registry file, as [`Address::entry_name`] makes them. Anything
/// else in the directory, such as a half-written temp file, is not an entry.
pub(crate) fn is_entry_name(name: &str) -> bool {
    name.len() == 16
        && name
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The 64-bit FNV-1a hash of `bytes`: tiny, and the same on every machine and Rust version.
fn fnv1a(bytes: &[u8]) -> u64 {
    const OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    bytes.iter().fold(OFFSET_BASIS, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(PRIME)
    })
}

/// The msgpack-RPC notification that tells a Neovim the worktree at `path` changed:
/// `[2, "nvim_exec_autocmds", ["User", {"pattern": EVENT, "data": {"path": path}}]]`. Type 2 is a
/// notification, which Neovim never answers. `path` is the worktree's path as `worktree ls`
/// prints it, so the client can compare the two.
pub(crate) fn worktree_changed(path: &str) -> Vec<u8> {
    let mut out = Vec::new();
    push_array(&mut out, 3);
    push_uint(&mut out, 2);
    push_str(&mut out, "nvim_exec_autocmds");
    // The call's arguments: the event, then its options.
    push_array(&mut out, 2);
    push_str(&mut out, "User");
    push_map(&mut out, 2);
    push_str(&mut out, "pattern");
    push_str(&mut out, EVENT);
    push_str(&mut out, "data");
    push_map(&mut out, 1);
    push_str(&mut out, "path");
    push_str(&mut out, path);
    out
}

/// A msgpack array of `len` items follows: a fixarray up to 15 items, else an array 16.
fn push_array(out: &mut Vec<u8>, len: u16) {
    if let Ok(short) = u8::try_from(len)
        && short < 16
    {
        out.push(0x90 | short);
    } else {
        out.push(0xdc);
        out.extend(len.to_be_bytes());
    }
}

/// A msgpack map of `len` pairs follows, as a fixmap: the core never sends more than 15 pairs.
fn push_map(out: &mut Vec<u8>, len: u8) {
    out.push(0x80 | (len & 0x0f));
}

/// A msgpack positive fixint: a number from 0 to 127.
fn push_uint(out: &mut Vec<u8>, value: u8) {
    out.push(value & 0x7f);
}

/// A msgpack string: a fixstr up to 31 bytes, else a str 8, str 16 or str 32 by its length.
fn push_str(out: &mut Vec<u8>, text: &str) {
    let bytes = text.as_bytes();
    let len = bytes.len();
    if let Ok(short) = u8::try_from(len) {
        if short < 32 {
            out.push(0xa0 | short);
        } else {
            out.push(0xd9);
            out.push(short);
        }
    } else if let Ok(len) = u16::try_from(len) {
        out.push(0xda);
        out.extend(len.to_be_bytes());
    } else {
        out.push(0xdb);
        // A path is never 4 GiB long; such a string is beyond msgpack anyway.
        out.extend(u32::try_from(len).unwrap_or(u32::MAX).to_be_bytes());
    }
    out.extend_from_slice(bytes);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// msgpack's fixstr of a text shorter than 32 bytes.
    fn fixstr(text: &str) -> Vec<u8> {
        let mut out = vec![0xa0 | u8::try_from(text.len()).unwrap()];
        out.extend_from_slice(text.as_bytes());
        out
    }

    /// The notification's bytes up to the path's string, which each test appends.
    fn head() -> Vec<u8> {
        let mut out = vec![0x93, 0x02];
        out.extend(fixstr("nvim_exec_autocmds"));
        out.push(0x92);
        out.extend(fixstr("User"));
        out.push(0x82);
        out.extend(fixstr("pattern"));
        out.extend(fixstr("OrcaTermWorktreeChanged"));
        out.extend(fixstr("data"));
        out.push(0x81);
        out.extend(fixstr("path"));
        out
    }

    #[test]
    fn the_notification_is_the_exact_msgpack_of_the_autocommand_call() {
        let mut expected = head();
        expected.extend(fixstr("/w/fix"));
        assert_eq!(worktree_changed("/w/fix"), expected);
        // The whole message, byte by byte, for a reader without msgpack at hand.
        assert_eq!(
            worktree_changed("/a")[..22],
            [
                0x93, 0x02, 0xb2, b'n', b'v', b'i', b'm', b'_', b'e', b'x', b'e', b'c', b'_', b'a',
                b'u', b't', b'o', b'c', b'm', b'd', b's', 0x92,
            ]
        );
        assert!(
            worktree_changed("/a").ends_with(&[0xa4, b'p', b'a', b't', b'h', 0xa2, b'/', b'a'])
        );
    }

    #[test]
    fn a_path_of_32_to_255_bytes_is_a_str8() {
        let path = format!("/{}", "x".repeat(39));
        let mut expected = head();
        expected.extend([0xd9, 40]);
        expected.extend_from_slice(path.as_bytes());
        assert_eq!(worktree_changed(&path), expected);
        let path = format!("/{}", "x".repeat(30));
        assert_eq!(
            worktree_changed(&path)[head().len()],
            0xa0 | 31,
            "31 bytes is a fixstr"
        );
    }

    #[test]
    fn a_path_of_256_bytes_or_more_is_a_str16() {
        let path = format!("/{}", "y".repeat(299));
        let mut expected = head();
        expected.extend([0xda, 0x01, 0x2c]);
        expected.extend_from_slice(path.as_bytes());
        assert_eq!(worktree_changed(&path), expected);
    }

    #[test]
    fn longer_strings_and_arrays_use_the_wider_headers() {
        let mut out = Vec::new();
        push_str(&mut out, &"z".repeat(70_000));
        assert_eq!(out[..5], [0xdb, 0x00, 0x01, 0x11, 0x70]);
        let mut out = Vec::new();
        push_array(&mut out, 16);
        assert_eq!(out, [0xdc, 0x00, 0x10]);
        let mut out = Vec::new();
        push_array(&mut out, 15);
        assert_eq!(out, [0x9f]);
    }

    #[test]
    fn only_an_absolute_path_is_an_address() {
        let address: Address = "/tmp/nvim.1/0".parse().unwrap();
        assert_eq!(address.as_path(), Path::new("/tmp/nvim.1/0"));
        assert_eq!(address.entry_bytes(), b"/tmp/nvim.1/0");
        for refused in [
            "",
            "nvim.sock",
            "./nvim.sock",
            "127.0.0.1:6666",
            "~/nvim.sock",
        ] {
            assert!(
                matches!(
                    refused.parse::<Address>(),
                    Err(AddressError::NotAbsolute(_))
                ),
                "{refused:?}"
            );
        }
        assert!(matches!(
            "/tmp/a\nb".parse::<Address>(),
            Err(AddressError::BadCharacter(_))
        ));
        let message = "rel".parse::<Address>().unwrap_err().to_string();
        assert!(
            message.contains("absolute path of a unix socket"),
            "{message}"
        );
    }

    #[test]
    fn an_address_file_is_named_by_a_stable_hash_and_reads_back() {
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        assert_eq!(fnv1a(b"a"), 0xaf63_dc4c_8601_ec8c);
        let address: Address = "/tmp/nvim.1/0".parse().unwrap();
        let name = address.entry_name();
        assert_eq!(name.len(), 16);
        assert!(is_entry_name(&name));
        assert_eq!(name, address.clone().entry_name(), "the same every time");
        let other: Address = "/tmp/nvim.2/0".parse().unwrap();
        assert_ne!(name, other.entry_name());
        assert_eq!(
            Address::from_entry(&address.entry_bytes()),
            Some(address.clone())
        );
        assert_eq!(Address::from_entry(b"/tmp/nvim.1/0\n"), Some(address));
        assert_eq!(Address::from_entry(b"relative"), None);
        assert_eq!(Address::from_entry(&[0xff, 0xfe]), None);
    }

    #[test]
    fn only_registry_names_are_entries() {
        assert!(is_entry_name("0123456789abcdef"));
        for other in [
            "0123456789ABCDEF",
            "0123456789abcde",
            ".0123456789abcdef.42.tmp",
            "notes.txt",
        ] {
            assert!(!is_entry_name(other), "{other}");
        }
    }

    #[test]
    fn the_registry_is_in_the_orca_term_directory_of_the_common_dir() {
        assert_eq!(
            registry_dir(Path::new("/r/.git")),
            Path::new("/r/.git/orca-term/nvim")
        );
    }
}
