//! Builds a repository holding the frozen samples of a state schema (#118), from
//! `tests/fixtures/schema-<n>/`, so a suite can check that the current binary still reads them.
//!
//! A sample directory mirrors where the core keeps its state:
//!
//! - `checkout/`: files committed in the primary checkout before the worktree is added, which the
//!   samples depend on (the `orca-term.yaml` whose setup script and writable list the approvals
//!   cover, the file the notes are on).
//! - `git/`: the repository's `.git` directory (git refuses a directory named `.git` in a tree):
//!   `git/orca-term-trust.json` is the approvals' file, and `git/worktrees/demo/orca-term/` the
//!   admin directory of the worktree `demo`.
//!
//! The samples were written by the real binary against the fake agents, then their local paths
//! replaced by placeholders, which the builder fills back in: `{{transcripts}}` is where claude's
//! transcripts were.

use std::fs;
use std::path::{Path, PathBuf};

use crate::fixture::AgentRepo;

/// The name of the worktree the samples belong to.
pub(crate) const WORKTREE: &str = "demo";

/// A repository holding a schema's samples, and the files the builder wrote.
pub(crate) struct Sampled {
    /// The worktree `demo`.
    pub(crate) worktree: PathBuf,
    /// Every state file copied from the samples, with the bytes written to it.
    pub(crate) files: Vec<(PathBuf, Vec<u8>)>,
}

impl Sampled {
    /// The files whose bytes are no longer the ones the builder wrote.
    pub(crate) fn changed_files(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .filter(|(path, bytes)| fs::read(path).ok().as_ref() != Some(bytes))
            .map(|(path, _)| path.clone())
            .collect()
    }
}

/// The directory of the samples of `schema`.
pub(crate) fn samples_dir(schema: u64) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(format!("schema-{schema}"))
}

/// Copies the samples of `schema` into `fx`'s repository: `base:` set in `.git/orca-term.yaml`
/// to a directory of the fixture, the checkout's files committed, the worktree `demo` added with
/// plain git under that base, then every state file copied where the core reads it.
pub(crate) fn build(fx: &AgentRepo, schema: u64) -> Sampled {
    let samples = samples_dir(schema);
    let tmp = fx.repo.tmp.path();
    let base = tmp.join("worktrees");
    fx.repo.write(
        ".git/orca-term.yaml",
        &format!("base: {}\n", base.display()),
    );
    copy_tree(&samples.join("checkout"), &fx.repo.root, &[]);
    fx.repo.commit_all("the files the samples depend on");
    let worktree_dir = base.join("repo").join(WORKTREE);
    fs::create_dir_all(worktree_dir.parent().unwrap()).unwrap();
    let worktree = fx.repo.add_worktree(&worktree_dir, WORKTREE);
    let transcripts = tmp.join("transcripts");
    let placeholders = [("{{transcripts}}", transcripts.to_str().unwrap())];
    let git_dir = fx.repo.root.join(".git");
    let files = copy_tree(&samples.join("git"), &git_dir, &placeholders);
    Sampled { worktree, files }
}

/// Copies every file under `from` to the same place under `to`, each placeholder replaced by its
/// value, and returns the files written with their bytes.
fn copy_tree(from: &Path, to: &Path, placeholders: &[(&str, &str)]) -> Vec<(PathBuf, Vec<u8>)> {
    let mut written = Vec::new();
    for entry in fs::read_dir(from).unwrap() {
        let path = entry.unwrap().path();
        let target = to.join(path.file_name().unwrap());
        if path.is_dir() {
            fs::create_dir_all(&target).unwrap();
            written.extend(copy_tree(&path, &target, placeholders));
            continue;
        }
        let mut text = fs::read_to_string(&path).unwrap();
        for (placeholder, value) in placeholders {
            text = text.replace(placeholder, value);
        }
        fs::write(&target, &text).unwrap();
        written.push((target, text.into_bytes()));
    }
    written
}
