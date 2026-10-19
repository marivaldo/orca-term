//! Domain: a worktree's setup, the repository's script from `orca-term.yaml` (`scripts.setup`)
//! run in a new worktree before anything else may use it.
//!
//! The script runs as `bash -e -c <script>` in the worktree, with the variables of
//! [`environment`] added to the inherited environment. While it runs the worktree is *setting up*;
//! when it fails the worktree is *not ready* until `orca-term worktree setup` succeeds. Nothing is
//! undone on failure.

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::domain::branch::Branch;
use crate::domain::config::Setting;
use crate::domain::state::State;

/// The file, in the worktree's admin directory, that holds the output of its latest setup.
pub(crate) const LOG_FILE: &str = "setup.log";

/// How many lines of the end of the setup log `worktree detail` shows.
pub(crate) const TAIL_LINES: usize = 20;

/// How a setup process ended when it did not succeed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Exit {
    /// It exited with this non-zero code.
    Code(i32),
    /// It was killed by this signal.
    Signal(i32),
}

/// How the end of a failed setup reads, as in `setup failed (exit 1)`.
impl fmt::Display for Exit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Code(code) => write!(f, "setup failed (exit {code})"),
            Self::Signal(signal) => write!(f, "setup killed by signal {signal}"),
        }
    }
}

/// What `worktree.json` records about the worktree's setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupRecord {
    /// A setup process started and has not recorded its end. Whether it is still alive is a
    /// volatile fact: the worktree's lock answers it on read (ADR 0003).
    Running,
    /// The latest setup succeeded.
    Succeeded,
    /// The latest setup failed, as `Exit` says; its output is in the setup log.
    Failed(Exit),
}

/// What running a worktree's setup did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SetupOutcome {
    /// No setup script is configured, so nothing ran and the worktree is ready.
    NoScript,
    /// The script exited 0.
    Succeeded,
    /// The script failed, as `Exit` says.
    Failed(Exit),
}

impl SetupOutcome {
    /// What `worktree.json` records once the setup ended: nothing when there was no script.
    pub(crate) fn record(self) -> Option<SetupRecord> {
        match self {
            Self::NoScript => None,
            Self::Succeeded => Some(SetupRecord::Succeeded),
            Self::Failed(exit) => Some(SetupRecord::Failed(exit)),
        }
    }

    /// Whether the setup failed, which leaves the worktree not ready.
    pub(crate) fn failed(self) -> bool {
        matches!(self, Self::Failed(_))
    }
}

/// What a command that runs setup reports about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SetupReport {
    pub(crate) outcome: SetupOutcome,
    /// The script that ran and which config file it came from; `None` when none is configured.
    pub(crate) script: Option<Setting<String>>,
    /// The setup log, written when a script ran.
    pub(crate) log: Option<PathBuf>,
    /// The state the setup left the worktree in: ready (no agent, or its agent's state) or not
    /// ready.
    pub(crate) state: State,
    /// The activity shown beside that state (`domain/fleet.rs`).
    pub(crate) activity: Option<String>,
    /// The agent the worktree keeps recorded through the setup, by name.
    pub(crate) agent: Option<String>,
    /// Whether the worktree is unseen after the setup (`domain/seen.rs`); a setup never changes it.
    pub(crate) unseen: bool,
    /// How many notes are pending on the worktree (`domain/note.rs`); a setup never changes it.
    pub(crate) notes: usize,
    /// How many tool calls its agent was denied in its latest turn; a setup never changes it.
    pub(crate) denials: usize,
}

/// What `worktree setup` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SetupDone {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) branch: Option<Branch>,
    pub(crate) setup: SetupReport,
}

