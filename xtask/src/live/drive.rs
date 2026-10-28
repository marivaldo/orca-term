//! Runs one scenario against the real agent, through the real orca-term, in a throwaway repository
//! of its own under the system's temporary directory:
//!
//! 1. a repository with one empty commit, its git config its own (as the test fixtures do, so the
//!    person's signing and hooks stay out of it), and an `orca-term.yaml` override in its git
//!    common dir putting the worktrees beside it, never under the person's home;
//! 2. `orca-term worktree new live`, the scenario's `file`s written into it, then `orca-term agent
//!    start live` with `ORCA_TERM_RECORD` naming the tap's file, so every hook claude runs, or
//!    every line opencode's plugin writes, is appended to it as received;
//! 3. `orca-term agent attach live` on a pseudo-terminal, which types the scenario's keys and waits
//!    for the hooks it names (or this terminal, for a scenario recorded by hand);
//! 4. `orca-term agent stop live`, then the files that must be missing, then the tap's file,
//!    normalised.
//!
//! The directory is removed once the recording is made, and kept for a look when it fails, with the
//! attach's screen in `screen.log` and what was recorded in `tap.log`.

use std::fs;
use std::io::{Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use serde_json::Value;

use super::recording;
use super::scenario::{Agent, Scenario, Step};

/// The worktree every scenario makes.
const WORKTREE: &str = "live";

/// The size of the pseudo-terminal the attach runs on, which the agent's screen takes.
const ROWS: u16 = 40;
const COLS: u16 = 120;

/// The pause after each typed text and key, so that each reaches the agent as input of its own.
const PAUSE: Duration = Duration::from_millis(300);

/// How often a `wait-for` reads the tap's file.
const POLL: Duration = Duration::from_millis(100);

/// Runs `scenario` with the orca-term at `orca_term` and returns its recording, normalised, or why
/// it failed. A scenario recorded by hand attaches this terminal instead of typing its steps.
pub(crate) fn record(orca_term: &Path, scenario: &Scenario) -> Result<String, String> {
    let place = Place::make(&scenario.name).map_err(|err| format!("{err:#}"))?;
    match record_in(&place, orca_term, scenario) {
        Ok(text) => {
            place.remove();
            Ok(text)
        }
        Err(err) => Err(format!("{err:#} (kept {})", place.root.display())),
    }
}

/// The steps of [`record`], inside `place`.
fn record_in(place: &Place, orca_term: &Path, scenario: &Scenario) -> Result<String> {
    place.make_repository()?;
    let worktree = place.new_worktree(orca_term)?;
    write_files(&worktree, &scenario.files)?;
    let denied = place.admin_dir(&worktree)?;
    let worktree = path_text(&worktree)?;
    let denied = path_text(&denied)?;
    let agent = Running::start(place, orca_term, scenario.agent)?;
    if scenario.manual {
        place.attach_here(orca_term)?;
    } else {
        let attach = Attached::start(place, orca_term)?;
        type_steps(place, scenario, &attach, worktree, denied)?;
    }
    agent.stop()?;
    for file in &scenario.check_missing {
        let file = recording::expand(file, worktree, denied);
        if Path::new(&file).symlink_metadata().is_ok() {
            bail!("{file} exists: the sandbox let the agent write it");
        }
    }
    let tapped = fs::read_to_string(place.tap()).unwrap_or_default();
    if tapped.trim().is_empty() {
        bail!("the agent sent nothing to orca-term's hooks");
    }
    let transcripts = recording::transcripts_dir(&tapped).unwrap_or_default();
    let tasks = recording::tasks_dir(&tapped).unwrap_or_default();
    let placeholders = recording::placeholders(&[
        (worktree, "{{worktree}}"),
        (denied, "{{denied}}"),
        (&transcripts, "{{transcripts}}"),
        (&tasks, "{{tasks}}"),
    ]);
    Ok(recording::normalise(&tapped, &placeholders))
}

/// Writes the scenario's `files` into `worktree`, each at its path there, before the agent starts.
fn write_files(worktree: &Path, files: &[(String, String)]) -> Result<()> {
    for (path, text) in files {
        let file = worktree.join(path);
        if let Some(dir) = file.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&file, format!("{text}\n"))
            .with_context(|| format!("could not write {}", file.display()))?;
    }
    Ok(())
}

