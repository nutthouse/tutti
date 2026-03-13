mod automation;
mod cli;
mod config;
mod error;
mod runtime;
mod session;
mod state;
mod usage;
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
        Commands::Watch {
            interval,
            restart_persistent,
        } => cli::watch::run(interval, restart_persistent),
        Commands::Doctor => cli::doctor::run(),
        Commands::Attach { ref agent } => cli::attach::run(agent),
        Commands::Peek { ref agent, lines } => cli::peek::run(agent, lines),
        Commands::Logs {
            ref agent,
            lines,
            follow,
        } => cli::logs::run(agent, lines, follow),
        Commands::Switch => cli::switch::run(),
        Commands::Run {
            ref workflow,
            ref agent,
            strict,
            dry_run,
        } => cli::run::run(workflow, agent.as_deref(), strict, dry_run),
        Commands::Verify {
            last,
            json,
            ref workflow,
            ref agent,
            strict,
        } => cli::verify::run(last, json, workflow.as_deref(), agent.as_deref(), strict),
        Commands::Usage {
            ref profile,
            by_workspace,
        } => cli::usage::run(profile.as_deref(), by_workspace),
        Commands::Permissions { command } => cli::permissions::run(command),
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
