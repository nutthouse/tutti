use crate::config::TuttiConfig;
use crate::error::{Result, TuttiError};
use crate::session::TmuxSession;

pub fn run(agent_ref: &str, prompt_parts: &[String]) -> Result<()> {
    crate::session::tmux::check_tmux()?;

    let prompt = assemble_prompt(prompt_parts)
        .ok_or_else(|| TuttiError::ConfigValidation("prompt cannot be empty".to_string()))?;
    let (workspace_name, agent_name) = resolve_agent_ref(agent_ref)?;
    let session = TmuxSession::session_name(&workspace_name, &agent_name);

    if !TmuxSession::session_exists(&session) {
        return Err(TuttiError::AgentNotRunning(agent_name.to_string()));
    }

    TmuxSession::send_text(&session, &prompt)?;
    println!("sent prompt to {agent_name} ({workspace_name})");

    Ok(())
}

fn assemble_prompt(parts: &[String]) -> Option<String> {
    let joined = parts.join(" ");
    if joined.trim().is_empty() {
        None
    } else {
        Some(joined)
    }
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

#[cfg(test)]
mod tests {
    use super::assemble_prompt;

    #[test]
    fn assemble_prompt_joins_parts() {
        let parts = vec![
            "analyze".to_string(),
            "review".to_string(),
            "ux".to_string(),
        ];
        assert_eq!(
            assemble_prompt(&parts).as_deref(),
            Some("analyze review ux")
        );
    }

    #[test]
    fn assemble_prompt_rejects_empty_input() {
        let parts = Vec::<String>::new();
        assert_eq!(assemble_prompt(&parts), None);
    }

    #[test]
    fn assemble_prompt_rejects_whitespace_only() {
        let parts = vec!["   ".to_string(), "\t".to_string()];
        assert_eq!(assemble_prompt(&parts), None);
    }
}
