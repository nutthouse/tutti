use crate::config::TuttiConfig;
use crate::error::{Result, TuttiError};
use crate::session::TmuxSession;
use crate::state;
use crate::state::ControlEvent;
use crate::worktree;
use crate::{automation::HookDispatcher, automation::HookEventPayload};
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
    config.validate()?;
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
    let mut hook_errors: Vec<String> = Vec::new();

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
        let _ = state::append_control_event(
            project_root,
            &ControlEvent {
                event: "agent.stopped".to_string(),
                workspace: config.workspace.name.clone(),
                agent: Some(agent.name.clone()),
                timestamp: Utc::now(),
                correlation_id: format!("down-{}-{}", Utc::now().timestamp_millis(), agent.name),
                data: Some(serde_json::json!({"reason":"manual","session_name":session})),
            },
        );
        println!("  {} {}", "stopped".red(), agent.name);
        stopped += 1;

        if !config.hooks.is_empty() {
            let payload = HookEventPayload {
                workspace_name: config.workspace.name.clone(),
                project_root: project_root.to_path_buf(),
                agent_name: agent.name.clone(),
                runtime: agent
                    .resolved_runtime(&config.defaults, &config.roles)
                    .unwrap_or_else(|| "—".to_string()),
                session_name: session.clone(),
                reason: "manual".to_string(),
            };
            if let Err(e) = HookDispatcher::dispatch_agent_stop(&config, &payload) {
                eprintln!(
                    "  {} hook dispatch for {}: {e}",
                    "warn".yellow(),
                    agent.name
                );
                hook_errors.push(format!("{}: {e}", agent.name));
            }
        }

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

    if !hook_errors.is_empty() {
        return Err(TuttiError::ConfigValidation(format!(
            "hook failures: {}",
            hook_errors.join("; ")
        )));
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
                if let Err(e) = config.validate() {
                    eprintln!("  Skipping {} (invalid config): {e}", ws.name);
                    continue;
                }
                let project_root = config_path.parent().unwrap();
                let mut ws_stopped = 0;
                let mut ws_hook_errors: Vec<String> = Vec::new();

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
                        let _ = state::append_control_event(
                            project_root,
                            &ControlEvent {
                                event: "agent.stopped".to_string(),
                                workspace: config.workspace.name.clone(),
                                agent: Some(agent.name.clone()),
                                timestamp: Utc::now(),
                                correlation_id: format!(
                                    "down-{}-{}",
                                    Utc::now().timestamp_millis(),
                                    agent.name
                                ),
                                data: Some(
                                    serde_json::json!({"reason":"manual","session_name":session}),
                                ),
                            },
                        );
                        ws_stopped += 1;

                        if !config.hooks.is_empty() {
                            let payload = HookEventPayload {
                                workspace_name: config.workspace.name.clone(),
                                project_root: project_root.to_path_buf(),
                                agent_name: agent.name.clone(),
                                runtime: agent
                                    .resolved_runtime(&config.defaults, &config.roles)
                                    .unwrap_or_else(|| "—".to_string()),
                                session_name: session.clone(),
                                reason: "manual".to_string(),
                            };
                            if let Err(e) = HookDispatcher::dispatch_agent_stop(&config, &payload) {
                                eprintln!(
                                    "  {} hook dispatch for {}: {e}",
                                    "warn".yellow(),
                                    agent.name
                                );
                                ws_hook_errors.push(format!("{}:{}: {e}", ws.name, agent.name));
                            }
                        }

                        if clean {
                            let _ = worktree::remove_worktree(project_root, &agent.name);
                        }
                    }
                }

                if ws_stopped > 0 {
                    println!("{}: stopped {ws_stopped} agent(s)", ws.name);
                }
                if !ws_hook_errors.is_empty() {
                    return Err(TuttiError::ConfigValidation(format!(
                        "hook failures: {}",
                        ws_hook_errors.join("; ")
                    )));
                }
            }
            Err(_) => continue,
        }
    }

    Ok(())
}