/// Types the steps of `scenario` through `attach`, waiting for what they wait for, all within the
/// scenario's timeout.
fn type_steps(
    place: &Place,
    scenario: &Scenario,
    attach: &Attached,
    worktree: &str,
    denied: &str,
) -> Result<()> {
    let deadline = Instant::now() + scenario.timeout;
    let mut waited: Vec<String> = Vec::new();
    for step in &scenario.steps {
        match step {
            Step::Sleep(pause) => thread::sleep(*pause),
            Step::Type(text) => {
                attach.send(recording::expand(text, worktree, denied).as_bytes())?;
            }
            Step::Press(key) => attach.send(key.bytes())?,
            Step::WaitFor(kind) => {
                waited.push(kind.clone());
                let nth = recording::count(&waited, kind);
                wait_for(place, scenario, attach, kind, nth, deadline)?;
            }
        }
    }
    Ok(())
}

/// Waits until the tap's file holds `nth` lines of `kind`; fails once the attach ended or the
/// scenario's time is up.
fn wait_for(
    place: &Place,
    scenario: &Scenario,
    attach: &Attached,
    kind: &str,
    nth: usize,
    deadline: Instant,
) -> Result<()> {
    loop {
        let tapped = fs::read_to_string(place.tap()).unwrap_or_default();
        if recording::count(&recording::kinds(scenario.agent, &tapped), kind) >= nth {
            return Ok(());
        }
        if attach.ended() {
            bail!("the attach ended while waiting for {kind}");
        }
        if Instant::now() >= deadline {
            bail!(
                "timed out after {} s waiting for {kind}",
                scenario.timeout.as_secs()
            );
        }
        thread::sleep(POLL);
    }
}

/// The scenario's throwaway directory and what is in it.
struct Place {
    root: PathBuf,
}

impl Place {
    /// Makes `<temporary directory>/orca-term-live-<scenario>-<pid>`, by its real path.
    fn make(scenario: &str) -> Result<Self> {
        let base = std::env::temp_dir();
        let root = base.join(format!("orca-term-live-{scenario}-{}", std::process::id()));
        fs::create_dir(&root).with_context(|| format!("could not make {}", root.display()))?;
        let root = root
            .canonicalize()
            .context("could not resolve the temporary directory")?;
        Ok(Self { root })
    }

    fn repo(&self) -> PathBuf {
        self.root.join("repo")
    }

    fn tap(&self) -> PathBuf {
        self.root.join("tap.log")
    }

    fn screen(&self) -> PathBuf {
        self.root.join("screen.log")
    }

    fn gitconfig(&self) -> PathBuf {
        self.root.join("gitconfig")
    }

    /// `program` to run in `dir` with the scenario's own git config and no input.
    fn command(&self, program: &Path, dir: &Path) -> Command {
        let mut command = Command::new(program);
        command
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", self.gitconfig())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(Stdio::null());
        command
    }

