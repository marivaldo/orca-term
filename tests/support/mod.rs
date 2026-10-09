//! Throwaway git repositories for black-box tests of the core.
//!
//! Every git process, ours and the binary's, runs against a gitconfig of the fixture's own, so
//! tests never read the developer's global config and behave the same on CI. The binary also gets
//! a `HOME` and an `XDG_CONFIG_HOME` inside the fixture, so it never reads the developer's global
//! orca-term config nor places worktrees under the developer's home.

use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::cargo::cargo_bin_cmd;
use assert_fs::TempDir;
use assert_fs::prelude::*;

#[derive(Debug)]
pub(crate) struct Repo {
    pub(crate) tmp: TempDir,
    pub(crate) root: PathBuf,
    /// The binary's `HOME`.
    pub(crate) home: PathBuf,
    /// The binary's `XDG_CONFIG_HOME`.
    pub(crate) xdg: PathBuf,
    gitconfig: PathBuf,
}

impl Repo {
    /// A repository with one commit on `main`, at `<tmp>/repo`.
    pub(crate) fn new() -> Self {
        let tmp = TempDir::new().unwrap();
        let gitconfig = tmp.child("gitconfig");
        gitconfig
            .write_str("[user]\n\tname = Fixture\n\temail = fixture@example.invalid\n[init]\n\tdefaultBranch = main\n")
            .unwrap();
        let root = tmp.child("repo");
        root.create_dir_all().unwrap();
        let home = tmp.child("home");
        home.create_dir_all().unwrap();
        let xdg = tmp.child("xdg");
        xdg.create_dir_all().unwrap();
        let repo = Self {
            root: root.path().canonicalize().unwrap(),
            home: home.path().canonicalize().unwrap(),
            xdg: xdg.path().canonicalize().unwrap(),
            gitconfig: gitconfig.path().to_owned(),
            tmp,
        };
        repo.git(&["init", "--quiet"]);
        repo.git(&["commit", "--quiet", "--allow-empty", "-m", "init"]);
        repo
    }

    /// Runs git in the primary checkout and panics on failure.
    pub(crate) fn git(&self, args: &[&str]) -> String {
        let out = self
            .env(Command::new("git").arg("-C").arg(&self.root).args(args))
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).unwrap()
    }

    /// Writes `contents` to `rel` under the primary checkout, creating parent directories.
    pub(crate) fn write(&self, rel: &str, contents: &str) -> PathBuf {
        let path = self.root.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, contents).unwrap();
        path
    }

    /// Stages everything and commits it.
    pub(crate) fn commit_all(&self, message: &str) {
        self.git(&["add", "--all"]);
        self.git(&["commit", "--quiet", "-m", message]);
    }

    /// Adds a worktree by hand, as a person would with plain git, on a new branch.
    pub(crate) fn add_worktree(&self, path: &Path, branch: &str) -> PathBuf {
        self.git(&[
            "worktree",
            "add",
            "--quiet",
            "-b",
            branch,
            path.to_str().unwrap(),
        ]);
        path.canonicalize().unwrap()
    }

    /// The `orca-term` binary, run from `dir` with the fixture's git environment.
    pub(crate) fn orca_term(&self, dir: &Path) -> assert_cmd::Command {
        let mut cmd = cargo_bin_cmd!("orca-term");
        cmd.current_dir(dir);
        self.env_assert(&mut cmd);
        cmd
    }

    fn env<'a>(&self, cmd: &'a mut Command) -> &'a mut Command {
        cmd.env("GIT_CONFIG_GLOBAL", &self.gitconfig)
            .env("GIT_CONFIG_NOSYSTEM", "1")
    }

    fn env_assert(&self, cmd: &mut assert_cmd::Command) {
        cmd.env("GIT_CONFIG_GLOBAL", &self.gitconfig)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.xdg);
    }
}
