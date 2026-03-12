use crate::config::TuttiConfig;
use crate::error::{Result, TuttiError};
use crate::session::TmuxSession;
use crate::state;
use crate::worktree;
use chrono::Utc;
use colored::Colorize;

pub fn run(
    agent_filter: Option<&str>,
    workspace_name: Option<&str>,
    all: bool,
    clean: bool,
) -> Result<()> {
    crate::session::tmux::check_tmux()?;

    if all {
        return run_all(clean);
    }

    let (config, config_path) = if let Some(ws) = workspace_name {
        super::up::load_workspace_by_name(ws)?
    } else {
        let cwd = std::env::current_dir()?;
        TuttiConfig::load(&cwd)?
    };
    let project_root = config_path.parent().unwrap();

    let agents: Vec<_> = if let Some(name) = agent_filter {
        let agent = config
            .agents
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| TuttiError::AgentNotFound(name.to_string()))?;
        vec![agent]
    } else {
        config.agents.iter().collect()
    };

    let mut stopped = 0;

    for agent in &agents {
        let session = TmuxSession::session_name(&config.workspace.name, &agent.name);

        if !TmuxSession::session_exists(&session) {
            println!("  {} {} (not running)", "skip".yellow(), agent.name);
            continue;
        }

        // Update state before killing
        if let Ok(Some(mut agent_state)) = state::load_agent_state(project_root, &agent.name) {
            agent_state.status = "Stopped".to_string();
            agent_state.stopped_at = Some(Utc::now());
            let _ = state::save_agent_state(project_root, &agent_state);
        }

        TmuxSession::kill_session(&session)?;
        println!("  {} {}", "stopped".red(), agent.name);
        stopped += 1;

        if clean {
            if let Err(e) = worktree::remove_worktree(project_root, &agent.name) {
                eprintln!(
                    "  {} cleaning worktree for {}: {e}",
                    "warn".yellow(),
                    agent.name
                );
            } else {
                println!("  {} worktree for {}", "cleaned".dimmed(), agent.name);
            }
        }
    }

    if stopped == 0 {
        println!("No running agents to stop.");
    } else {
        println!("\nStopped {stopped} agent(s).");
    }

    Ok(())
}

fn run_all(clean: bool) -> Result<()> {
    let global = crate::config::GlobalConfig::load()?;
    if global.registered_workspaces.is_empty() {
        println!("No registered workspaces.");
        return Ok(());
    }

    for ws in &global.registered_workspaces {
        match TuttiConfig::load(&ws.path) {
            Ok((config, config_path)) => {
                let project_root = config_path.parent().unwrap();
                let mut ws_stopped = 0;

                for agent in &config.agents {
                    let session = TmuxSession::session_name(&config.workspace.name, &agent.name);
                    if TmuxSession::session_exists(&session) {
                        if let Ok(Some(mut agent_state)) =
                            state::load_agent_state(project_root, &agent.name)
                        {
                            agent_state.status = "Stopped".to_string();
                            agent_state.stopped_at = Some(Utc::now());
                            let _ = state::save_agent_state(project_root, &agent_state);
                        }
                        let _ = TmuxSession::kill_session(&session);
                        ws_stopped += 1;

                        if clean {
                            let _ = worktree::remove_worktree(project_root, &agent.name);
                        }
                    }
                }

                if ws_stopped > 0 {
                    println!("{}: stopped {ws_stopped} agent(s)", ws.name);
                }
            }
            Err(_) => continue,
        }
    }

    Ok(())
}
