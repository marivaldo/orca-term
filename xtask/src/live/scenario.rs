//! The live scenarios: one small text file each, `tests/live/<name>.scenario`, saying which agent
//! to drive, what to type, and what the recording must replay to. A line is one directive; blank
//! lines and lines starting with `#` are comments, and the comments a manual scenario starts with
//! are its written procedure.
//!
//! ```text
//! agent claude-code            the agent: claude-code or opencode
//! recording p-hello            the recording's name: <name>-<version>.log
//! timeout 120                  seconds the whole scenario may take
//! expect finished              the outcome of each turn the recording must replay to
//! expect-denials 1             how many denials each turn must count, in the same order; the
//!                              counts are not checked when the line is left out
//! check-missing {{denied}}/x   a file that must not exist once the agent stopped
//! file opencode.json {"a": 1}  a file written into the worktree before the agent starts: its
//!                              path in the worktree, then its whole text on the same line
//! manual                       recorded by hand (`just record-live <name>`), never by check-live
//!
//! sleep 2                      waits, in seconds (`0.5` too)
//! type reply with ok           types the text, as is
//! press enter                  presses a key: enter, esc, tab, space, backspace, up, down, ctrl-c
//! wait-for Stop                waits for the agent's next hook (claude) or line (opencode) of
//!                              that kind
//! ```
//!
//! `{{worktree}}` and `{{denied}}` in a typed text or a `check-missing` path stand for the
//! scenario's worktree and for a directory the sandbox denies (the worktree's admin directory).
//! A `file`'s text is written as is, such as a project config the agent reads from its worktree.
//! The suite that replays the recordings (`tests/recordings.rs`) checks the outcomes and the
//! denials; this runner only checks that they are well formed.

use std::path::{Component, Path};
use std::time::Duration;

/// The longest timeout a scenario may ask for: an hour.
const MAX_TIMEOUT: u64 = 3600;

/// The outcomes a turn can end with, plus `open` for one that never ended.
const OUTCOMES: &[&str] = &["finished", "failed", "interrupted", "open"];

/// The agent a scenario drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Agent {
    ClaudeCode,
    OpenCode,
}

impl Agent {
    /// The agent's name, as `agent start --agent` takes it.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-code",
            Self::OpenCode => "opencode",
        }
    }

    /// The agent's program, which is also the name of its check in `orca-term doctor`.
    pub(crate) fn program(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude",
            Self::OpenCode => "opencode",
        }
    }

    /// The directory of `tests/fixtures/` its recordings are in.
    pub(crate) fn recordings_dir(self) -> &'static str {
        match self {
            Self::ClaudeCode => "claude-hooks",
            Self::OpenCode => "opencode-events",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        [Self::ClaudeCode, Self::OpenCode]
            .into_iter()
            .find(|agent| agent.name() == name)
    }
}

/// A key `press` can press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Key {
    Enter,
    Esc,
    Tab,
    Space,
    Backspace,
    Up,
    Down,
    CtrlC,
}

impl Key {
    const ALL: [(&'static str, Self); 8] = [
        ("enter", Self::Enter),
        ("esc", Self::Esc),
        ("tab", Self::Tab),
        ("space", Self::Space),
        ("backspace", Self::Backspace),
        ("up", Self::Up),
        ("down", Self::Down),
        ("ctrl-c", Self::CtrlC),
    ];

    /// The bytes a terminal sends for the key.
    pub(crate) fn bytes(self) -> &'static [u8] {
        match self {
            Self::Enter => b"\r",
            Self::Esc => b"\x1b",
            Self::Tab => b"\t",
            Self::Space => b" ",
            Self::Backspace => b"\x7f",
            Self::Up => b"\x1b[A",
            Self::Down => b"\x1b[B",
            Self::CtrlC => b"\x03",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, key)| *key)
    }
}

/// One step of a scenario.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Step {
    Sleep(Duration),
    Type(String),
    Press(Key),
    /// Waits for the agent's next hook or line of this kind.
    WaitFor(String),
}

