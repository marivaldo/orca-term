//! A changed file's diff opened in a real Neovim through the core (#194): `nvim diff <worktree>
//! <file>`, which is what the application does on enter on a changed file, asks every registered
//! Neovim to open the file beside its version at the merge base. The Neovim here is the real
//! `nvim --clean --headless --listen <socket>`: no plugin and no config, so everything that
//! opens the diff is the Lua the core sends (`src/domain/nvim/open_diff.lua`).
//!
//! The test asks each Neovim what it shows with `nvim --server <socket> --remote-expr`, and reads
//! the answer as JSON.
//!
//! Without `nvim` on PATH the test says so and skips; with `ORCA_TERM_REQUIRE_NVIM` set, as CI
//! sets it on Linux, a missing `nvim` fails it instead.

#![expect(
    clippy::unwrap_used,
    reason = "clippy.toml relaxes unwrap only inside #[test] fns; fixture helpers panic on failure too"
)]
#![expect(
    clippy::disallowed_methods,
    clippy::disallowed_types,
    reason = "a black-box test builds real repositories and files around the binary it drives"
)]

#[expect(
    dead_code,
    reason = "the fixture is shared between suites, and this one leaves part of it unused"
)]
mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;
use support::Repo;

/// How long a test waits for Neovim to start, or to show what it was asked.
const WAIT: Duration = Duration::from_secs(30);

/// What a Neovim is asked, to learn what it shows: how many tabs it has, which window of the
/// current tab is current, and each window of that tab, left to right, with its buffer's name,
/// its buffer's full path, whether it is in diff mode and its lines.
const SHOWN: &str = "json_encode(#{tabs: tabpagenr('$'), current: winnr(), \
    windows: map(range(1, winnr('$')), {_, n -> #{\
    name: bufname(winbufnr(n)), \
    path: fnamemodify(bufname(winbufnr(n)), ':p'), \
    diff: getwinvar(n, '&diff'), \
    lines: getbufline(winbufnr(n), 1, '$')}})})";

/// Whether a real `nvim` can be run here. Fails instead of answering no when
/// `ORCA_TERM_REQUIRE_NVIM` is set.
fn nvim_is_installed() -> bool {
    let found = Command::new("nvim")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success());
    if !found {
        assert!(
            std::env::var_os("ORCA_TERM_REQUIRE_NVIM").is_none(),
            "ORCA_TERM_REQUIRE_NVIM is set, and nvim is not on PATH"
        );
    }
    found
}

/// A real headless Neovim with no config and no plugin, listening on a socket of its own; killed,
/// and its socket's directory removed, when dropped.
struct Neovim {
    child: Child,
    dir: PathBuf,
}

impl Neovim {
    /// Starts one in `cwd`, as the application starts a worktree's Neovim in the worktree, and
    /// waits until it listens. Its home and its XDG directories are the fixture's.
    fn start_in(cwd: &Path, repo: &Repo) -> Self {
        let dir = short_dir();
        let child = Command::new("nvim")
            .args(["--clean", "--headless", "--listen"])
            .arg(dir.join("nvim.sock"))
            .current_dir(cwd)
            .env("HOME", &repo.home)
            .env("XDG_CONFIG_HOME", &repo.xdg)
            .env("XDG_STATE_HOME", repo.home.join("state"))
            .env("XDG_DATA_HOME", repo.home.join("data"))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let neovim = Self { child, dir };
        let start = Instant::now();
        while !neovim.socket().exists() {
            assert!(start.elapsed() < WAIT, "Neovim never listened");
            thread::sleep(Duration::from_millis(20));
        }
        neovim
    }

    /// Where it listens: its server address.
    fn socket(&self) -> PathBuf {
        self.dir.join("nvim.sock")
    }

    /// What it shows now ([`SHOWN`]).
    fn shown(&self) -> Value {
        let out = Command::new("nvim")
            .arg("--server")
            .arg(self.socket())
            .args(["--remote-expr", SHOWN])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        serde_json::from_slice(&out.stdout).unwrap()
    }

    /// Waits until what it shows satisfies `wanted`, and returns it.
    fn wait_until_shown(&self, what: &str, wanted: impl Fn(&Value) -> bool) -> Value {
        let start = Instant::now();
        loop {
            let shown = self.shown();
            if wanted(&shown) {
                return shown;
            }
            assert!(start.elapsed() < WAIT, "never showed {what}: {shown}");
            thread::sleep(Duration::from_millis(50));
        }
    }
}

