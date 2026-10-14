//! Edge: the command-line surface. It parses the arguments, hands each command to its ops module
//! and gives the result to `output`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

use crate::adapters::terminal::DetachKey;
use crate::domain::agent::Size;
use crate::domain::setup::SetupReport;
use crate::domain::worktree::{Removal, Target, WorktreeName};
use crate::ops::{
    agent_attach, agent_holder, agent_start, agent_stop, worktree_ls, worktree_new, worktree_prune,
    worktree_rm, worktree_setup,
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
        /// Print JSON carrying the contract integer instead of text, and no setup output.
        #[arg(long)]
        json: bool,
    },
    /// Run a worktree's setup script again; a success makes the worktree ready.
    Setup {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
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
}

#[derive(Debug, Subcommand)]
enum AgentCommand {
    /// Start claude in a worktree, inside its sandbox, held by a detached process.
    Start {
        /// The worktree's name, or its path when several worktrees share the name.
        worktree: Target,
        /// The agent's first terminal height, in rows (default: this terminal's, else 24).
        #[arg(long)]
        rows: Option<u16>,
        /// The agent's first terminal width, in columns (default: this terminal's, else 80).
        #[arg(long)]
        cols: Option<u16>,
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
                | WorktreeCommand::Rm { json, .. } => *json,
                WorktreeCommand::Prune => false,
            },
            Command::Agent { command } => match command {
                AgentCommand::Start { json, .. } | AgentCommand::Stop { json, .. } => *json,
                AgentCommand::Attach { .. } | AgentCommand::Holder { .. } => false,
            },
        };
        Format::from_json_flag(json)
    }
}

/// Runs the command `cli` names and returns the process's exit code: 1 when a setup failed,
/// although the command itself did its job.
pub fn run(cli: &Cli) -> Result<ExitCode> {
    match &cli.command {
        Command::Worktree { command } => run_worktree(command, cli.format()),
        Command::Agent { command } => run_agent(command, cli.format()),
    }
}

fn run_worktree(command: &WorktreeCommand, format: Format) -> Result<ExitCode> {
    match command {
        WorktreeCommand::New { name, .. } => {
            output::creating_worktree(name, format);
            let created = worktree_new::run(name, &mut *output::live_echo(format))?;
            output::worktree_created(&created, format)?;
            Ok(exit_code(&created.setup))
        }
        WorktreeCommand::Setup { worktree, .. } => {
            output::running_setup(worktree, format);
            let done = worktree_setup::run(worktree, &mut *output::live_echo(format))?;
            output::worktree_set_up(&done, format)?;
            Ok(exit_code(&done.setup))
        }
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

fn run_agent(command: &AgentCommand, format: Format) -> Result<ExitCode> {
    match command {
        AgentCommand::Start {
            worktree,
            rows,
            cols,
            ..
        } => {
            let started = agent_start::run(worktree, *rows, *cols)?;
            output::agent_started(&started, format)?;
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
        AgentCommand::Holder {
            admin,
            worktree,
            rows,
            cols,
            argv,
        } => {
            let size = Size {
                rows: *rows,
                cols: *cols,
            };
            agent_holder::run(admin.clone(), worktree.clone(), size, argv.clone())?;
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