/// A scenario, read from its file.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Scenario {
    /// The file's name without `.scenario`.
    pub(crate) name: String,
    pub(crate) agent: Agent,
    pub(crate) recording: String,
    pub(crate) timeout: Duration,
    /// The outcome of each turn, in order.
    pub(crate) expect: Vec<String>,
    /// Files that must not exist once the agent stopped, placeholders unexpanded.
    pub(crate) check_missing: Vec<String>,
    /// Files written into the worktree before the agent starts: each one's path in the worktree
    /// and its text.
    pub(crate) files: Vec<(String, String)>,
    /// Recorded by hand rather than by `check-live`.
    pub(crate) manual: bool,
    /// The comment lines the file starts with, without their `#`.
    pub(crate) about: Vec<String>,
    pub(crate) steps: Vec<Step>,
}

/// Reads the scenario `name` from its file's `text`, or says what is wrong and on which line.
pub(crate) fn parse(name: &str, text: &str) -> Result<Scenario, String> {
    let mut agent = None;
    let mut recording = None;
    let mut timeout = None;
    let mut expect = Vec::new();
    let mut expect_denials = None;
    let mut check_missing = Vec::new();
    let mut files = Vec::new();
    let mut manual = false;
    let mut steps = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let at = |why: String| format!("line {}: {why}", number + 1);
        let (word, rest) = line.split_once(' ').unwrap_or((line, ""));
        let rest = rest.trim();
        match word {
            "agent" => agent = Some(Agent::from_name(rest).ok_or_else(|| at(unknown_agent(rest)))?),
            "recording" => recording = Some(recording_name(rest).map_err(at)?),
            "timeout" => timeout = Some(timeout_secs(rest).map_err(at)?),
            "expect" => expect = outcomes(rest).map_err(at)?,
            "expect-denials" => expect_denials = Some(denial_counts(rest).map_err(at)?),
            "check-missing" if !rest.is_empty() => check_missing.push(rest.to_owned()),
            "file" => files.push(file(rest).map_err(at)?),
            "manual" if rest.is_empty() => manual = true,
            _ => steps.push(step(word, rest).map_err(at)?),
        }
    }
    let agent = agent.ok_or("no `agent` line")?;
    let recording = recording.ok_or("no `recording` line")?;
    let timeout = timeout.ok_or("no `timeout` line")?;
    if expect.is_empty() {
        return Err("no `expect` line".to_owned());
    }
    check_denial_counts(&expect, expect_denials.as_deref())?;
    if manual && !steps.is_empty() {
        return Err("a manual scenario has no steps: its comments are the procedure".to_owned());
    }
    if !manual && steps.is_empty() {
        return Err("no steps".to_owned());
    }
    Ok(Scenario {
        name: name.to_owned(),
        agent,
        recording,
        timeout,
        expect,
        check_missing,
        files,
        manual,
        about: about(text),
        steps,
    })
}

/// The denial counts of `expect-denials`, one whole number per turn.
fn denial_counts(text: &str) -> Result<Vec<u64>, String> {
    let counts: Option<Vec<u64>> = text.split_whitespace().map(|n| n.parse().ok()).collect();
    match counts {
        Some(counts) if !counts.is_empty() => Ok(counts),
        _ => Err(format!(
            "`expect-denials {text}`: give a whole number for each turn"
        )),
    }
}

/// Checks that `expect-denials`, when given, counts as many turns as `expect` names.
fn check_denial_counts(expect: &[String], counts: Option<&[u64]>) -> Result<(), String> {
    match counts {
        Some(counts) if counts.len() != expect.len() => Err(format!(
            "`expect-denials` counts {} turns, but `expect` names {}",
            counts.len(),
            expect.len()
        )),
        _ => Ok(()),
    }
}

/// The file of `file <path> <text>`: a path inside the worktree (relative, with no `..`), and
/// the text, which may not be empty.
fn file(rest: &str) -> Result<(String, String), String> {
    let (path, text) = rest.split_once(' ').unwrap_or((rest, ""));
    let text = text.trim();
    let inside = !path.is_empty()
        && Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
    if !inside {
        return Err(format!(
            "`file {path}`: give a path inside the worktree, relative and with no `..`"
        ));
    }
    if text.is_empty() {
        return Err(format!("`file {path}`: give its text after the path"));
    }
    Ok((path.to_owned(), text.to_owned()))
}

