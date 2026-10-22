//! Domain: a worktree's review (`worktree review`). The review shows everything the worktree
//! changed since it forked from the repository's default branch, committed or not: the client
//! compares the working tree with the review's base, the commit where the worktree's history
//! leaves the default branch's (`git merge-base <default branch> HEAD`). The core finds that base,
//! since the client runs no git itself.

use std::fmt;

use anyhow::{Result, bail};

use crate::domain::branch::StartPoint;
use crate::domain::fleet::Worktree;
use crate::domain::state::State;

/// How many characters of a commit id the text output shows, as `git log --oneline` does.
const SHORT_LEN: usize = 7;

/// A full commit id as git prints it: 40 lowercase hex characters (64 in a repository using
/// SHA-256).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Commit(String);

impl Commit {
    /// The commit id in `text`, which git printed. Refuses anything that is not a full id, so a
    /// surprise in git's output never reaches the client as a base.
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let is_hex = text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !is_hex || !matches!(text.len(), 40 | 64) {
            bail!("git printed `{text}` where a full commit id was expected");
        }
        Ok(Self(text.to_owned()))
    }

    /// The full id.
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }

    /// The id's first seven characters, for a person to read.
    pub(crate) fn short(&self) -> &str {
        &self.0[..SHORT_LEN]
    }
}

impl fmt::Display for Commit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What `worktree review` found: the worktree as it is now (marked seen), the review's base, and
/// the default branch it was taken from, as git names it (`main` or `origin/main`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Reviewed {
    pub(crate) worktree: Worktree,
    pub(crate) base: Commit,
    pub(crate) against: StartPoint,
}

/// Refuses to review a worktree whose directory is gone: there is nothing left to compare.
pub(crate) fn check_can_review(worktree: &Worktree) -> Result<()> {
    if let State::Broken(reason) = &worktree.state
        && reason.is_gone()
    {
        bail!(
            "worktree {} at {} is broken ({reason}): its directory is gone, so it has nothing to \
             review",
            worktree.name,
            worktree.path.display(),
        );
    }
    Ok(())
}

/// The error for a worktree whose history shares no commit with the default branch `against`.
pub(crate) fn no_base(worktree: &Worktree, against: &StartPoint) -> anyhow::Error {
    anyhow::anyhow!(
        "cannot review worktree {}: its history shares no commit with the default branch \
         `{against}`, so there is no base to review against",
        worktree.name,
    )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::domain::agent::HolderStatus;
    use crate::domain::branch::Branch;
    use crate::domain::state::BrokenReason;

    const SHA: &str = "1a2b3c4d5e6f7a8b9c0d1e2f3a4b5c6d7e8f9a0b";

    fn worktree(state: State) -> Worktree {
        Worktree {
            name: "demo".to_owned(),
            path: PathBuf::from("/l/demo"),
            branch: Some(Branch::new("demo")),
            state,
            activity: None,
            agent: None,
            holder: HolderStatus::Gone,
            unseen: false,
            notes: 0,
            denials: 0,
            operation: None,
        }
    }

    #[test]
    fn a_full_commit_id_is_taken_and_shortened_to_seven_characters() {
        let commit = Commit::parse(SHA).unwrap();
        assert_eq!(commit.as_str(), SHA);
        assert_eq!(commit.short(), "1a2b3c4");
        let sha256 = "a".repeat(64);
        assert_eq!(Commit::parse(&sha256).unwrap().as_str(), sha256);
    }

    #[test]
    fn anything_but_a_full_commit_id_is_refused() {
        for text in [
            "",
            "1a2b3c4",
            &SHA.to_uppercase(),
            &format!("{SHA}\n"),
            "main",
        ] {
            assert!(Commit::parse(text).is_err(), "{text:?}");
        }
        let near = format!("{}g", &SHA[..39]);
        assert!(Commit::parse(&near).is_err());
    }

    #[test]
    fn a_worktree_whose_directory_is_gone_has_nothing_to_review() {
        for reason in [
            BrokenReason::DirectoryMissing,
            BrokenReason::Prunable("gitdir file points to non-existent location".to_owned()),
        ] {
            let err = check_can_review(&worktree(State::Broken(reason))).unwrap_err();
            assert!(err.to_string().contains("nothing to review"), "{err}");
        }
        // Its directory is still there: the review can compare its files.
        let unreadable = State::Broken(BrokenReason::StateUnreadable("corrupt".to_owned()));
        assert!(check_can_review(&worktree(unreadable)).is_ok());
        assert!(check_can_review(&worktree(State::NoAgent)).is_ok());
    }

    #[test]
    fn no_base_names_the_worktree_and_the_default_branch() {
        let against = StartPoint::Remote(Branch::new("main"));
        let message = no_base(&worktree(State::NoAgent), &against).to_string();
        assert!(message.contains("worktree demo"), "{message}");
        assert!(message.contains("`origin/main`"), "{message}");
    }
}
