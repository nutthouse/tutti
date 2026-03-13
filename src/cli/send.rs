use crate::config::TuttiConfig;
use crate::error::{Result, TuttiError};
use crate::health;
use crate::session::TmuxSession;
use std::path::PathBuf;
use std::time::Duration;

pub fn run(
    agent_ref: &str,
    prompt_parts: &[String],
    wait: bool,
    timeout_secs: u64,
    idle_stable_secs: u64,
) -> Result<()> {
    crate::session::tmux::check_tmux()?;

    let prompt = assemble_prompt(prompt_parts)
        .ok_or_else(|| TuttiError::ConfigValidation("prompt cannot be empty".to_string()))?;
    let target = resolve_agent_ref(agent_ref)?;
    let workspace_name = target.workspace_name;
    let agent_name = target.agent_name;
    let runtime_name = target.runtime_name;
    let session = TmuxSession::session_name(&workspace_name, &agent_name);

    if !TmuxSession::session_exists(&session) {
        return Err(TuttiError::AgentNotRunning(agent_name.to_string()));
    }

    TmuxSession::send_text(&session, &prompt)?;
    if wait {
        let outcome = health::wait_for_agent_idle(
            &runtime_name,
            &session,
            Duration::from_secs(timeout_secs.max(1)),
            Duration::from_secs(idle_stable_secs.max(1)),
        )?;
        if outcome.timed_out {
            return Err(TuttiError::ConfigValidation(format!(
                "timed out waiting for '{}' to go idle after {}s",
                agent_name,
                timeout_secs.max(1)
            )));
        }
        if let Ok((config, _)) = TuttiConfig::load(&target.project_root) {
            let _ = health::probe_workspace(&config, &target.project_root, 200);
        }
        println!("sent prompt to {agent_name} ({workspace_name}) and wait completed");
    } else {
        println!("sent prompt to {agent_name} ({workspace_name})");
    }

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
fn resolve_agent_ref(agent_ref: &str) -> Result<SendTarget> {
    if let Some((ws_name, agent_name)) = agent_ref.split_once('/') {
        let (config, config_path) = super::up::load_workspace_by_name(ws_name)?;
        let agent = config
            .agents
            .iter()
            .find(|a| a.name == agent_name)
            .ok_or_else(|| TuttiError::AgentNotFound(agent_ref.to_string()))?;
        let runtime_name = agent
            .resolved_runtime(&config.defaults)
            .unwrap_or_else(|| "unknown".to_string());
        let project_root = config_path.parent().ok_or_else(|| {
            TuttiError::ConfigValidation("could not determine workspace root".to_string())
        })?;
        Ok(SendTarget {
            workspace_name: config.workspace.name.clone(),
            agent_name: agent_name.to_string(),
            runtime_name,
            project_root: project_root.to_path_buf(),
        })
    } else {
        let cwd = std::env::current_dir()?;
        let (config, config_path) = TuttiConfig::load(&cwd)?;
        let agent = config
            .agents
            .iter()
            .find(|a| a.name == agent_ref)
            .ok_or_else(|| TuttiError::AgentNotFound(agent_ref.to_string()))?;
        let runtime_name = agent
            .resolved_runtime(&config.defaults)
            .unwrap_or_else(|| "unknown".to_string());
        let project_root = config_path.parent().ok_or_else(|| {
            TuttiError::ConfigValidation("could not determine workspace root".to_string())
        })?;
        Ok(SendTarget {
            workspace_name: config.workspace.name.clone(),
            agent_name: agent_ref.to_string(),
            runtime_name,
            project_root: project_root.to_path_buf(),
        })
    }
}

struct SendTarget {
    workspace_name: String,
    agent_name: String,
    runtime_name: String,
    project_root: PathBuf,
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