/// The step `word rest`.
fn step(word: &str, rest: &str) -> Result<Step, String> {
    match word {
        "sleep" => {
            let secs: f64 = rest
                .parse()
                .ok()
                .filter(|secs: &f64| secs.is_finite() && *secs > 0.0 && *secs <= 60.0)
                .ok_or_else(|| {
                    format!("`sleep {rest}`: give seconds, more than 0 and at most 60")
                })?;
            Ok(Step::Sleep(Duration::from_secs_f64(secs)))
        }
        "type" if !rest.is_empty() => Ok(Step::Type(rest.to_owned())),
        "press" => Key::from_name(rest).map(Step::Press).ok_or_else(|| {
            let keys: Vec<&str> = Key::ALL.iter().map(|(name, _)| *name).collect();
            format!("`press {rest}`: the keys are {}", keys.join(", "))
        }),
        "wait-for" if is_word(rest) => Ok(Step::WaitFor(rest.to_owned())),
        _ => Err(format!(
            "`{}` is not a directive",
            format!("{word} {rest}").trim()
        )),
    }
}

fn unknown_agent(name: &str) -> String {
    format!("unknown agent `{name}`; the agents are claude-code and opencode")
}

/// A recording's name: a lowercase letter, then lowercase letters, digits and dashes.
fn recording_name(name: &str) -> Result<String, String> {
    let ok = name.starts_with(|c: char| c.is_ascii_lowercase())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-');
    if ok {
        Ok(name.to_owned())
    } else {
        Err(format!(
            "`recording {name}`: a name is lowercase letters, digits and dashes"
        ))
    }
}

fn timeout_secs(text: &str) -> Result<Duration, String> {
    text.parse::<u64>()
        .ok()
        .filter(|secs| (1..=MAX_TIMEOUT).contains(secs))
        .map(Duration::from_secs)
        .ok_or_else(|| format!("`timeout {text}`: give whole seconds, 1 to {MAX_TIMEOUT}"))
}

fn outcomes(text: &str) -> Result<Vec<String>, String> {
    let words: Vec<String> = text.split_whitespace().map(str::to_owned).collect();
    match words.iter().find(|word| !OUTCOMES.contains(&word.as_str())) {
        Some(word) => Err(format!(
            "`{word}` is not an outcome; the outcomes are {}",
            OUTCOMES.join(", ")
        )),
        None if words.is_empty() => Err("`expect` names no outcome".to_owned()),
        None => Ok(words),
    }
}

