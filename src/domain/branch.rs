//! Domain: branches. A branch's name, the start point a new worktree's branch is created from, and
//! git's rules for a valid branch name.

use std::fmt;

/// A local branch's name, without `refs/heads/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Branch(String);

impl Branch {
    /// A branch git reported or accepted, taken as it is.
    pub(crate) fn new(name: &str) -> Self {
        Self(name.to_owned())
    }

    /// The branch's name, as git writes it.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for Branch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Where a new worktree's branch starts: the default branch, local or on `origin`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StartPoint {
    /// The local branch, such as `main`.
    Local(Branch),
    /// The branch on `origin`, such as `origin/main`, when there is no local one.
    Remote(Branch),
}

/// Shows the start point as git reads it: `main` or `origin/main`.
impl fmt::Display for StartPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local(branch) => write!(f, "{branch}"),
            Self::Remote(branch) => write!(f, "origin/{branch}"),
        }
    }
}

/// The rule of git's that a branch name breaks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub(crate) enum BranchNameError {
    #[error("it is empty")]
    Empty,
    #[error("it is `@`")]
    LoneAt,
    #[error("it is `HEAD`")]
    Head,
    #[error("it starts with `.`")]
    StartsWithDot,
    #[error("it ends with `.`")]
    EndsWithDot,
    #[error("it ends with `.lock`")]
    EndsWithLock,
    #[error("it contains `..`")]
    DoubleDot,
    #[error("it contains `@{{`")]
    AtBrace,
    #[error("it contains a space")]
    Space,
    #[error("it contains a control character")]
    ControlCharacter,
    #[error("it contains `{0}`")]
    Forbidden(char),
}

/// Characters git never allows in a branch name.
const FORBIDDEN: [char; 7] = ['~', '^', ':', '?', '*', '[', '\\'];

/// Checks `name`, a branch name of one segment (no `/`), against the rules
/// `git check-ref-format --branch` applies, without running git.
#[expect(
    clippy::case_sensitive_file_extension_comparisons,
    reason = "`.lock` is git's rule, not a file extension, and git compares it case-sensitively"
)]
pub(crate) fn check_name(name: &str) -> Result<(), BranchNameError> {
    let broken = match name {
        "" => Some(BranchNameError::Empty),
        "@" => Some(BranchNameError::LoneAt),
        "HEAD" => Some(BranchNameError::Head),
        _ if name.starts_with('.') => Some(BranchNameError::StartsWithDot),
        _ if name.ends_with('.') => Some(BranchNameError::EndsWithDot),
        _ if name.ends_with(".lock") => Some(BranchNameError::EndsWithLock),
        _ if name.contains("..") => Some(BranchNameError::DoubleDot),
        _ if name.contains("@{") => Some(BranchNameError::AtBrace),
        _ => broken_character(name),
    };
    match broken {
        Some(rule) => Err(rule),
        None => Ok(()),
    }
}

/// The first character of `name` git never allows in a branch name, as the rule it breaks.
fn broken_character(name: &str) -> Option<BranchNameError> {
    name.chars().find_map(|c| match c {
        ' ' => Some(BranchNameError::Space),
        _ if c.is_ascii_control() => Some(BranchNameError::ControlCharacter),
        _ if FORBIDDEN.contains(&c) => Some(BranchNameError::Forbidden(c)),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_git_accepts_pass() {
        for name in [
            "fix-login",
            "a@b",
            "a{b}",
            "x.lock.y",
            "x.LOCK",
            "v1.2",
            "ü",
            "under_score",
        ] {
            assert_eq!(check_name(name), Ok(()), "{name}");
        }
    }

    #[test]
    fn names_git_rejects_name_the_rule_they_break() {
        for (name, rule) in [
            ("", BranchNameError::Empty),
            ("@", BranchNameError::LoneAt),
            ("HEAD", BranchNameError::Head),
            (".hidden", BranchNameError::StartsWithDot),
            ("trailing.", BranchNameError::EndsWithDot),
            ("x.lock", BranchNameError::EndsWithLock),
            ("bad..name", BranchNameError::DoubleDot),
            ("a@{1}", BranchNameError::AtBrace),
            ("has space", BranchNameError::Space),
            ("tab\there", BranchNameError::ControlCharacter),
            ("del\u{7f}", BranchNameError::ControlCharacter),
            ("a~1", BranchNameError::Forbidden('~')),
            ("a^1", BranchNameError::Forbidden('^')),
            ("a:b", BranchNameError::Forbidden(':')),
            ("a?b", BranchNameError::Forbidden('?')),
            ("a*b", BranchNameError::Forbidden('*')),
            ("a[b", BranchNameError::Forbidden('[')),
            ("a\\b", BranchNameError::Forbidden('\\')),
        ] {
            assert_eq!(check_name(name), Err(rule), "{name:?}");
        }
    }

    #[test]
    fn a_rule_reads_as_a_sentence() {
        assert_eq!(BranchNameError::DoubleDot.to_string(), "it contains `..`");
        assert_eq!(BranchNameError::AtBrace.to_string(), "it contains `@{`");
        assert_eq!(
            BranchNameError::Forbidden('~').to_string(),
            "it contains `~`"
        );
    }

    #[test]
    fn a_start_point_reads_as_git_writes_it() {
        assert_eq!(StartPoint::Local(Branch::new("main")).to_string(), "main");
        assert_eq!(
            StartPoint::Remote(Branch::new("trunk")).to_string(),
            "origin/trunk"
        );
    }
}