    /// Runs `program` with `args` in `dir` and returns its stdout, or fails with its stderr.
    fn run(&self, program: &Path, dir: &Path, args: &[&str]) -> Result<String> {
        let out = self
            .command(program, dir)
            .args(args)
            .output()
            .with_context(|| format!("could not run {}", program.display()))?;
        if !out.status.success() {
            bail!(
                "`{} {}` failed: {}",
                program.file_name().unwrap_or_default().to_string_lossy(),
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// The repository, its first commit, and the override that puts the worktrees beside it.
    fn make_repository(&self) -> Result<()> {
        fs::write(
            self.gitconfig(),
            "[user]\n\tname = orca-term live check\n\temail = live-check@example.invalid\n\
             [init]\n\tdefaultBranch = main\n",
        )?;
        let git = Path::new("git");
        let repo = self.repo();
        self.run(git, &self.root, &["init", "--quiet", "repo"])?;
        self.run(
            git,
            &repo,
            &["commit", "--quiet", "--allow-empty", "-m", "init"],
        )?;
        let base = self.root.join("worktrees");
        let base = Value::String(path_text(&base)?.to_owned());
        fs::write(repo.join(".git/orca-term.yaml"), format!("base: {base}\n"))?;
        Ok(())
    }

    /// `orca-term worktree new live`, and the worktree's path it reports.
    fn new_worktree(&self, orca_term: &Path) -> Result<PathBuf> {
        let out = self.run(
            orca_term,
            &self.repo(),
            &["worktree", "new", WORKTREE, "--json"],
        )?;
        let doc: Value = serde_json::from_str(&out).context("worktree new printed no JSON")?;
        let path = doc["worktree"]["path"]
            .as_str()
            .context("worktree new reported no path")?;
        Ok(PathBuf::from(path))
    }

    /// The worktree's admin directory, which both agents' sandboxes deny: `{{denied}}`.
    fn admin_dir(&self, worktree: &Path) -> Result<PathBuf> {
        let git = Path::new("git");
        let args = [
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "orca-term",
        ];
        Ok(PathBuf::from(self.run(git, worktree, &args)?.trim()))
    }

    /// Attaches this terminal to the agent, for a scenario recorded by hand, until the person
    /// detaches or the agent ends.
    fn attach_here(&self, orca_term: &Path) -> Result<()> {
        self.command(orca_term, &self.repo())
            .args(["agent", "attach", WORKTREE])
            .stdin(Stdio::inherit())
            .status()
            .context("could not attach to the agent")?;
        Ok(())
    }

    /// Removes the directory, all of it made here.
    fn remove(&self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// The agent of the scenario's worktree, started with the tap on; stopped when dropped, so that a
/// failing scenario leaves no agent behind.
struct Running<'a> {
    place: &'a Place,
    orca_term: &'a Path,
    stopped: bool,
}

impl<'a> Running<'a> {
    fn start(place: &'a Place, orca_term: &'a Path, agent: Agent) -> Result<Self> {
        let out = place
            .command(orca_term, &place.repo())
            .args(["agent", "start", WORKTREE, "--agent", agent.name()])
            .env("ORCA_TERM_RECORD", place.tap())
            .output()
            .context("could not run orca-term agent start")?;
        if !out.status.success() {
            bail!(
                "agent start failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(Self {
            place,
            orca_term,
            stopped: false,
        })
    }

    /// `orca-term agent stop live`, which returns once the agent and its holder are gone, the
    /// agent's last hooks run.
    fn stop(mut self) -> Result<()> {
        self.stopped = true;
        self.place
            .run(
                self.orca_term,
                &self.place.repo(),
                &["agent", "stop", WORKTREE],
            )
            .map(drop)
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        if !self.stopped {
            let _ = self.place.run(
                self.orca_term,
                &self.place.repo(),
                &["agent", "stop", WORKTREE],
            );
        }
    }
}

/// `orca-term agent attach live` on a pseudo-terminal, its screen collected as it comes. Killed
/// when dropped, its screen written to `screen.log`.
struct Attached {
    child: Mutex<Child>,
    pty: Arc<pty_process::blocking::Pty>,
    screen: Arc<Mutex<Vec<u8>>>,
    screen_file: PathBuf,
}

impl Attached {
    fn start(place: &Place, orca_term: &Path) -> Result<Self> {
        let (pty, pts) =
            pty_process::blocking::open().context("could not open a pseudo-terminal")?;
        pty.resize(pty_process::Size::new(ROWS, COLS))
            .context("could not size the pseudo-terminal")?;
        let child = pty_process::blocking::Command::new(orca_term)
            .args(["agent", "attach", WORKTREE])
            .current_dir(place.repo())
            .env("GIT_CONFIG_GLOBAL", place.gitconfig())
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .spawn(pts)
            .context("could not start orca-term agent attach")?;
        let pty = Arc::new(pty);
        let screen = Arc::new(Mutex::new(Vec::new()));
        let (reader, sink) = (Arc::clone(&pty), Arc::clone(&screen));
        // The thread ends when the read fails; on macOS it may wait for the process to exit.
        thread::spawn(move || copy_screen(&reader, &sink));
        Ok(Self {
            child: Mutex::new(child),
            pty,
            screen,
            screen_file: place.screen(),
        })
    }

    /// Types `bytes`, then pauses.
    fn send(&self, bytes: &[u8]) -> Result<()> {
        (&*self.pty)
            .write_all(bytes)
            .context("could not type into the attach")?;
        thread::sleep(PAUSE);
        Ok(())
    }

    /// Whether the attach has ended.
    fn ended(&self) -> bool {
        self.child
            .lock()
            .map_or(true, |mut child| !matches!(child.try_wait(), Ok(None)))
    }
}

impl Drop for Attached {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Ok(screen) = self.screen.lock() {
            let _ = fs::write(&self.screen_file, &*screen);
        }
    }
}

/// Copies what the attach draws on `pty` into `screen`, until the read fails or ends.
fn copy_screen(pty: &pty_process::blocking::Pty, screen: &Mutex<Vec<u8>>) {
    let mut chunk = [0u8; 4096];
    let mut reader = pty;
    while let Ok(n) = reader.read(&mut chunk) {
        if n == 0 {
            break;
        }
        if let Ok(mut screen) = screen.lock() {
            screen.extend_from_slice(&chunk[..n]);
        }
    }
}

/// `path` as text, which every path here is.
fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .with_context(|| format!("{} is not valid UTF-8", path.display()))
}