impl Drop for Neovim {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_dir_all(&self.dir);
    }
}

/// A fresh, short directory under `/tmp`, so a socket in it fits a unix socket address.
fn short_dir() -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::SeqCst);
    let dir = PathBuf::from(format!("/tmp/ot-diff-{}-{n}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// Runs the binary from the primary checkout with `args`, which must succeed.
fn orca_term(repo: &Repo, args: &[&str]) {
    repo.orca_term(&repo.root).args(args).assert().success();
}

/// The lines of the window `index` (from the left, from 0) of what a Neovim shows.
fn lines_of(shown: &Value, index: usize) -> Vec<&str> {
    shown["windows"][index]["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line.as_str().unwrap())
        .collect()
}

/// Whether the current tab of what a Neovim shows is the diff of the file at `path`: two windows
/// in diff mode, the file on the right with the cursor in it.
fn is_diff_of(shown: &Value, path: &Path) -> bool {
    let windows = shown["windows"].as_array().unwrap();
    windows.len() == 2
        && windows.iter().all(|window| window["diff"] == 1)
        && windows[1]["path"] == path.to_str().unwrap()
        && shown["current"] == 2
}

#[test]
fn a_changed_files_diff_opens_in_the_worktrees_neovim_beside_its_base_with_no_plugin() {
    if !nvim_is_installed() {
        eprintln!("skipped: nvim is not on PATH");
        return;
    }
    let repo = Repo::new();
    repo.write("src/a.txt", "one\ntwo\nthree\n");
    repo.commit_all("a");
    let worktree = repo.add_worktree(&repo.tmp.path().join("fix"), "fix");
    // Changed in a commit of the worktree and again on disk: the diff is against the merge base.
    fs::write(worktree.join("src/a.txt"), "one\n2\nthree\n").unwrap();
    let at = worktree.to_str().unwrap();
    repo.git(&["-C", at, "commit", "--quiet", "--all", "-m", "two"]);
    fs::write(worktree.join("src/a.txt"), "one\n2\nthree\nfour\n").unwrap();
    fs::write(worktree.join("new.txt"), "new\n").unwrap();
    let base = repo.git(&["rev-parse", "--short=7", "main"]);

    // One Neovim works in the worktree; the other, in the primary checkout, does not.
    let inside = Neovim::start_in(&worktree, &repo);
    let outside = Neovim::start_in(&repo.root, &repo);
    for neovim in [&inside, &outside] {
        orca_term(
            &repo,
            &["nvim", "register", neovim.socket().to_str().unwrap()],
        );
    }

    orca_term(&repo, &["nvim", "diff", "fix", "src/a.txt"]);
    let file = worktree.join("src/a.txt");
    let shown = inside.wait_until_shown("the diff", |shown| is_diff_of(shown, &file));
    assert_eq!(shown["tabs"], 2, "the diff has a tab of its own: {shown}");
    assert_eq!(
        shown["windows"][0]["name"],
        format!("orca-term://{}/src/a.txt", base.trim()),
        "{shown}"
    );
    assert_eq!(lines_of(&shown, 0), ["one", "two", "three"], "the base");
    assert_eq!(
        lines_of(&shown, 1),
        ["one", "2", "three", "four"],
        "on disk"
    );
    assert_eq!(outside.shown()["tabs"], 1, "another worktree's Neovim");

    // A new file has nothing at the merge base: an empty window beside it.
    orca_term(&repo, &["nvim", "diff", "fix", "new.txt"]);
    let new_file = worktree.join("new.txt");
    let shown = inside.wait_until_shown("the new file", |shown| is_diff_of(shown, &new_file));
    assert_eq!(shown["tabs"], 3, "{shown}");
    assert_eq!(lines_of(&shown, 0), [""], "nothing at the base");
    assert_eq!(lines_of(&shown, 1), ["new"]);

    // The same file asked for again: its diff is opened again, in place of the one it had.
    orca_term(&repo, &["nvim", "diff", "fix", "src/a.txt"]);
    let shown = inside.wait_until_shown("the first diff again", |shown| is_diff_of(shown, &file));
    assert_eq!(shown["tabs"], 3, "one diff per file: {shown}");
    assert_eq!(lines_of(&shown, 0), ["one", "two", "three"]);
    assert_eq!(outside.shown()["tabs"], 1);
}
