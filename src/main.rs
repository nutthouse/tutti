mod auth;
mod automation;
mod budget;
mod cli;
mod config;
mod dashboard;
mod error;
mod health;
mod permissions;
mod runtime;
mod scheduler;
mod session;
mod state;
mod usage;
mod webhook;
mod worktree;

use clap::Parser;
use cli::{
    Cli, Commands, IssueClaimSubcommand, RemoteSubcommand, RunsSubcommand, WorkspacesSubcommand,
};
use std::process;

fn main() {
    let cli = Cli::parse();

    let result = match cli.command {
        Commands::Init => cli::init::run(),
        Commands::Up {
            ref agent,
            ref workspace,
            all,
            fresh_worktree,
            mode,
            policy,
        } => cli::up::run(
            agent.as_deref(),
            workspace.as_deref(),
            all,
            fresh_worktree,
            mode,
            policy,
        ),
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
        Commands::Doctor { json, strict } => cli::doctor::run(json, strict),
        Commands::Attach { ref agent } => cli::attach::run(agent),
        Commands::Diff {
            ref agent,
            staged,
            name_only,
            stat,
        } => cli::diff::run(agent, staged, name_only, stat),
        Commands::Detect {
            ref agent,
            lines,
            json,
        } => cli::detect::run(agent, lines, json),
        Commands::Land {
            ref agent,
            pr,
            force,
        } => cli::land::run(agent, pr, force),
        Commands::Review {
            ref agent,
            ref reviewer,
        } => cli::review::run(agent, reviewer),
        Commands::Send {
            ref agent,
            auto_up,
            wait,
            timeout_secs,
            idle_stable_secs,
            output,
            output_lines,
            ref prompt,
        } => cli::send::run(
            agent,
            prompt,
            cli::send::SendOptions {
                auto_up,
                wait,
                timeout_secs,
                idle_stable_secs,
                output,
                output_lines,
            },
        )
        .map(|_| ()),
        Commands::Peek { ref agent, lines } => cli::peek::run(agent, lines),
        Commands::Logs {
            ref agent,
            lines,
            follow,
        } => cli::logs::run(agent, lines, follow),
        Commands::Health {
            ref agent,
            ref workspace,
            all,
            json,
        } => cli::health::run(agent.as_deref(), workspace.as_deref(), all, json),
        Commands::Serve {
            ref workspace,
            all,
            port,
            probe_interval,
            remote,
            ref bind,
        } => cli::serve::run(
            workspace.as_deref(),
            all,
            port,
            probe_interval,
            remote,
            bind.as_deref(),
        ),
        Commands::Switch => cli::switch::run(),
        Commands::Handoff { command } => cli::handoff::run(command),
        Commands::Run {
            ref workflow,
            ref resume,
            list,
            ref agent,
            json,
            strict,
            dry_run,
        } => cli::run::run(
            workflow.as_deref(),
            resume.as_deref(),
            list,
            agent.as_deref(),
            json,
            strict,
            dry_run,
        ),
        Commands::Runs { command } => match command {
            RunsSubcommand::List => cli::runs::list(),
            RunsSubcommand::Show { ref run_id } => cli::runs::show(run_id),
        },
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
        Commands::Remote { command } => match command {
            RemoteSubcommand::Attach {
                ref host,
                port,
                ref name,
            } => cli::remote::attach(host, port, name.as_deref()),
            RemoteSubcommand::Status => cli::remote::status(),
        },
        Commands::IssueClaim { command } => match command {
            IssueClaimSubcommand::Acquire {
                ref output,
                ref label,
                ref milestone,
                lease_ttl_secs,
            } => cli::issue_claim::acquire(output, label, milestone.as_deref(), lease_ttl_secs),
            IssueClaimSubcommand::Heartbeat { ref state } => cli::issue_claim::heartbeat(state),
            IssueClaimSubcommand::Release {
                ref state,
                ref reason,
            } => cli::issue_claim::release(state, reason.as_deref()),
            IssueClaimSubcommand::Sweep => cli::issue_claim::sweep(),
        },
    };

    if let Err(e) = result {
        let attr = state::classify_failure(&e);
        eprintln!("error[{}]: {}", attr.category, attr.message);
        eprintln!("  hint: {}", attr.hint);
        process::exit(1);
    }
}
