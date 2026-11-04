//! Domain: rebasing a worktree's branch onto its base (#141), as Orca's **Rebase from `<base>`**
//! does, and handing the conflicts of a rebase to the worktree's agent.
//!
//! The base is always the default branch on `origin` (`origin/main`), never a local branch. The
//! rebase fetches it into a private temporary ref, [`temp_ref`], so a fetch running at the same
//! time cannot move it, rebases onto that ref, and deletes the ref whatever happens. A rebase that
//! stops on conflicts stays in progress, as in Orca: the person sends the conflicts to the agent
//! with [`resolve_prompt`], or aborts the rebase.

use std::path::PathBuf;

use crate::domain::branch::{Branch, StartPoint};

/// The prefix of the temporary refs a rebase fetches its base into, one per rebase.
pub(crate) const TEMP_REF_PREFIX: &str = "refs/orca-term/rebase/";

/// The temporary ref the rebase of the worktree whose git id is `worktree_id` fetches its base
/// into: `refs/orca-term/rebase/<id>-<nonce>`, where `nonce` (the time, in milliseconds) keeps two
/// rebases apart.
pub(crate) fn temp_ref(worktree_id: &str, nonce: u64) -> String {
    format!("{TEMP_REF_PREFIX}{worktree_id}-{nonce}")
}

/// The base a worktree is rebased onto: the default branch `branch` on `origin`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RemoteBase {
    pub(crate) branch: Branch,
}

impl RemoteBase {
    /// How the base is shown: `origin/main`.
    pub(crate) fn shown(&self) -> String {
        StartPoint::Remote(self.branch.clone()).to_string()
    }

    /// The local copy git keeps of the base: `refs/remotes/origin/main`. Its reflog is where the
    /// fork point is looked for.
    pub(crate) fn tracking_ref(&self) -> String {
        format!("refs/remotes/origin/{}", self.branch)
    }

    /// git's refspec that fetches the base into `temp`, replacing whatever `temp` held:
    /// `+refs/heads/main:<temp>`.
    pub(crate) fn refspec_into(&self, temp: &str) -> String {
        format!("+refs/heads/{}:{temp}", self.branch)
    }
}

/// What a rebase that went through did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// The branch was rebased, `replayed` commits of its own now on top of the base.
    Rebased { replayed: usize },
    /// The branch already had every commit of the base: nothing was done.
    UpToDate,
}

impl Outcome {
    /// How `--json` names it: `rebased` or `up_to_date`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Rebased { .. } => "rebased",
            Self::UpToDate => "up_to_date",
        }
    }
}

/// What `worktree rebase` reports when the rebase went through.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Rebased {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    /// The base, as shown: `origin/main`.
    pub(crate) base: String,
    pub(crate) outcome: Outcome,
}

impl Rebased {
    /// The line the text output prints: `demo: rebased onto origin/main (3 commits replayed)`, or
    /// `demo: already up to date with origin/main`.
    pub(crate) fn line(&self) -> String {
        match self.outcome {
            Outcome::Rebased { replayed } => format!(
                "{}: rebased onto {} ({})",
                self.name,
                self.base,
                count(replayed, "commit replayed", "commits replayed")
            ),
            Outcome::UpToDate => format!("{}: already up to date with {}", self.name, self.base),
        }
    }
}

/// What `worktree rebase --abort` reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Aborted {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
}

/// What `agent resolve` reports: the prompt typed into the agent, and the files in conflict it
/// names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ResolveSent {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) prompt: String,
    pub(crate) files: Vec<String>,
}

/// `n` with the word for one or for several: `1 commit replayed`, `3 commits replayed`.
fn count(n: usize, one: &str, several: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {several}")
    }
}

/// The paths in git's NUL-separated output `raw` (`git diff --name-only -z`), in order.
pub(crate) fn paths(raw: &[u8]) -> Vec<String> {
    raw.split(|&byte| byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| String::from_utf8_lossy(path).into_owned())
        .collect()
}

/// The files with changes not committed in what `git status --porcelain -z --untracked-files=all`
/// printed, untracked ones included, each once: a rename or a copy names its new path, and its
/// old path, the field after it, is skipped.
pub(crate) fn uncommitted_files(porcelain: &[u8]) -> Vec<String> {
    let mut files = Vec::new();
    let mut fields = porcelain
        .split(|&byte| byte == 0)
        .filter(|field| !field.is_empty());
    while let Some(entry) = fields.next() {
        let status = entry.get(..2).unwrap_or_default();
        if let Some(path) = entry.get(3..) {
            files.push(String::from_utf8_lossy(path).into_owned());
        }
        if status.contains(&b'R') || status.contains(&b'C') {
            fields.next();
        }
    }
    files
}

