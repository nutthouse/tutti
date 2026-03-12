use crate::config::TuttiConfig;
use crate::error::{Result, TuttiError};
use crate::session::TmuxSession;

pub fn run(agent_ref: &str) -> Result<()> {
    crate::session::tmux::check_tmux()?;

    let (workspace_name, agent_name, config) = resolve_agent_ref(agent_ref)?;

    let session = TmuxSession::session_name(&workspace_name, &agent_name);

    if !TmuxSession::session_exists(&session) {
        return Err(TuttiError::AgentNotRunning(agent_name.to_string()));
    }

    // Build list of other agents for the status bar hint
    let others: Vec<&str> = config
        .agents
        .iter()
        .filter(|a| a.name != agent_name)
        .map(|a| a.name.as_str())
        .collect();

    let switch_hint = if others.is_empty() {
        String::new()
    } else {
        let first_other = others[0];
        format!(" ── tt attach {first_other}")
    };

    // Set a tmux status bar on the session before attaching
    let status_line = format!(
        " tutti: {} ({}) ── Ctrl+b d to detach{}",
        agent_name, workspace_name, switch_hint
    );

    TmuxSession::set_status_bar(&session, &status_line)?;
    TmuxSession::attach_session(&session)
}

/// Parse "agent" (current workspace) or "workspace/agent" (cross-workspace).
/// Returns (workspace_name, agent_name, config).
fn resolve_agent_ref(agent_ref: &str) -> Result<(String, String, TuttiConfig)> {
    if let Some((ws_name, agent_name)) = agent_ref.split_once('/') {
        let (config, _) = super::up::load_workspace_by_name(ws_name)?;
        if !config.agents.iter().any(|a| a.name == agent_name) {
            return Err(TuttiError::AgentNotFound(agent_ref.to_string()));
        }
        Ok((
            config.workspace.name.clone(),
            agent_name.to_string(),
            config,
        ))
    } else {
        let cwd = std::env::current_dir()?;
        let (config, _) = TuttiConfig::load(&cwd)?;
        if !config.agents.iter().any(|a| a.name == agent_ref) {
            return Err(TuttiError::AgentNotFound(agent_ref.to_string()));
        }
        Ok((config.workspace.name.clone(), agent_ref.to_string(), config))
    }
}
