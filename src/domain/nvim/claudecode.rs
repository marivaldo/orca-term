//! Domain: whether the running Neovims of a repository have coder/claudecode.nvim loaded, the
//! plugin that serves Claude Code's editor protocol (#179), as `orca-term doctor` reports it. The
//! person installs the plugin in their own Neovim; orca-term ships no part of it. claude, started
//! with `--ide` (`domain/launch.rs`), links to the one Neovim of its worktree that runs it.
//!
//! The doctor asks each Neovim registered with the core one msgpack-RPC request, and reads its
//! answer, a single boolean. It never runs anything in Neovim that changes it, and a Neovim that
//! cannot answer only leaves the plugin unchecked: the line is a warning at worst, never a failure.

use super::{REQUEST_ID, push_array, push_str, push_uint};
use crate::domain::prerequisite::{Check, Status};

/// The doctor check's name.
pub(crate) const NAME: &str = "claudecode";

/// The Lua the request runs in Neovim: whether the plugin's module is loaded. It only reads.
/// `require` would load a plugin that loads lazily and start its server, a change the doctor
/// must never make; and a plugin not loaded yet has written no lock file, so claude could not
/// link to it anyway.
pub(crate) const PROBE: &str = "return package.loaded['claudecode'] ~= nil";

/// The length of the answer to [`request`], in bytes: `[1, REQUEST_ID, nil, <boolean>]`.
pub(crate) const ANSWER_LEN: usize = 5;

/// The msgpack-RPC request that asks a Neovim whether it has the plugin loaded:
/// `[0, REQUEST_ID, "nvim_exec_lua", [PROBE, []]]`.
pub(crate) fn request() -> Vec<u8> {
    let mut out = Vec::new();
    push_array(&mut out, 4);
    push_uint(&mut out, 0);
    push_uint(&mut out, REQUEST_ID);
    push_str(&mut out, "nvim_exec_lua");
    // The call's arguments: the code, then the arguments it is given, none.
    push_array(&mut out, 2);
    push_str(&mut out, PROBE);
    push_array(&mut out, 0);
    out
}

/// What one Neovim said about the plugin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Plugin {
    Loaded,
    NotLoaded,
    /// The Neovim did not answer in time, is gone, or answered something else (an error).
    NoAnswer,
}

/// What the first [`ANSWER_LEN`] bytes of a Neovim's answer (`None` when it gave none) say. A
/// successful answer is a response (type 1) to [`REQUEST_ID`] with no error (nil) and a boolean;
/// anything else, such as an error raised by the Lua, is no answer.
pub(crate) fn read_answer(answer: Option<&[u8]>) -> Plugin {
    const RESPONSE: u8 = 1;
    const NIL: u8 = 0xc0;
    const FALSE: u8 = 0xc2;
    const TRUE: u8 = 0xc3;
    match answer {
        Some([0x94, RESPONSE, id, NIL, TRUE]) if u64::from(*id) == REQUEST_ID => Plugin::Loaded,
        Some([0x94, RESPONSE, id, NIL, FALSE]) if u64::from(*id) == REQUEST_ID => Plugin::NotLoaded,
        _ => Plugin::NoAnswer,
    }
}

/// The doctor's line from what each registered Neovim said, `found` empty when none is
/// registered: ok when every one that answered has the plugin loaded, a warning when one has not,
/// and only information when none answered, since then nothing was checked.
pub(crate) fn check(found: &[Plugin]) -> Check {
    let registered = found.len();
    let count = |plugin: Plugin| found.iter().filter(|&&p| p == plugin).count();
    let loaded = count(Plugin::Loaded);
    let not_loaded = count(Plugin::NotLoaded);
    let (status, message) = if registered == 0 {
        (
            Status::Info,
            "claudecode.nvim not checked: no Neovim of this repository is registered with \
             orca-term"
                .to_owned(),
        )
    } else if loaded + not_loaded == 0 {
        (
            Status::Info,
            format!(
                "claudecode.nvim not checked: none of the {registered} registered Neovims \
                 answered"
            ),
        )
    } else if not_loaded > 0 {
        (
            Status::Warn,
            format!(
                "claudecode.nvim not loaded in {not_loaded} of {registered} registered Neovims; \
                 claude links only to a Neovim running it (see the README)"
            ),
        )
    } else {
        (
            Status::Ok,
            format!("claudecode.nvim loaded in {loaded} of {registered} registered Neovims"),
        )
    };
    Check {
        name: NAME,
        status,
        found: None,
        need: None,
        message,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_request_is_the_exact_msgpack_of_the_lua_call() {
        let mut expected = vec![0x94, 0x00, 0x01, 0xad];
        expected.extend_from_slice(b"nvim_exec_lua");
        expected.extend([0x92, 0xd9, 42]);
        expected.extend_from_slice(PROBE.as_bytes());
        expected.push(0x90);
        assert_eq!(PROBE.len(), 42);
        assert_eq!(request(), expected);
    }

    #[test]
    fn the_probe_only_reads_whether_the_module_is_loaded() {
        assert_eq!(PROBE, "return package.loaded['claudecode'] ~= nil");
        assert!(!PROBE.contains("require"), "require would load the plugin");
    }

    #[test]
    fn only_a_boolean_answer_to_our_request_counts() {
        let answer = |bytes: &[u8]| read_answer(Some(bytes));
        assert_eq!(answer(&[0x94, 0x01, 0x01, 0xc0, 0xc3]), Plugin::Loaded);
        assert_eq!(answer(&[0x94, 0x01, 0x01, 0xc0, 0xc2]), Plugin::NotLoaded);
        assert_eq!(read_answer(None), Plugin::NoAnswer);
        for other in [
            &[0x94, 0x01, 0x02, 0xc0, 0xc3][..],
            &[0x94, 0x00, 0x01, 0xc0, 0xc3],
            &[0x94, 0x01, 0x01, 0x92, 0x01],
            &[0x94, 0x01, 0x01, 0xc0, 0xc0],
            &[0x94, 0x01, 0x01, 0xc0],
            &[],
        ] {
            assert_eq!(answer(other), Plugin::NoAnswer, "{other:?}");
        }
    }

    #[test]
    fn the_line_is_ok_a_warning_or_not_checked_and_never_refuses() {
        use Plugin::{Loaded, NoAnswer, NotLoaded};
        let none = check(&[]);
        assert_eq!(none.name, "claudecode");
        assert_eq!(none.status, Status::Info);
        assert_eq!(
            none.message,
            "claudecode.nvim not checked: no Neovim of this repository is registered with \
             orca-term"
        );
        let silent = check(&[NoAnswer, NoAnswer]);
        assert_eq!(silent.status, Status::Info);
        assert_eq!(
            silent.message,
            "claudecode.nvim not checked: none of the 2 registered Neovims answered"
        );
        let loaded = check(&[Loaded, NoAnswer]);
        assert_eq!(loaded.status, Status::Ok);
        assert_eq!(
            loaded.message,
            "claudecode.nvim loaded in 1 of 2 registered Neovims"
        );
        let missing = check(&[Loaded, NotLoaded]);
        assert_eq!(missing.status, Status::Warn);
        assert_eq!(
            missing.message,
            "claudecode.nvim not loaded in 1 of 2 registered Neovims; claude links only to a \
             Neovim running it (see the README)"
        );
        for line in [none, silent, loaded, missing] {
            assert!(!line.status.refuses(), "{line:?}");
            assert_eq!((line.found, line.need), (None, None));
        }
    }
}
