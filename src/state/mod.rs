use crate::error::{Result, TuttiError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentState {
    pub name: String,
    pub runtime: String,
    pub session_name: String,
    pub worktree_path: Option<PathBuf>,
    pub branch: Option<String>,
    pub status: String,
    pub started_at: DateTime<Utc>,
    pub stopped_at: Option<DateTime<Utc>>,
}

/// Ensure the .tutti/ directory structure exists.
pub fn ensure_tutti_dir(project_root: &Path) -> Result<PathBuf> {
    let tutti_dir = project_root.join(".tutti");
    let subdirs = ["state", "worktrees", "handoffs", "logs"];

    for subdir in &subdirs {
        std::fs::create_dir_all(tutti_dir.join(subdir))?;
    }

    Ok(tutti_dir)
}

/// Save agent state to .tutti/state/{agent}.json.
pub fn save_agent_state(project_root: &Path, state: &AgentState) -> Result<()> {
    let state_dir = project_root.join(".tutti").join("state");
    std::fs::create_dir_all(&state_dir)?;

    let path = state_dir.join(format!("{}.json", state.name));
    let json = serde_json::to_string_pretty(state)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Load agent state from .tutti/state/{agent}.json.
pub fn load_agent_state(project_root: &Path, agent_name: &str) -> Result<Option<AgentState>> {
    let path = project_root
        .join(".tutti")
        .join("state")
        .join(format!("{agent_name}.json"));

    if !path.exists() {
        return Ok(None);
    }

    let contents = std::fs::read_to_string(&path)?;
    let state: AgentState =
        serde_json::from_str(&contents).map_err(|e| TuttiError::State(e.to_string()))?;
    Ok(Some(state))
}

/// Load all agent states from .tutti/state/.
pub fn load_all_states(project_root: &Path) -> Result<Vec<AgentState>> {
    let state_dir = project_root.join(".tutti").join("state");
    if !state_dir.exists() {
        return Ok(vec![]);
    }

    let mut states = Vec::new();
    for entry in std::fs::read_dir(&state_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let contents = std::fs::read_to_string(&path)?;
            if let Ok(state) = serde_json::from_str::<AgentState>(&contents) {
                states.push(state);
            }
        }
    }
    Ok(states)
}

/// Save emergency handoff state when auth failure is detected.
pub fn save_emergency_state(
    project_root: &Path,
    agent_name: &str,
    terminal_output: &str,
    reason: &str,
) -> Result<PathBuf> {
    let handoff_dir = project_root.join(".tutti").join("handoffs");
    std::fs::create_dir_all(&handoff_dir)?;

    let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
    let filename = format!("{agent_name}-emergency-{timestamp}.md");
    let path = handoff_dir.join(&filename);

    let content = format!(
        "# Emergency State: {agent_name}\n\
         \n\
         **Reason:** {reason}\n\
         **Timestamp:** {}\n\
         \n\
         ## Last Terminal Output\n\
         \n\
         ```\n\
         {terminal_output}\n\
         ```\n",
        Utc::now().to_rfc3339(),
    );

    std::fs::write(&path, content)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_agent_state() {
        let dir = std::env::temp_dir().join(format!("tutti-test-state-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        ensure_tutti_dir(&dir).unwrap();

        let state = AgentState {
            name: "backend".to_string(),
            runtime: "claude-code".to_string(),
            session_name: "tutti-test-backend".to_string(),
            worktree_path: Some(dir.join(".tutti/worktrees/backend")),
            branch: Some("tutti/backend".to_string()),
            status: "Working".to_string(),
            started_at: Utc::now(),
            stopped_at: None,
        };

        save_agent_state(&dir, &state).unwrap();
        let loaded = load_agent_state(&dir, "backend").unwrap().unwrap();
        assert_eq!(loaded.name, "backend");
        assert_eq!(loaded.runtime, "claude-code");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_nonexistent_state_returns_none() {
        let dir =
            std::env::temp_dir().join(format!("tutti-test-state-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let result = load_agent_state(&dir, "nonexistent").unwrap();
        assert!(result.is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_all_states_works() {
        let dir = std::env::temp_dir().join(format!("tutti-test-state-all-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        ensure_tutti_dir(&dir).unwrap();

        for name in &["agent1", "agent2"] {
            let state = AgentState {
                name: name.to_string(),
                runtime: "claude-code".to_string(),
                session_name: format!("tutti-test-{name}"),
                worktree_path: None,
                branch: None,
                status: "Working".to_string(),
                started_at: Utc::now(),
                stopped_at: None,
            };
            save_agent_state(&dir, &state).unwrap();
        }

        let all = load_all_states(&dir).unwrap();
        assert_eq!(all.len(), 2);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn ensure_tutti_dir_creates_structure() {
        let dir = std::env::temp_dir().join(format!("tutti-test-dir-{}", std::process::id()));
        ensure_tutti_dir(&dir).unwrap();

        assert!(dir.join(".tutti/state").exists());
        assert!(dir.join(".tutti/worktrees").exists());
        assert!(dir.join(".tutti/handoffs").exists());
        assert!(dir.join(".tutti/logs").exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
