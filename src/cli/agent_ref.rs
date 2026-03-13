use crate::config::{AgentConfig, TuttiConfig};
use crate::error::{Result, TuttiError};
use std::path::PathBuf;

pub(crate) struct ResolvedAgentRef {
    pub workspace_name: String,
    pub agent_name: String,
    pub project_root: PathBuf,
    pub config: TuttiConfig,
}

impl ResolvedAgentRef {
    pub fn agent_config(&self) -> Result<&AgentConfig> {
        self.config
            .agents
            .iter()
            .find(|agent| agent.name == self.agent_name)
            .ok_or_else(|| TuttiError::AgentNotFound(self.agent_name.clone()))
    }
}

/// Parse "agent" (current workspace) or "workspace/agent" (cross-workspace).
pub(crate) fn resolve(agent_ref: &str) -> Result<ResolvedAgentRef> {
    if let Some((ws_name, agent_name)) = agent_ref.split_once('/') {
        let (config, config_path) = super::up::load_workspace_by_name(ws_name)?;
        if !config.agents.iter().any(|a| a.name == agent_name) {
            return Err(TuttiError::AgentNotFound(agent_ref.to_string()));
        }
        let project_root = config_path
            .parent()
            .ok_or_else(|| TuttiError::State("invalid workspace config path".to_string()))?
            .to_path_buf();
        Ok(ResolvedAgentRef {
            workspace_name: config.workspace.name.clone(),
            agent_name: agent_name.to_string(),
            project_root,
            config,
        })
    } else {
        let cwd = std::env::current_dir()?;
        let (config, config_path) = TuttiConfig::load(&cwd)?;
        if !config.agents.iter().any(|a| a.name == agent_ref) {
            return Err(TuttiError::AgentNotFound(agent_ref.to_string()));
        }
        let project_root = config_path
            .parent()
            .ok_or_else(|| TuttiError::State("invalid workspace config path".to_string()))?
            .to_path_buf();
        Ok(ResolvedAgentRef {
            workspace_name: config.workspace.name.clone(),
            agent_name: agent_ref.to_string(),
            project_root,
            config,
        })
    }
}
