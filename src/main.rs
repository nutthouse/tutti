mod cli;
mod config;
mod error;
mod runtime;
mod session;
mod state;
mod worktree;

use clap::Parser;
use cli::{Cli, Commands, WorkspacesSubcommand};
use std::process;

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init => cli::init::run(),
        Commands::Up {
            ref agent,
            ref workspace,
            all,
        } => cli::up::run(agent.as_deref(), workspace.as_deref(), all),
        Commands::Down {
            ref agent,
            ref workspace,
            all,
            clean,
        } => cli::down::run(agent.as_deref(), workspace.as_deref(), all, clean),
        Commands::Status { all } | Commands::Voices { all } => cli::status::run(all),
        Commands::Watch { interval, .. } => cli::watch::run(interval),
        Commands::Attach { ref agent } => cli::attach::run(agent),
        Commands::Peek { ref agent, lines } => cli::peek::run(agent, lines),
        Commands::Workspaces { ref command } => match command {
            Some(WorkspacesSubcommand::Status) => cli::workspaces::status(),
            None => cli::workspaces::list(),
        },
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        process::exit(1);
    }
}