/// The variables added to the script's environment: where the primary checkout and the worktree
/// are, and the worktree's name. `ORCA_ROOT_PATH` and `ORCA_WORKTREE_PATH` carry the same values
/// under the names Orca uses, so a setup script written for Orca works unchanged.
pub(crate) fn environment(primary: &Path, worktree: &Path) -> Vec<(&'static str, OsString)> {
    let name = worktree
        .file_name()
        .unwrap_or(worktree.as_os_str())
        .to_owned();
    vec![
        ("ORCA_TERM_ROOT_PATH", primary.as_os_str().to_owned()),
        ("ORCA_TERM_WORKTREE_PATH", worktree.as_os_str().to_owned()),
        ("ORCA_TERM_WORKTREE_NAME", name),
        ("ORCA_ROOT_PATH", primary.as_os_str().to_owned()),
        ("ORCA_WORKTREE_PATH", worktree.as_os_str().to_owned()),
    ]
}

/// The last line of `log` that is not blank, trimmed, or `None` when there is none.
pub(crate) fn last_line(log: &[u8]) -> Option<String> {
    String::from_utf8_lossy(log)
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(str::to_owned)
}

/// The last [`TAIL_LINES`] lines of the setup log whose end is `log`, as a person reads them: the
/// terminal's control sequences (colours, cursor moves, window titles) taken out, a line a
/// carriage return rewrote showing only its last rewrite, trailing blanks and blank lines at the
/// end dropped. `cut` says the log is longer than `log`, whose first line is then partial and
/// left out (unless it is the only one).
pub(crate) fn log_tail(log: &[u8], cut: bool) -> Vec<String> {
    let text = String::from_utf8_lossy(log);
    let mut lines: Vec<String> = text.split('\n').map(readable_line).collect();
    if cut && lines.len() > 1 {
        lines.remove(0);
    }
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    let start = lines.len().saturating_sub(TAIL_LINES);
    lines.split_off(start)
}

/// One line of a log as a person reads it: what its last carriage return left, without the
/// terminal's control sequences, trailing blanks trimmed.
fn readable_line(line: &str) -> String {
    let line = line.strip_suffix('\r').unwrap_or(line);
    let last_rewrite = line.rsplit('\r').next().unwrap_or_default();
    strip_terminal_codes(last_rewrite).trim_end().to_owned()
}

/// `text` without the terminal's control sequences and control characters: CSI sequences
/// (`ESC [ ... m`, colours and cursor moves, also as the one-byte CSI), OSC, DCS and the other
/// string sequences (ended by BEL or `ESC \`), character set designations (`ESC ( B`) and every
/// other two-character escape; line feeds and tabs are kept.
pub(crate) fn strip_terminal_codes(text: &str) -> String {
    let mut kept = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\u{1b}' => skip_escape(&mut chars),
            '\u{9b}' => skip_csi(&mut chars),
            '\n' | '\t' => kept.push(c),
            c if c.is_control() => {}
            c => kept.push(c),
        }
    }
    kept
}

/// Skips what follows an ESC: the rest of its sequence.
fn skip_escape(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    match chars.next() {
        Some('[') => skip_csi(chars),
        Some(']' | 'P' | 'X' | '^' | '_') => skip_string(chars),
        Some('(' | ')' | '*' | '+' | '-' | '.' | '/' | '#' | '%' | ' ') => {
            chars.next();
        }
        _ => {}
    }
}

/// Skips a CSI sequence's parameters up to its final character (`@` to `~`), included.
fn skip_csi(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    for c in chars.by_ref() {
        if ('\u{40}'..='\u{7e}').contains(&c) {
            break;
        }
    }
}

