use crate::config::TuttiConfig;
use crate::error::{Result, TuttiError};
use crate::session::TmuxSession;

pub fn run(agent_ref: &str, lines: u32) -> Result<()> {
    crate::session::tmux::check_tmux()?;

    let (workspace_name, agent_name) = resolve_agent_ref(agent_ref)?;

    let session = TmuxSession::session_name(&workspace_name, &agent_name);

    if !TmuxSession::session_exists(&session) {
        return Err(TuttiError::AgentNotRunning(agent_name.to_string()));
    }

    let output = TmuxSession::capture_pane(&session, lines)?;
    println!("{output}");

    Ok(())
}

/// Parse "agent" (current workspace) or "workspace/agent" (cross-workspace).
fn resolve_agent_ref(agent_ref: &str) -> Result<(String, String)> {
    if let Some((ws_name, agent_name)) = agent_ref.split_once('/') {
        let (config, _) = super::up::load_workspace_by_name(ws_name)?;
        if !config.agents.iter().any(|a| a.name == agent_name) {
            return Err(TuttiError::AgentNotFound(agent_ref.to_string()));
        }
        Ok((config.workspace.name.clone(), agent_name.to_string()))
    } else {
        let cwd = std::env::current_dir()?;
        let (config, _) = TuttiConfig::load(&cwd)?;
        if !config.agents.iter().any(|a| a.name == agent_ref) {
            return Err(TuttiError::AgentNotFound(agent_ref.to_string()));
        }
        Ok((config.workspace.name.clone(), agent_ref.to_string()))
    }
}
