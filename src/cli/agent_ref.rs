use crate::config::{AgentConfig, TuttiConfig};
use crate::error::{Result, TuttiError};
use std::path::PathBuf;

#[derive(Debug)]
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
    use serial_test::serial;
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct CurrentDirGuard {
        original: PathBuf,
    }

    impl CurrentDirGuard {
        fn change_to(path: &Path) -> Self {
            let original = std::env::current_dir().expect("current dir");
            std::env::set_current_dir(path).expect("set current dir");
            Self { original }
        }
    }

    impl Drop for CurrentDirGuard {
        fn drop(&mut self) {
            std::env::set_current_dir(&self.original).expect("restore current dir");
        }
    }

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("{prefix}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write_workspace_config(dir: &Path, workspace_name: &str) {
        let config = format!(
            r#"[workspace]
name = "{workspace_name}"

[[agent]]
name = "backend"
runtime = "codex"

[[agent]]
name = "reviewer"
runtime = "claude-code"
"#
        );
        fs::write(dir.join("tutti.toml"), config).expect("write config");
    }

    #[test]
    fn agent_config_returns_missing_agent_error() {
        let config: TuttiConfig = toml::from_str(
            r#"[workspace]
name = "ws"

[[agent]]
name = "backend"
"#,
        )
        .expect("parse config");
        let resolved = ResolvedAgentRef {
            workspace_name: "ws".to_string(),
            agent_name: "reviewer".to_string(),
            project_root: PathBuf::from("/tmp/ws"),
            config,
        };

        let err = resolved.agent_config().expect_err("missing agent should fail");
        assert!(matches!(err, TuttiError::AgentNotFound(agent) if agent == "reviewer"));
    }

    #[test]
    #[serial]
    fn resolve_loads_agent_from_current_workspace() {
        let dir = unique_temp_dir("tutti-agent-ref-current");
        write_workspace_config(&dir, "alpha");

        {
            let _cwd = CurrentDirGuard::change_to(&dir);
            let resolved = resolve("backend").expect("resolve current workspace agent");
            let agent = resolved.agent_config().expect("agent config");

            assert_eq!(resolved.workspace_name, "alpha");
            assert_eq!(resolved.agent_name, "backend");
            assert_eq!(
                resolved.project_root.canonicalize().expect("canonical resolved path"),
                dir.canonicalize().expect("canonical temp dir")
            );
            assert_eq!(agent.name, "backend");
            assert_eq!(agent.runtime.as_deref(), Some("codex"));
        }

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    #[serial]
    fn resolve_rejects_unknown_agent_in_current_workspace() {
        let dir = unique_temp_dir("tutti-agent-ref-missing");
        write_workspace_config(&dir, "alpha");

        {
            let _cwd = CurrentDirGuard::change_to(&dir);
            let err = resolve("missing").expect_err("missing agent should fail");
            assert!(matches!(err, TuttiError::AgentNotFound(agent) if agent == "missing"));
        }

        let _ = fs::remove_dir_all(&dir);
    }
}
