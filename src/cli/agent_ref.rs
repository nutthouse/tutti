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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_config(agent_names: &[&str]) -> TuttiConfig {
        let mut toml = String::from("[workspace]\nname = \"demo\"\n\n");
        for agent_name in agent_names {
            toml.push_str("[[agent]]\n");
            toml.push_str(&format!("name = \"{agent_name}\"\n"));
            toml.push_str("runtime = \"claude-code\"\n\n");
        }
        toml::from_str(&toml).unwrap()
    }

    #[test]
    fn agent_config_returns_matching_agent() {
        let resolved = ResolvedAgentRef {
            workspace_name: "demo".to_string(),
            agent_name: "backend".to_string(),
            project_root: PathBuf::from("/tmp/demo"),
            config: parse_config(&["backend", "frontend"]),
        };

        let agent = resolved.agent_config().unwrap();

        assert_eq!(agent.name, "backend");
        assert_eq!(agent.resolved_branch(), "tutti/backend");
    }

    #[test]
    fn agent_config_returns_not_found_for_missing_agent() {
        let resolved = ResolvedAgentRef {
            workspace_name: "demo".to_string(),
            agent_name: "missing".to_string(),
            project_root: PathBuf::from("/tmp/demo"),
            config: parse_config(&["backend"]),
        };

        let err = resolved.agent_config().unwrap_err();

        assert!(matches!(err, TuttiError::AgentNotFound(name) if name == "missing"));
    }
}
