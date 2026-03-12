use crate::config::TuttiConfig;
use crate::error::{Result, TuttiError};
use crate::runtime;
use crate::session::TmuxSession;
use crate::state;
use crate::worktree;
use chrono::Utc;
use colored::Colorize;
use std::path::Path;

pub fn run(agent_filter: Option<&str>, workspace_name: Option<&str>, all: bool) -> Result<()> {
    crate::session::tmux::check_tmux()?;

    if all {
        return run_all();
    }

    let (config, config_path) = if let Some(ws) = workspace_name {
        load_workspace_by_name(ws)?
    } else {
        let cwd = std::env::current_dir()?;
        TuttiConfig::load(&cwd)?
    };
    config.validate()?;

    let project_root = config_path.parent().unwrap();
    state::ensure_tutti_dir(project_root)?;

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

    let mut launched = Vec::new();

    for agent in &agents {
        let runtime_name = agent.resolved_runtime(&config.defaults).ok_or_else(|| {
            TuttiError::ConfigValidation(format!(
                "agent '{}' has no runtime (set runtime on agent or in [defaults])",
                agent.name
            ))
        })?;

        let adapter = runtime::get_adapter(&runtime_name)
            .ok_or_else(|| TuttiError::RuntimeUnknown(runtime_name.clone()))?;

        if !adapter.is_available() {
            return Err(TuttiError::RuntimeNotAvailable(runtime_name.clone()));
        }

        let session = TmuxSession::session_name(&config.workspace.name, &agent.name);

        if TmuxSession::session_exists(&session) {
            println!("  {} {} (already running)", "skip".yellow(), agent.name);
            continue;
        }

        // Set up worktree if enabled
        let (working_dir, worktree_path, branch) = if agent.resolved_worktree(&config.defaults) {
            let branch = agent.resolved_branch();
            match worktree::ensure_worktree(project_root, &agent.name, &branch) {
                Ok(wt_path) => {
                    let dir = wt_path.to_str().unwrap().to_string();
                    (dir, Some(wt_path), Some(branch))
                }
                Err(e) => {
                    eprintln!(
                        "  {} worktree for {}: {e} (using project root)",
                        "warn".yellow(),
                        agent.name
                    );
                    let dir = project_root.to_str().unwrap().to_string();
                    (dir, None, None)
                }
            }
        } else {
            let dir = project_root.to_str().unwrap().to_string();
            (dir, None, None)
        };

        let cmd = adapter.build_spawn_command(agent.prompt.as_deref());
        TmuxSession::create_session(&session, &working_dir, &cmd)?;

        // Save state
        let agent_state = state::AgentState {
            name: agent.name.clone(),
            runtime: runtime_name.clone(),
            session_name: session.clone(),
            worktree_path,
            branch,
            status: "Working".to_string(),
            started_at: Utc::now(),
            stopped_at: None,
        };
        state::save_agent_state(project_root, &agent_state)?;

        launched.push((agent.name.clone(), session, runtime_name));
    }

    if launched.is_empty() {
        println!("No agents to launch.");
        return Ok(());
    }

    // Print summary
    println!();
    println!("{}", "Launched agents:".bold());
    print_launch_summary(&launched);
    println!();
    println!(
        "Use {} to see status, {} to connect.",
        "tt status".cyan(),
        "tt attach <agent>".cyan()
    );

    Ok(())
}

fn print_launch_summary(launched: &[(String, String, String)]) {
    use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["Agent", "Runtime", "Session"]);

    for (name, session, runtime) in launched {
        table.add_row(vec![name, runtime, session]);
    }

    println!("{table}");
}

/// Check that git is available (needed for worktrees).
pub fn check_git(project_root: &Path) -> Result<()> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(project_root)
        .output()?;

    if !output.status.success() {
        return Err(TuttiError::Git(
            "not a git repository (worktrees require git)".to_string(),
        ));
    }
    Ok(())
}

/// Load config for a named workspace from the global registry.
pub fn load_workspace_by_name(ws_name: &str) -> Result<(TuttiConfig, std::path::PathBuf)> {
    let global = crate::config::GlobalConfig::load()?;
    let ws = global
        .registered_workspaces
        .iter()
        .find(|w| w.name == ws_name)
        .ok_or_else(|| {
            TuttiError::ConfigValidation(format!(
                "workspace '{ws_name}' not found in global config. Run `tt init` in that project first."
            ))
        })?;
    TuttiConfig::load(&ws.path)
}

/// Launch all agents in all registered workspaces.
fn run_all() -> Result<()> {
    let global = crate::config::GlobalConfig::load()?;
    if global.registered_workspaces.is_empty() {
        println!("No registered workspaces. Run `tt init` in your projects first.");
        return Ok(());
    }

    for ws in &global.registered_workspaces {
        println!("Workspace: {}", ws.name);
        match TuttiConfig::load(&ws.path) {
            Ok((config, config_path)) => {
                if let Err(e) = config.validate() {
                    eprintln!("  Skipping {} (invalid config): {e}", ws.name);
                    continue;
                }
                let project_root = config_path.parent().unwrap();
                state::ensure_tutti_dir(project_root)?;

                for agent in &config.agents {
                    let runtime_name = match agent.resolved_runtime(&config.defaults) {
                        Some(rt) => rt,
                        None => {
                            eprintln!("  Skipping {} (no runtime)", agent.name);
                            continue;
                        }
                    };
                    let adapter = match runtime::get_adapter(&runtime_name) {
                        Some(a) => a,
                        None => {
                            eprintln!(
                                "  Skipping {} (unknown runtime '{runtime_name}')",
                                agent.name
                            );
                            continue;
                        }
                    };
                    if !adapter.is_available() {
                        eprintln!(
                            "  Skipping {} (runtime '{runtime_name}' not installed)",
                            agent.name
                        );
                        continue;
                    }

                    let session = TmuxSession::session_name(&config.workspace.name, &agent.name);
                    if TmuxSession::session_exists(&session) {
                        println!("  skip {} (already running)", agent.name);
                        continue;
                    }

                    let working_dir = project_root.to_str().unwrap().to_string();
                    let cmd = adapter.build_spawn_command(agent.prompt.as_deref());
                    if let Err(e) = TmuxSession::create_session(&session, &working_dir, &cmd) {
                        eprintln!("  Failed to launch {}: {e}", agent.name);
                        continue;
                    }

                    let agent_state = state::AgentState {
                        name: agent.name.clone(),
                        runtime: runtime_name,
                        session_name: session,
                        worktree_path: None,
                        branch: None,
                        status: "Working".to_string(),
                        started_at: Utc::now(),
                        stopped_at: None,
                    };
                    let _ = state::save_agent_state(project_root, &agent_state);
                    println!("  launched {}", agent.name);
                }
            }
            Err(e) => {
                eprintln!("  Skipping {} (config error): {e}", ws.name);
            }
        }
        println!();
    }
    Ok(())
}