/// The prompt `agent resolve` types into the worktree's agent: the files in conflict (`files`),
/// what to do with them, and what never to do, as Orca's conflict prompt says it.
pub(crate) fn resolve_prompt(files: &[String]) -> String {
    let listed = if files.is_empty() {
        "- (none is left in conflict; check `git status`)".to_owned()
    } else {
        files
            .iter()
            .map(|file| format!("- {file}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    format!(
        "A rebase of this worktree's branch stopped on conflicts. Resolve them.\n\
         \n\
         Files in conflict:\n\
         {listed}\n\
         \n\
         1. Start from `git status`.\n\
         2. Resolve each file, keeping the intent of both sides; do not take ours or theirs \
         wholesale.\n\
         3. `git add` each file once it is resolved.\n\
         4. Run `GIT_EDITOR=true git rebase --continue`. If it stops on new conflicts, resolve \
         them the same way, until the rebase is done.\n\
         \n\
         Never run `git rebase --abort`, `git reset`, `git stash` or `git push`, and make no \
         other commits. When you are done, say how you resolved each file."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rebased(outcome: Outcome) -> Rebased {
        Rebased {
            name: "demo".to_owned(),
            path: PathBuf::from("/w/demo"),
            base: "origin/main".to_owned(),
            outcome,
        }
    }

    #[test]
    fn the_temp_ref_is_private_and_unique_per_rebase() {
        assert_eq!(temp_ref("demo", 42), "refs/orca-term/rebase/demo-42");
    }

    #[test]
    fn the_base_is_the_default_branch_on_origin() {
        let base = RemoteBase {
            branch: Branch::new("main"),
        };
        assert_eq!(base.shown(), "origin/main");
        assert_eq!(base.tracking_ref(), "refs/remotes/origin/main");
        assert_eq!(
            base.refspec_into("refs/orca-term/rebase/x"),
            "+refs/heads/main:refs/orca-term/rebase/x"
        );
    }

    #[test]
    fn the_line_says_what_was_replayed_or_that_nothing_was_done() {
        assert_eq!(
            rebased(Outcome::Rebased { replayed: 3 }).line(),
            "demo: rebased onto origin/main (3 commits replayed)"
        );
        assert_eq!(
            rebased(Outcome::Rebased { replayed: 1 }).line(),
            "demo: rebased onto origin/main (1 commit replayed)"
        );
        assert_eq!(
            rebased(Outcome::UpToDate).line(),
            "demo: already up to date with origin/main"
        );
        assert_eq!(Outcome::UpToDate.name(), "up_to_date");
        assert_eq!(Outcome::Rebased { replayed: 0 }.name(), "rebased");
    }

    #[test]
    fn uncommitted_files_are_named_once_each_renames_by_their_new_path() {
        let status = b" M a.txt\0?? new dir/b.txt\0R  c.txt\0old.txt\0A  d.txt\0";
        assert_eq!(
            uncommitted_files(status),
            ["a.txt", "new dir/b.txt", "c.txt", "d.txt"]
        );
        assert_eq!(uncommitted_files(b""), Vec::<String>::new());
    }

    #[test]
    fn paths_are_split_on_nul() {
        assert_eq!(paths(b"a.txt\0b c.txt\0"), ["a.txt", "b c.txt"]);
        assert_eq!(paths(b""), Vec::<String>::new());
    }

    #[test]
    fn the_resolve_prompt_lists_the_files_and_forbids_abort_reset_stash_and_push() {
        let prompt = resolve_prompt(&["a.txt".to_owned(), "src/b.rs".to_owned()]);
        assert!(
            prompt.contains("Files in conflict:\n- a.txt\n- src/b.rs\n"),
            "{prompt}"
        );
        assert!(prompt.contains("`git add`"), "{prompt}");
        assert!(prompt.contains("git rebase --continue"), "{prompt}");
        assert!(
            prompt
                .contains("Never run `git rebase --abort`, `git reset`, `git stash` or `git push`"),
            "{prompt}"
        );
        let none = resolve_prompt(&[]);
        assert!(none.contains("none is left in conflict"), "{none}");
    }
}
