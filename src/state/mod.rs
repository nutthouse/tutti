use crate::error::{Result, TuttiError};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationRunRecord {
    pub workflow_name: String,
    pub timestamp: DateTime<Utc>,
    pub trigger: String,
    pub success: bool,
    pub strict: bool,
    pub failed_steps: Vec<usize>,
    pub warning_count: usize,
    pub agent_scope: Option<String>,
    pub hook_event: Option<String>,
    pub hook_agent: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerifyLastSummary {
    pub workflow_name: String,
    pub timestamp: DateTime<Utc>,
    pub success: bool,
    pub failed_steps: Vec<usize>,
    pub strict: bool,
    pub agent_scope: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ActivityState {
    Working,
    Idle,
    Stopped,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AuthState {
    Ok,
    Failed,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentHealth {
    pub workspace: String,
    pub agent: String,
    pub runtime: String,
    pub session_name: String,
    pub running: bool,
    pub activity_state: ActivityState,
    pub auth_state: AuthState,
    pub last_output_change_at: Option<DateTime<Utc>>,
    pub last_probe_at: DateTime<Utc>,
    pub reason: Option<String>,
    #[serde(default)]
    pub pane_hash: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ControlEvent {
    pub event: String,
    pub workspace: String,
    #[serde(default)]
    pub agent: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub correlation_id: String,
    #[serde(default)]
    pub data: Option<Value>,
}

/// Ensure the .tutti/ directory structure exists.
pub fn ensure_tutti_dir(project_root: &Path) -> Result<PathBuf> {
    let tutti_dir = project_root.join(".tutti");
    let subdirs = [
        "state",
        "state/runtime-settings",
        "state/health",
        "state/workflow-outputs",
        "worktrees",
        "handoffs",
        "logs",
    ];

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

/// Append a workflow/hook execution record to .tutti/state/automation-runs.jsonl.
pub fn append_automation_run(project_root: &Path, record: &AutomationRunRecord) -> Result<()> {
    let state_dir = project_root.join(".tutti").join("state");
    std::fs::create_dir_all(&state_dir)?;
    let path = state_dir.join("automation-runs.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::to_string(record)?;
    use std::io::Write;
    writeln!(file, "{line}")?;
    Ok(())
}

/// Save latest verification summary to .tutti/state/verify-last.json.
pub fn save_verify_last_summary(project_root: &Path, summary: &VerifyLastSummary) -> Result<()> {
    let state_dir = project_root.join(".tutti").join("state");
    std::fs::create_dir_all(&state_dir)?;
    let path = state_dir.join("verify-last.json");
    let json = serde_json::to_string_pretty(summary)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Load latest verification summary.
pub fn load_verify_last_summary(project_root: &Path) -> Result<Option<VerifyLastSummary>> {
    let path = project_root
        .join(".tutti")
        .join("state")
        .join("verify-last.json");
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(path)?;
    let summary: VerifyLastSummary =
        serde_json::from_str(&contents).map_err(|e| TuttiError::State(e.to_string()))?;
    Ok(Some(summary))
}

pub fn save_agent_health(project_root: &Path, health: &AgentHealth) -> Result<()> {
    let state_dir = project_root.join(".tutti").join("state").join("health");
    std::fs::create_dir_all(&state_dir)?;
    let path = state_dir.join(format!("{}.json", health.agent));
    let json = serde_json::to_string_pretty(health)?;
    std::fs::write(path, json)?;
    Ok(())
}

pub fn load_agent_health(project_root: &Path, agent_name: &str) -> Result<Option<AgentHealth>> {
    let path = project_root
        .join(".tutti")
        .join("state")
        .join("health")
        .join(format!("{agent_name}.json"));
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(path)?;
    let health: AgentHealth =
        serde_json::from_str(&contents).map_err(|e| TuttiError::State(e.to_string()))?;
    Ok(Some(health))
}

pub fn load_all_health(project_root: &Path) -> Result<Vec<AgentHealth>> {
    let dir = project_root.join(".tutti").join("state").join("health");
    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut out = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "json") {
            let contents = std::fs::read_to_string(path)?;
            if let Ok(health) = serde_json::from_str::<AgentHealth>(&contents) {
                out.push(health);
            }
        }
    }
    out.sort_by(|a, b| a.agent.cmp(&b.agent));
    Ok(out)
}

pub fn save_scheduler_last_runs(
    project_root: &Path,
    map: &HashMap<String, DateTime<Utc>>,
) -> Result<()> {
    let state_dir = project_root.join(".tutti").join("state");
    std::fs::create_dir_all(&state_dir)?;
    let path = state_dir.join("scheduler-last-runs.json");
    let json = serde_json::to_string_pretty(map)?;
    std::fs::write(path, json)?;
    Ok(())
}

pub fn load_scheduler_last_runs(project_root: &Path) -> Result<HashMap<String, DateTime<Utc>>> {
    let path = project_root
        .join(".tutti")
        .join("state")
        .join("scheduler-last-runs.json");
    if !path.exists() {
        return Ok(HashMap::new());
    }
    let contents = std::fs::read_to_string(path)?;
    let parsed = serde_json::from_str::<HashMap<String, DateTime<Utc>>>(&contents)
        .map_err(|e| TuttiError::State(e.to_string()))?;
    Ok(parsed)
}

pub fn save_workflow_output(
    project_root: &Path,
    run_id: &str,
    step_id: &str,
    json: &serde_json::Value,
) -> Result<PathBuf> {
    let dir = project_root
        .join(".tutti")
        .join("state")
        .join("workflow-outputs")
        .join(run_id);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{step_id}.json"));
    let body = serde_json::to_string_pretty(json)?;
    std::fs::write(&path, body)?;
    Ok(path)
}

pub fn append_control_event(project_root: &Path, event: &ControlEvent) -> Result<()> {
    let state_dir = project_root.join(".tutti").join("state");
    std::fs::create_dir_all(&state_dir)?;
    let path = state_dir.join("events.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::to_string(event)?;
    use std::io::Write;
    writeln!(file, "{line}")?;
    Ok(())
}

pub fn load_control_events(project_root: &Path) -> Result<Vec<ControlEvent>> {
    let path = project_root
        .join(".tutti")
        .join("state")
        .join("events.jsonl");
    if !path.exists() {
        return Ok(vec![]);
    }
    let body = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for line in body.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(event) = serde_json::from_str::<ControlEvent>(line) {
            out.push(event);
        }
    }
    Ok(out)
}

/// Load all agent states from .tutti/state/.
#[cfg(test)]
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

/// Update the status field of an existing agent state file, if it exists.
pub fn update_status_if_exists(project_root: &Path, agent_name: &str, status: &str) -> Result<()> {
    if let Some(mut state) = load_agent_state(project_root, agent_name)? {
        state.status = status.to_string();
        save_agent_state(project_root, &state)?;
    }
    Ok(())
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
        assert!(dir.join(".tutti/state/runtime-settings").exists());
        assert!(dir.join(".tutti/worktrees").exists());
        assert!(dir.join(".tutti/handoffs").exists());
        assert!(dir.join(".tutti/logs").exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn automation_runs_jsonl_is_appended() {
        let dir = std::env::temp_dir().join(format!(
            "tutti-test-automation-state-{}",
            std::process::id()
        ));
        ensure_tutti_dir(&dir).unwrap();

        let record = AutomationRunRecord {
            workflow_name: "verify".to_string(),
            timestamp: Utc::now(),
            trigger: "run".to_string(),
            success: true,
            strict: false,
            failed_steps: vec![],
            warning_count: 1,
            agent_scope: Some("backend".to_string()),
            hook_event: None,
            hook_agent: None,
        };
        append_automation_run(&dir, &record).unwrap();
        append_automation_run(&dir, &record).unwrap();

        let path = dir
            .join(".tutti")
            .join("state")
            .join("automation-runs.jsonl");
        let contents = std::fs::read_to_string(path).unwrap();
        assert_eq!(contents.lines().count(), 2);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn verify_last_summary_round_trip() {
        let dir =
            std::env::temp_dir().join(format!("tutti-test-verify-state-{}", std::process::id()));
        ensure_tutti_dir(&dir).unwrap();

        let summary = VerifyLastSummary {
            workflow_name: "verify".to_string(),
            timestamp: Utc::now(),
            success: false,
            failed_steps: vec![2],
            strict: true,
            agent_scope: Some("backend".to_string()),
        };

        save_verify_last_summary(&dir, &summary).unwrap();
        let loaded = load_verify_last_summary(&dir).unwrap().unwrap();
        assert_eq!(loaded.workflow_name, "verify");
        assert_eq!(loaded.failed_steps, vec![2]);
        assert!(loaded.strict);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn health_round_trip() {
        let dir =
            std::env::temp_dir().join(format!("tutti-test-health-state-{}", std::process::id()));
        ensure_tutti_dir(&dir).unwrap();

        let health = AgentHealth {
            workspace: "ws".to_string(),
            agent: "backend".to_string(),
            runtime: "claude-code".to_string(),
            session_name: "tutti-ws-backend".to_string(),
            running: true,
            activity_state: ActivityState::Working,
            auth_state: AuthState::Ok,
            last_output_change_at: Some(Utc::now()),
            last_probe_at: Utc::now(),
            reason: None,
            pane_hash: Some(123),
        };

        save_agent_health(&dir, &health).unwrap();
        let loaded = load_agent_health(&dir, "backend").unwrap().unwrap();
        assert_eq!(loaded.agent, "backend");
        assert_eq!(loaded.activity_state, ActivityState::Working);

        let all = load_all_health(&dir).unwrap();
        assert_eq!(all.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn scheduler_last_runs_round_trip() {
        let dir =
            std::env::temp_dir().join(format!("tutti-test-scheduler-state-{}", std::process::id()));
        ensure_tutti_dir(&dir).unwrap();

        let mut map = HashMap::new();
        map.insert("ws/verify".to_string(), Utc::now());
        save_scheduler_last_runs(&dir, &map).unwrap();
        let loaded = load_scheduler_last_runs(&dir).unwrap();
        assert_eq!(loaded.len(), 1);
        assert!(loaded.contains_key("ws/verify"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn workflow_output_is_persisted() {
        let dir =
            std::env::temp_dir().join(format!("tutti-test-workflow-output-{}", std::process::id()));
        ensure_tutti_dir(&dir).unwrap();

        let value = serde_json::json!({"ok": true});
        let path = save_workflow_output(&dir, "run123", "scan", &value).unwrap();
        assert!(path.exists());
        let body = std::fs::read_to_string(path).unwrap();
        assert!(body.contains("\"ok\": true"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn control_events_append_and_load() {
        let dir =
            std::env::temp_dir().join(format!("tutti-test-events-state-{}", std::process::id()));
        ensure_tutti_dir(&dir).unwrap();

        let event = ControlEvent {
            event: "agent.started".to_string(),
            workspace: "ws".to_string(),
            agent: Some("backend".to_string()),
            timestamp: Utc::now(),
            correlation_id: "abc123".to_string(),
            data: Some(serde_json::json!({"runtime":"claude-code"})),
        };
        append_control_event(&dir, &event).unwrap();
        append_control_event(&dir, &event).unwrap();

        let loaded = load_control_events(&dir).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].event, "agent.started");

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