/// Whether `text` is one word of letters, digits, `_` and `-`: a hook or a line type.
fn is_word(text: &str) -> bool {
    !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The comment lines `text` starts with, without their `#` and the space after it.
fn about(text: &str) -> Vec<String> {
    text.lines()
        .map_while(|line| line.strip_prefix('#'))
        .map(|line| line.strip_prefix(' ').unwrap_or(line).to_owned())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: &str = "\
# A plain prompt.
# One prompt.
agent claude-code
recording p-hello
timeout 120
expect finished

sleep 0.5
type reply with ok
press enter
wait-for Stop
";

    #[test]
    fn a_scenario_reads_its_agent_recording_timeout_outcomes_and_steps() {
        let scenario = parse("p-hello", HELLO).unwrap();
        assert_eq!(scenario.agent, Agent::ClaudeCode);
        assert_eq!(scenario.recording, "p-hello");
        assert_eq!(scenario.timeout, Duration::from_mins(2));
        assert_eq!(scenario.expect, ["finished"]);
        assert!(!scenario.manual);
        assert_eq!(scenario.about, ["A plain prompt.", "One prompt."]);
        assert_eq!(
            scenario.steps,
            [
                Step::Sleep(Duration::from_millis(500)),
                Step::Type("reply with ok".to_owned()),
                Step::Press(Key::Enter),
                Step::WaitFor("Stop".to_owned()),
            ]
        );
    }

    #[test]
    fn a_manual_scenario_has_no_steps_and_an_automatic_one_needs_some() {
        let manual = "agent opencode\nrecording x\ntimeout 9\nexpect open\nmanual\n";
        let scenario = parse("x", manual).unwrap();
        assert!(scenario.manual);
        assert_eq!(scenario.agent, Agent::OpenCode);
        assert!(
            parse("x", &format!("{manual}sleep 1\n"))
                .unwrap_err()
                .contains("no steps")
        );
        assert_eq!(
            parse("x", "agent opencode\nrecording x\ntimeout 9\nexpect open\n").unwrap_err(),
            "no steps"
        );
    }

    #[test]
    fn a_mistake_is_named_with_its_line() {
        let wrong = |line: &str| parse("x", &HELLO.replace("press enter", line)).unwrap_err();
        assert_eq!(
            wrong("press f1"),
            "line 10: `press f1`: the keys are enter, esc, tab, space, backspace, up, down, ctrl-c"
        );
        assert_eq!(
            wrong("sleep 0"),
            "line 10: `sleep 0`: give seconds, more than 0 and at most 60"
        );
        assert_eq!(
            wrong("dance now"),
            "line 10: `dance now` is not a directive"
        );
        assert_eq!(
            wrong("wait-for two words"),
            "line 10: `wait-for two words` is not a directive"
        );
        let bad = |from: &str, to: &str| parse("x", &HELLO.replace(from, to)).unwrap_err();
        assert_eq!(
            bad("agent claude-code", "agent claude"),
            "line 3: unknown agent `claude`; the agents are claude-code and opencode"
        );
        assert_eq!(
            bad("expect finished", "expect done"),
            "line 6: `done` is not an outcome; the outcomes are finished, failed, interrupted, open"
        );
        assert_eq!(
            bad("timeout 120", "timeout 0"),
            "line 5: `timeout 0`: give whole seconds, 1 to 3600"
        );
        assert_eq!(
            bad("recording p-hello", "recording P_hello"),
            "line 4: `recording P_hello`: a name is lowercase letters, digits and dashes"
        );
        assert_eq!(bad("agent claude-code\n", ""), "no `agent` line");
    }

    #[test]
    fn a_scenario_may_write_files_into_the_worktree_and_expect_denials() {
        let config = r#"{"permission": {"bash": {"touch *": "deny"}}}"#;
        let text = HELLO.replace(
            "expect finished\n",
            &format!(
                "expect finished\nexpect-denials 1\nfile opencode.json {config}\n\
                 file .opencode/notes.txt two  words\n"
            ),
        );
        let scenario = parse("x", &text).unwrap();
        assert_eq!(
            scenario.files,
            [
                ("opencode.json".to_owned(), config.to_owned()),
                (".opencode/notes.txt".to_owned(), "two  words".to_owned())
            ]
        );
        assert_eq!(parse("p-hello", HELLO).unwrap().files, []);
    }

    #[test]
    fn a_wrong_file_or_denial_count_is_named() {
        let wrong = |line: &str| {
            parse(
                "x",
                &HELLO.replace("expect finished\n", &format!("expect finished\n{line}\n")),
            )
            .unwrap_err()
        };
        assert_eq!(
            wrong("expect-denials 1 0"),
            "`expect-denials` counts 2 turns, but `expect` names 1"
        );
        assert_eq!(
            wrong("expect-denials one"),
            "line 7: `expect-denials one`: give a whole number for each turn"
        );
        assert_eq!(
            wrong("expect-denials"),
            "line 7: `expect-denials `: give a whole number for each turn"
        );
        for outside in ["/etc/x.json", "../x.json", "a/../../x.json", "./x.json"] {
            assert_eq!(
                wrong(&format!("file {outside} {{}}")),
                format!(
                    "line 7: `file {outside}`: give a path inside the worktree, relative and with no `..`"
                )
            );
        }
        assert_eq!(
            wrong("file opencode.json"),
            "line 7: `file opencode.json`: give its text after the path"
        );
    }

    #[test]
    fn every_key_has_its_terminal_bytes() {
        assert_eq!(Key::Enter.bytes(), b"\r");
        assert_eq!(Key::Esc.bytes(), b"\x1b");
        assert_eq!(Key::Down.bytes(), b"\x1b[B");
        assert_eq!(Key::CtrlC.bytes(), b"\x03");
        for (name, key) in Key::ALL {
            assert_eq!(Key::from_name(name), Some(key));
            assert!(!key.bytes().is_empty());
        }
    }
}