/// Skips a string sequence (OSC, DCS, ...) up to BEL or `ESC \`, included.
fn skip_string(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while let Some(c) = chars.next() {
        if c == '\u{7}' {
            break;
        }
        if c == '\u{1b}' {
            chars.next_if_eq(&'\\');
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_gets_the_paths_and_the_name_under_both_prefixes() {
        let vars = environment(Path::new("/repo"), Path::new("/w/repo/fix"));
        let pairs: Vec<(&str, &str)> = vars
            .iter()
            .map(|(name, value)| (*name, value.to_str().unwrap()))
            .collect();
        assert_eq!(
            pairs,
            [
                ("ORCA_TERM_ROOT_PATH", "/repo"),
                ("ORCA_TERM_WORKTREE_PATH", "/w/repo/fix"),
                ("ORCA_TERM_WORKTREE_NAME", "fix"),
                ("ORCA_ROOT_PATH", "/repo"),
                ("ORCA_WORKTREE_PATH", "/w/repo/fix"),
            ]
        );
    }

    #[test]
    fn the_last_line_skips_blank_lines() {
        assert_eq!(
            last_line(b"installing\nerror: no lockfile\n\n  \n"),
            Some("error: no lockfile".to_owned())
        );
        assert_eq!(last_line(b"\r\n  done  \r\n"), Some("done".to_owned()));
        assert_eq!(last_line(b""), None);
        assert_eq!(last_line(b"\n\n"), None);
    }

    #[test]
    fn the_terminals_control_sequences_are_taken_out() {
        assert_eq!(
            strip_terminal_codes("\u{1b}[1;31merror\u{1b}[0m: no lockfile"),
            "error: no lockfile"
        );
        assert_eq!(
            strip_terminal_codes("\u{1b}]0;title\u{7}a\u{1b}]8;;https://x\u{1b}\\b\u{1b}(Bc"),
            "abc",
            "OSC ended by BEL or ESC \\, and a character set designation"
        );
        assert_eq!(
            strip_terminal_codes("\u{1b}[?25l\u{1b}[2K\u{1b}=x\u{9b}1my\u{8}\u{7}z\tw"),
            "xyz\tw",
            "private modes, keypad mode, the one-byte CSI and stray control characters"
        );
        assert_eq!(strip_terminal_codes("na\u{ef}ve \u{1b}"), "na\u{ef}ve ");
    }

    #[test]
    fn the_log_tail_is_the_last_lines_as_a_person_reads_them() {
        let log = b"\x1b[32mfetching\x1b[0m\r\n 10%\r 50%\r100%\r\nerror: boom  \n\n\n";
        assert_eq!(log_tail(log, false), ["fetching", "100%", "error: boom"]);
        assert!(log_tail(b"", false).is_empty());
        assert!(log_tail(b"\n\n", false).is_empty());

        let many = (1..=30)
            .map(|n| format!("line {n}"))
            .collect::<Vec<_>>()
            .join("\n");
        let tail = log_tail(many.as_bytes(), false);
        assert_eq!(tail.len(), TAIL_LINES);
        assert_eq!(tail.first().map(String::as_str), Some("line 11"));
        assert_eq!(tail.last().map(String::as_str), Some("line 30"));

        assert_eq!(
            log_tail(b"ial line\nwhole\n", true),
            ["whole"],
            "the partial first line of a cut log is left out"
        );
        assert_eq!(log_tail(b"only a part", true), ["only a part"]);
        assert_eq!(
            log_tail(b"\xffbad\n", false),
            ["\u{fffd}bad"],
            "bytes that are not UTF-8 are replaced"
        );
    }

    #[test]
    fn a_failure_says_how_the_process_ended() {
        assert_eq!(Exit::Code(1).to_string(), "setup failed (exit 1)");
        assert_eq!(Exit::Signal(9).to_string(), "setup killed by signal 9");
    }

    #[test]
    fn only_a_script_that_ran_is_recorded() {
        assert_eq!(SetupOutcome::NoScript.record(), None);
        assert_eq!(
            SetupOutcome::Succeeded.record(),
            Some(SetupRecord::Succeeded)
        );
        assert_eq!(
            SetupOutcome::Failed(Exit::Code(2)).record(),
            Some(SetupRecord::Failed(Exit::Code(2)))
        );
        assert!(SetupOutcome::Failed(Exit::Signal(15)).failed());
        assert!(!SetupOutcome::NoScript.failed());
    }
}
