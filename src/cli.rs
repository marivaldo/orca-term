//! Edge: the command-line surface. It parses the arguments, hands each command to its ops module
//! and gives the result to `output`.

use std::panic;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::adapters::holder::HolderArgs;
use crate::adapters::stdin;
use crate::adapters::terminal::{self, DetachKey};
use crate::domain::agent::{Agent, Size, StartMode};
use crate::domain::note::{Anchor, LineRange};
use crate::domain::nvim::Address;
use crate::domain::seen::Mark;
use crate::domain::setup::SetupReport;
use crate::domain::state::AdminDir;
use crate::domain::trust::{self, Unapproved};
use crate::domain::worktree::{Removal, Target, WorktreeName};
use crate::ops::{
    Approval, agent_attach, agent_holder, agent_start, agent_stop, agent_swap, check_git, doctor,
    hook, note_add, note_edit, note_ls, note_rm, note_send, nvim_register, nvim_unregister,
    pr_refresh, session_ls, session_new, session_resume, turn_blame, turn_diff, turn_ls,
    worktree_detail, worktree_ls, worktree_new, worktree_prune, worktree_review, worktree_rm,
    worktree_seen, worktree_setup,
};
use crate::output::{self, Format};

#[derive(Debug, Parser)]
#[command(
    name = "orca-term",
    version,
    about = "A fleet of parallel CLI coding agents"
)]
pub struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Work with the worktrees of the current repository.
    #[command(visible_alias = "wt")]
    Worktree {
        #[command(subcommand)]
        command: WorktreeCommand,
    },
    /// Run a worktree's agent: start it in a detached holder, attach a terminal to it, stop it.
    Agent {
        #[command(subcommand)]
        command: AgentCommand,
    },
    /// Attach notes to a worktree's files, then send them to its agent as one prompt.
    Note {
        #[command(subcommand)]
        command: NoteCommand,
    },
    /// Read a worktree's turns: list them, show what one changed, say which turn wrote each line.
    Turn {
        #[command(subcommand)]
        command: TurnCommand,
    },
    /// Work with a worktree's sessions: list them, make one current again, start a new one.
    Session {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// Read each worktree's pull request on GitHub, and its checks, through the GitHub CLI (`gh`).
    Pr {
        #[command(subcommand)]
        command: PrCommand,
    },
    /// Check every program orca-term needs (git, claude's sandbox, the agents) and say how to fix
    /// what is missing; installs nothing. Works outside a repository.
    Doctor {
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
    /// What the agent's hooks run: `hook claude --admin <dir> <event>`, the event's payload on
    /// stdin, or `hook opencode --admin <dir>`, which the holder runs for each line opencode's
    /// plugin writes. Its arguments are taken as they come, unchecked, so that clap never refuses
    /// them (clap exits 2 on a usage error, which claude would take as a refusal); `HookArgs` reads
    /// them.
    #[command(hide = true, disable_help_flag = true)]
    Hook {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
        args: Vec<String>,
    },
    /// What a running Neovim runs so the core tells it whenever a worktree changes: `nvim
    /// register <address>` as it starts, `nvim unregister <address>` as it exits.
    #[command(hide = true)]
    Nvim {
        #[command(subcommand)]
        command: NvimCommand,
    },
}

#[derive(Debug, Subcommand)]
enum PrCommand {
    /// Ask gh about the pull request of each worktree's branch and its checks, and keep the
    /// answer for `worktree ls` and `worktree detail`. gh missing or not logged in is kept as
    /// unavailable, with gh's reason.
    Refresh {
        /// The worktrees' names, or their paths when several worktrees share a name (default:
        /// every worktree).
        worktrees: Vec<Target>,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum NvimCommand {
    /// Record a running Neovim's server address in the repository, to be told of every change.
    Register {
        /// The Neovim's server address (`v:servername`): the absolute path of its unix socket.
        // Parsed by clap through `Address`'s `FromStr`: anything else is refused here.
        address: Address,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Take a Neovim's server address out of the repository; one not recorded is not an error.
    Unregister {
        /// The Neovim's server address (`v:servername`): the absolute path of its unix socket.
        address: Address,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum WorktreeCommand {
    /// List the fleet: every worktree of the current repository.
    Ls {
        /// Print JSON carrying the contract integer instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Create a worktree on a new branch from the default branch.
    New {
        /// The worktree's name, which is also its branch and its directory's name.
        // Parsed by clap through `WorktreeName`'s `FromStr`: a name that breaks a rule is refused
        // here, before anything runs.
        #[arg(allow_hyphen_values = true)]
        name: WorktreeName,
        /// Approve the setup script of a committed orca-term.yaml without being asked, then run
        /// it.
        #[arg(long)]
        approve_setup: bool,
        /// Print JSON carrying the contract integer instead of text, and no setup output.
        #[arg(long)]
        json: bool,
    },
    /// Run a worktree's setup script again; a success makes the worktree ready.
    Setup {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// Approve the setup script of a committed orca-term.yaml without being asked, then run
        /// it.
        #[arg(long)]
        approve_setup: bool,
        /// Print JSON carrying the contract integer instead of text, and no setup output.
        #[arg(long)]
        json: bool,
    },
    /// Remove a worktree; its branch goes too only when merged into the default branch.
    Rm {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// Remove a worktree with changes and delete an unmerged branch.
        #[arg(long)]
        force: bool,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Clean the worktrees whose directory is gone (`git worktree prune`). Nothing else prunes.
    Prune,
    /// Mark a worktree seen: every turn that ended so far is reviewed, and a mark set by hand goes.
    Seen {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Mark a worktree unseen by hand, whatever its turns, until `worktree seen`.
    Unseen {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Print the commit a worktree's review compares against (where it forked from the default
    /// branch) and mark the worktree seen.
    Review {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Print everything about a worktree: why it is in its state, whole; the tool calls its agent
    /// was denied in its latest turn; the end of its setup log; how far it is ahead of or behind
    /// the local default branch. Only reads, and fetches nothing.
    Detail {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum NoteCommand {
    /// Attach a note to a line, a range of lines or (with neither flag) a whole file of a worktree.
    Add {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// The file, relative to the worktree's root; a whole-file note may name a deleted file.
        path: String,
        /// The line the note is on, numbered from 1.
        #[arg(long, conflicts_with = "lines", value_parser = clap::value_parser!(u32).range(1..))]
        line: Option<u32>,
        /// The lines the note is on, `S-E` with S before E, such as `3-5`.
        #[arg(long)]
        lines: Option<LineRange>,
        /// What the note says to the agent.
        #[arg(long, allow_hyphen_values = true)]
        body: String,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
    /// List a worktree's pending notes, each with where its lines are in the file now.
    Ls {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Replace what a pending note says.
    Edit {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// The note's id, as `note ls` shows it.
        id: u64,
        /// What the note says to the agent now.
        #[arg(long, allow_hyphen_values = true)]
        body: String,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Remove a pending note without sending it.
    Rm {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// The note's id, as `note ls` shows it.
        id: u64,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Paste pending notes into the worktree's running agent as one prompt, and consume them.
    Send {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// The ids of the notes to send, as `note ls` shows them (default: every pending note).
        ids: Vec<u64>,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum TurnCommand {
    /// List every turn of a worktree, oldest first, the open one last. Only reads.
    Ls {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// Print JSON carrying the contract integer instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Print what a turn changed: the diff from its begin snapshot to its end snapshot. Only
    /// reads.
    Diff {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// The turn's number, as `turn ls` shows it.
        turn: u64,
        /// Print JSON carrying the contract integer instead of the patch.
        #[arg(long)]
        json: bool,
    },
    /// Say which turn wrote each line of a file of a worktree as it is on disk now. Only reads.
    Blame {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// The file, relative to the worktree's root or absolute inside it.
        path: PathBuf,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum SessionCommand {
    /// List every session a worktree held, the most recently active first, the current one
    /// marked. Only reads.
    Ls {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// Print JSON carrying the contract integer instead of a table.
        #[arg(long)]
        json: bool,
    },
    /// Make one of a worktree's sessions its current one and restart its agent on it, swapping
    /// the worktree's agent when the session is the other agent's (refused while a turn runs).
    Resume {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// The session's id, as `session ls` shows it.
        id: String,
        /// The agent's first terminal height, in rows (default: this terminal's, else 24).
        #[arg(long)]
        rows: Option<u16>,
        /// The agent's first terminal width, in columns (default: this terminal's, else 80).
        #[arg(long)]
        cols: Option<u16>,
        /// Approve the sandbox's extra writable paths of a committed orca-term.yaml without being
        /// asked, then start.
        #[arg(long)]
        approve_sandbox: bool,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Discard a worktree's current session, so its agent's next start begins a new one; a
    /// running agent is restarted on a new session at once (refused while a turn runs). The
    /// discarded session keeps its turns, and `session resume` brings it back.
    New {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// The agent's first terminal height, in rows (default: this terminal's, else 24).
        #[arg(long)]
        rows: Option<u16>,
        /// The agent's first terminal width, in columns (default: this terminal's, else 80).
        #[arg(long)]
        cols: Option<u16>,
        /// Approve the sandbox's extra writable paths of a committed orca-term.yaml without being
        /// asked, then start.
        #[arg(long)]
        approve_sandbox: bool,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
enum AgentCommand {
    /// Start a worktree's agent, claude-code or opencode, inside its sandbox, held by a detached
    /// process.
    Start {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// The agent to start: claude-code or opencode (default: the one the worktree recorded,
        /// else claude-code). It is recorded as the worktree's agent.
        // Parsed by clap through `Agent`'s `FromStr`: an unknown agent is refused here.
        #[arg(long)]
        agent: Option<Agent>,
        /// The agent's first terminal height, in rows (default: this terminal's, else 24).
        #[arg(long)]
        rows: Option<u16>,
        /// The agent's first terminal width, in columns (default: this terminal's, else 80).
        #[arg(long)]
        cols: Option<u16>,
        /// Start a fresh session instead of resuming the one the worktree recorded (no effect
        /// when the agent is already running).
        #[arg(long)]
        fresh: bool,
        /// Approve the sandbox's extra writable paths of a committed orca-term.yaml without being
        /// asked, then start.
        #[arg(long)]
        approve_sandbox: bool,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Swap a worktree's agent for another: stop the one running (refused while it is in a turn)
    /// and start the other on the same worktree, which records it. Turns, notes and snapshots stay.
    Swap {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// The agent to swap in: claude-code or opencode.
        agent: Agent,
        /// The new agent's first terminal height, in rows (default: this terminal's, else 24).
        #[arg(long)]
        rows: Option<u16>,
        /// The new agent's first terminal width, in columns (default: this terminal's, else 80).
        #[arg(long)]
        cols: Option<u16>,
        /// Approve the sandbox's extra writable paths of a committed orca-term.yaml without being
        /// asked, then start.
        #[arg(long)]
        approve_sandbox: bool,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Attach this terminal to a worktree's running agent; Ctrl-\ detaches and leaves it running.
    Attach {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// Let no key detach: only closing the terminal does (for Neovim, whose terminal uses
        /// Ctrl-\ itself).
        #[arg(long)]
        no_detach_key: bool,
    },
    /// Stop a worktree's agent and wait for its holder to end.
    Stop {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// Print JSON carrying the contract integer instead of text.
        #[arg(long)]
        json: bool,
    },
    /// The holder process itself, started by `agent start`.
    #[command(hide = true)]
    Holder {
        /// The worktree's admin directory, absolute.
        #[arg(long)]
        admin: PathBuf,
        /// The worktree, absolute.
        #[arg(long)]
        worktree: PathBuf,
        #[arg(long)]
        rows: u16,
        #[arg(long)]
        cols: u16,
        /// For an agent that reports its events through a pipe (opencode): the agent its hooks
        /// are named after, whose hook runs for each line.
        #[arg(long)]
        events: Option<String>,
        /// The agent's command line, program first.
        #[arg(last = true, required = true)]
        argv: Vec<String>,
    },
}

impl Cli {
    /// How the command asked for its output, which is also how its error is printed.
    pub fn format(&self) -> Format {
        let json = match &self.command {
            Command::Worktree { command } => match command {
                WorktreeCommand::Ls { json }
                | WorktreeCommand::New { json, .. }
                | WorktreeCommand::Setup { json, .. }
                | WorktreeCommand::Rm { json, .. }
                | WorktreeCommand::Seen { json, .. }
                | WorktreeCommand::Unseen { json, .. }
                | WorktreeCommand::Review { json, .. }
                | WorktreeCommand::Detail { json, .. } => *json,
                WorktreeCommand::Prune => false,
            },
            Command::Agent { command } => match command {
                AgentCommand::Start { json, .. }
                | AgentCommand::Swap { json, .. }
                | AgentCommand::Stop { json, .. } => *json,
                AgentCommand::Attach { .. } | AgentCommand::Holder { .. } => false,
            },
            Command::Note { command } => match command {
                NoteCommand::Add { json, .. }
                | NoteCommand::Ls { json, .. }
                | NoteCommand::Edit { json, .. }
                | NoteCommand::Rm { json, .. }
                | NoteCommand::Send { json, .. } => *json,
            },
            Command::Turn { command } => match command {
                TurnCommand::Ls { json, .. }
                | TurnCommand::Diff { json, .. }
                | TurnCommand::Blame { json, .. } => *json,
            },
            Command::Session { command } => match command {
                SessionCommand::Ls { json, .. }
                | SessionCommand::Resume { json, .. }
                | SessionCommand::New { json, .. } => *json,
            },
            Command::Pr {
                command: PrCommand::Refresh { json, .. },
            }
            | Command::Doctor { json } => *json,
            Command::Hook { .. } => false,
            Command::Nvim { command } => match command {
                NvimCommand::Register { json, .. } | NvimCommand::Unregister { json, .. } => *json,
            },
        };
        Format::from_json_flag(json)
    }

    /// Whether the command runs git, and so checks git first. `doctor` reports on git instead,
    /// the hook must stay fast and silent, and the holder is started by `agent start`, which
    /// checked.
    fn runs_git(&self) -> bool {
        match &self.command {
            Command::Doctor { .. } | Command::Hook { .. } => false,
            Command::Agent { command } => !matches!(command, AgentCommand::Holder { .. }),
            Command::Worktree { .. }
            | Command::Note { .. }
            | Command::Turn { .. }
            | Command::Session { .. }
            | Command::Pr { .. }
            | Command::Nvim { .. } => true,
        }
    }
}

/// Runs the command `cli` names and returns the process's exit code: 1 when a setup failed,
/// although the command itself did its job. A command that runs git first refuses, as the typed
/// `prerequisite`, when git is missing or too old.
pub fn run(cli: &Cli) -> Result<ExitCode> {
    if cli.runs_git() {
        check_git()?;
    }
    match &cli.command {
        Command::Doctor { .. } => Ok(output::doctor(&doctor::run(), cli.format())?),
        Command::Worktree { command } => run_worktree(command, cli.format()),
        Command::Agent { command } => run_agent(command, cli.format()),
        Command::Note { command } => run_note(command, cli.format()),
        Command::Turn { command } => run_turn(command, cli.format()),
        Command::Session { command } => run_session(command, cli.format()),
        Command::Pr {
            command: PrCommand::Refresh { worktrees, .. },
        } => {
            output::prs_refreshed(&pr_refresh::run(worktrees)?, cli.format())?;
            Ok(ExitCode::SUCCESS)
        }
        Command::Hook { args } => {
            run_hook(&HookArgs::parse(args));
            Ok(ExitCode::SUCCESS)
        }
        Command::Nvim { command } => run_nvim(command, cli.format()),
    }
}

/// The arguments of `hook`, read leniently: the agent first, `--admin <dir>` (or `--admin=<dir>`)
/// anywhere, and the event as the next word. Anything else is ignored.
#[derive(Debug, Default, PartialEq, Eq)]
struct HookArgs {
    agent: Option<String>,
    admin: Option<PathBuf>,
    /// The event as the settings name it: only the recording tap uses it, since the payload names
    /// it too.
    event: Option<String>,
}

impl HookArgs {
    fn parse(args: &[String]) -> Self {
        let mut parsed = Self::default();
        let mut words = args.iter();
        while let Some(word) = words.next() {
            match word.as_str() {
                "--admin" => parsed.admin = words.next().map(PathBuf::from),
                flag if flag.starts_with("--admin=") => {
                    parsed.admin = Some(PathBuf::from(&flag["--admin=".len()..]));
                }
                // Any other flag is ignored.
                flag if flag.starts_with('-') => {}
                _ if parsed.agent.is_none() => parsed.agent = Some(word.clone()),
                _ if parsed.event.is_none() => parsed.event = Some(word.clone()),
                _ => {}
            }
        }
        parsed
    }
}

/// Runs the hook, which exits 0 and prints nothing whatever happens: even a panic is caught, and
/// its message silenced, since claude reads a hook's output and exit status.
fn run_hook(args: &HookArgs) {
    panic::set_hook(Box::new(|_| {}));
    let _ = panic::catch_unwind(|| {
        hook::run(
            args.agent.as_deref(),
            args.event.as_deref(),
            args.admin.as_deref(),
        );
    });
}

fn run_worktree(command: &WorktreeCommand, format: Format) -> Result<ExitCode> {
    match command {
        WorktreeCommand::New {
            name,
            approve_setup,
            ..
        } => run_worktree_new(name, *approve_setup, format),
        WorktreeCommand::Setup {
            worktree,
            approve_setup,
            ..
        } => run_worktree_setup(worktree, *approve_setup, format),
        WorktreeCommand::Rm {
            worktree, force, ..
        } => {
            let removal = if *force {
                Removal::Forced
            } else {
                Removal::Safe
            };
            output::worktree_removed(&worktree_rm::run(worktree, removal)?, format)?;
            Ok(ExitCode::SUCCESS)
        }
        WorktreeCommand::Prune => {
            output::worktrees_pruned(&worktree_prune::run()?);
            Ok(ExitCode::SUCCESS)
        }
        WorktreeCommand::Seen { worktree, .. } => {
            output::worktree_marked(&worktree_seen::run(worktree, Mark::Seen)?, format)?;
            Ok(ExitCode::SUCCESS)
        }
        WorktreeCommand::Unseen { worktree, .. } => {
            output::worktree_marked(&worktree_seen::run(worktree, Mark::Unseen)?, format)?;
            Ok(ExitCode::SUCCESS)
        }
        WorktreeCommand::Review { worktree, .. } => {
            output::worktree_reviewed(&worktree_review::run(worktree)?, format)?;
            Ok(ExitCode::SUCCESS)
        }
        WorktreeCommand::Detail { worktree, .. } => {
            output::worktree_detail(&worktree_detail::run(worktree)?, format)?;
            Ok(ExitCode::SUCCESS)
        }
        WorktreeCommand::Ls { json: true } => {
            output::fleet_json(&worktree_ls::run()?)?;
            Ok(ExitCode::SUCCESS)
        }
        WorktreeCommand::Ls { json: false } => {
            output::fleet_table(&worktree_ls::run()?);
            Ok(ExitCode::SUCCESS)
        }
    }
}

/// Runs `worktree new`: says what it creates, creates it and prints what it made.
fn run_worktree_new(name: &WorktreeName, approve_setup: bool, format: Format) -> Result<ExitCode> {
    output::creating_worktree(name, format);
    let mut ask = ask_at_terminal;
    let mut approval = approval(approve_setup, format, &mut ask);
    let created = worktree_new::run(name, &mut approval, &mut *output::live_echo(format))?;
    output::worktree_created(&created, format)?;
    Ok(exit_code(&created.setup))
}

/// Runs `worktree setup`: says whose setup it runs, runs it and prints what it did.
fn run_worktree_setup(worktree: &Target, approve_setup: bool, format: Format) -> Result<ExitCode> {
    output::running_setup(worktree, format);
    let mut ask = ask_at_terminal;
    let mut approval = approval(approve_setup, format, &mut ask);
    let done = worktree_setup::run(worktree, &mut approval, &mut *output::live_echo(format))?;
    output::worktree_set_up(&done, format)?;
    Ok(exit_code(&done.setup))
}

/// How `worktree new`, `worktree setup` and `agent start` get the person's approval of what a
/// committed `orca-term.yaml` asks for (a setup script, the sandbox's writable paths): given
/// beforehand with `--approve-setup` or `--approve-sandbox` (`approved`); else asked through `ask`
/// when the output is text and both stdin and stderr are terminals; else nobody can be asked, and
/// it is not used.
fn approval(
    approved: bool,
    format: Format,
    ask: &mut dyn FnMut(&Unapproved) -> Result<bool>,
) -> Approval<'_> {
    if approved {
        Approval::Given
    } else if format == Format::Text && terminal::is_interactive() {
        Approval::Ask(ask)
    } else {
        Approval::Unavailable
    }
}

/// Shows the script or the paths `unapproved` and where they come from on stderr, asks `run this
/// setup script? [y/N]` or `widen the sandbox with these writable paths? [y/N]`, and reads the
/// answer, one line, from stdin: whether the person approved them.
fn ask_at_terminal(unapproved: &Unapproved) -> Result<bool> {
    output::ask_approval(unapproved);
    Ok(trust::is_yes(&stdin::read_line()?))
}

fn run_agent(command: &AgentCommand, format: Format) -> Result<ExitCode> {
    match command {
        AgentCommand::Start {
            worktree,
            agent,
            rows,
            cols,
            fresh,
            approve_sandbox,
            ..
        } => {
            let mode = StartMode::from_fresh_flag(*fresh);
            let wanted = agent_start::Wanted {
                agent: *agent,
                rows: *rows,
                cols: *cols,
                mode,
            };
            run_agent_start(worktree, &wanted, *approve_sandbox, format)?;
        }
        AgentCommand::Swap {
            worktree,
            agent,
            rows,
            cols,
            approve_sandbox,
            ..
        } => {
            let size = (*rows, *cols);
            run_agent_swap(worktree, *agent, size, *approve_sandbox, format)?;
        }
        AgentCommand::Holder {
            admin,
            worktree,
            rows,
            cols,
            events,
            argv,
        } => {
            let size = Size {
                rows: *rows,
                cols: *cols,
            };
            run_holder(admin, worktree, size, events.as_deref(), argv)?;
        }
        AgentCommand::Attach {
            worktree,
            no_detach_key,
        } => {
            let detach_key = if *no_detach_key {
                DetachKey::None
            } else {
                DetachKey::CtrlBackslash
            };
            output::attach_ended(&agent_attach::run(worktree, detach_key)?);
        }
        AgentCommand::Stop { worktree, .. } => {
            output::agent_stopped(&agent_stop::run(worktree)?, format)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// Runs `agent swap` of `worktree` for `agent`, with its first terminal `size` (rows, columns)
/// where given, the sandbox's writable paths approved beforehand when `approve_sandbox`, and
/// prints what it did.
fn run_agent_swap(
    worktree: &Target,
    agent: Agent,
    size: (Option<u16>, Option<u16>),
    approve_sandbox: bool,
    format: Format,
) -> Result<()> {
    let mut ask = ask_at_terminal;
    let mut approval = approval(approve_sandbox, format, &mut ask);
    let swapped = agent_swap::run(worktree, agent, size.0, size.1, &mut approval)?;
    output::agent_swapped(&swapped, format)
}

/// Runs the hidden `agent holder`, the holder process itself, until its agent `argv` exits.
fn run_holder(
    admin: &Path,
    worktree: &Path,
    size: Size,
    events: Option<&str>,
    argv: &[String],
) -> Result<()> {
    agent_holder::run(&HolderArgs {
        admin: AdminDir::new(admin.to_owned()),
        worktree: worktree.to_owned(),
        size,
        argv: argv.to_vec(),
        events: events.map(str::to_owned),
    })
}

/// Runs `agent start` of what `wanted` names in `worktree`, the sandbox's writable paths approved
/// beforehand when `approve_sandbox`, and prints what it did.
fn run_agent_start(
    worktree: &Target,
    wanted: &agent_start::Wanted,
    approve_sandbox: bool,
    format: Format,
) -> Result<()> {
    let mut ask = ask_at_terminal;
    let mut approval = approval(approve_sandbox, format, &mut ask);
    let started = agent_start::run(worktree, wanted, &mut approval)?;
    output::agent_started(&started, format)
}

fn run_note(command: &NoteCommand, format: Format) -> Result<ExitCode> {
    match command {
        NoteCommand::Add {
            worktree,
            path,
            line,
            lines,
            body,
            ..
        } => {
            let anchor = match (line, lines) {
                (Some(line), _) => Anchor::line(*line)?,
                (None, Some(range)) => Anchor::Lines(*range),
                (None, None) => Anchor::File,
            };
            output::noted(&note_add::run(worktree, path, anchor, body)?, format)?;
        }
        NoteCommand::Ls { worktree, .. } => output::notes_listed(&note_ls::run(worktree)?, format)?,
        NoteCommand::Edit {
            worktree, id, body, ..
        } => output::noted(&note_edit::run(worktree, *id, body)?, format)?,
        NoteCommand::Rm { worktree, id, .. } => {
            output::unnoted(&note_rm::run(worktree, *id)?, format)?;
        }
        NoteCommand::Send { worktree, ids, .. } => {
            output::notes_sent(&note_send::run(worktree, ids)?, format)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn run_turn(command: &TurnCommand, format: Format) -> Result<ExitCode> {
    match command {
        TurnCommand::Ls { worktree, .. } => output::turns_listed(&turn_ls::run(worktree)?, format)?,
        TurnCommand::Diff { worktree, turn, .. } => {
            output::turn_diff(&turn_diff::run(worktree, *turn)?, format)?;
        }
        TurnCommand::Blame { worktree, path, .. } => {
            output::turn_blame(&turn_blame::run(worktree, path)?, format)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn run_session(command: &SessionCommand, format: Format) -> Result<ExitCode> {
    let mut ask = ask_at_terminal;
    match command {
        SessionCommand::Ls { worktree, .. } => {
            output::sessions_listed(&session_ls::run(worktree)?, format)?;
        }
        SessionCommand::Resume {
            worktree,
            id,
            rows,
            cols,
            approve_sandbox,
            ..
        } => {
            let mut approval = approval(*approve_sandbox, format, &mut ask);
            let resumed = session_resume::run(worktree, id, *rows, *cols, &mut approval)?;
            output::session_resumed(&resumed, format)?;
        }
        SessionCommand::New {
            worktree,
            rows,
            cols,
            approve_sandbox,
            ..
        } => {
            let mut approval = approval(*approve_sandbox, format, &mut ask);
            let discarded = session_new::run(worktree, *rows, *cols, &mut approval)?;
            output::session_discarded(&discarded, format)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn run_nvim(command: &NvimCommand, format: Format) -> Result<ExitCode> {
    match command {
        NvimCommand::Register { address, .. } => {
            nvim_register::run(address)?;
            output::nvim_registered(address, format)?;
        }
        NvimCommand::Unregister { address, .. } => {
            nvim_unregister::run(address)?;
            output::nvim_unregistered(address, format)?;
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// A failed setup leaves the worktree not ready and the command exits 1.
fn exit_code(setup: &SetupReport) -> ExitCode {
    if setup.outcome.failed() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(args: &[&str]) -> Vec<String> {
        args.iter().map(|&arg| arg.to_owned()).collect()
    }

    #[test]
    fn hook_arguments_are_read_leniently() {
        assert_eq!(
            HookArgs::parse(&words(&["claude", "--admin", "/a/orca-term", "Stop"])),
            HookArgs {
                agent: Some("claude".to_owned()),
                admin: Some(PathBuf::from("/a/orca-term")),
                event: Some("Stop".to_owned()),
            }
        );
        assert_eq!(
            HookArgs::parse(&words(&[
                "--bogus",
                "claude",
                "Stop",
                "--admin=/a",
                "extra"
            ])),
            HookArgs {
                agent: Some("claude".to_owned()),
                admin: Some(PathBuf::from("/a")),
                event: Some("Stop".to_owned()),
            }
        );
        assert_eq!(HookArgs::parse(&[]), HookArgs::default());
        assert_eq!(HookArgs::parse(&words(&["claude", "--admin"])).admin, None);
    }

    #[test]
    fn only_doctor_the_hook_and_the_holder_skip_the_git_check() {
        let runs_git = |args: &[&str]| Cli::try_parse_from(args).unwrap().runs_git();
        assert!(!runs_git(&["orca-term", "doctor"]));
        assert!(!runs_git(&["orca-term", "hook", "claude", "Stop"]));
        assert!(!runs_git(&[
            "orca-term",
            "agent",
            "holder",
            "--admin",
            "/a",
            "--worktree",
            "/w",
            "--rows",
            "1",
            "--cols",
            "1",
            "--",
            "claude",
        ]));
        for args in [
            &["orca-term", "worktree", "ls"][..],
            &["orca-term", "worktree", "prune"],
            &["orca-term", "agent", "attach", "fix"],
            &["orca-term", "note", "ls", "fix"],
            &["orca-term", "turn", "ls", "fix"],
            &["orca-term", "pr", "refresh"],
            &["orca-term", "nvim", "unregister", "/tmp/n.sock"],
        ] {
            assert!(runs_git(args), "{args:?}");
        }
    }

    #[test]
    fn clap_takes_any_hook_arguments() {
        for args in [
            vec!["orca-term", "hook"],
            vec!["orca-term", "hook", "--help"],
            vec![
                "orca-term",
                "hook",
                "claude",
                "--admin",
                "/a",
                "Stop",
                "--what",
                "-x",
            ],
        ] {
            let cli = Cli::try_parse_from(&args).unwrap();
            assert!(matches!(cli.command, Command::Hook { .. }), "{args:?}");
        }
    }
}
