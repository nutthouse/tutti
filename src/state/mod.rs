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
    pub run_id: String,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStepIntentRecord {
    pub run_id: String,
    pub workflow_name: String,
    pub step_index: usize,
    pub step_id: String,
    pub step_type: String,
    pub planned_at: DateTime<Utc>,
    pub intent: Value,
    #[serde(default)]
    pub attempt: u32,
    #[serde(default)]
    pub outcome: Option<WorkflowStepOutcomeRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStepOutcomeRecord {
    pub completed_at: DateTime<Utc>,
    pub status: String,
    pub success: bool,
    #[serde(default)]
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub side_effects: Option<Value>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SdlcRunState {
    Selected,
    Branched,
    Implemented,
    Tested,
    Docs,
    PrOpen,
    Reviewed,
    ReadyToMerge,
    Merged,
}

impl SdlcRunState {
    #[allow(dead_code)]
    fn can_transition_to(&self, next: &SdlcRunState) -> bool {
        matches!(
            (self, next),
            (SdlcRunState::Selected, SdlcRunState::Branched)
                | (SdlcRunState::Branched, SdlcRunState::Implemented)
                | (SdlcRunState::Implemented, SdlcRunState::Tested)
                | (SdlcRunState::Tested, SdlcRunState::Docs)
                | (SdlcRunState::Docs, SdlcRunState::PrOpen)
                | (SdlcRunState::PrOpen, SdlcRunState::Reviewed)
                | (SdlcRunState::Reviewed, SdlcRunState::ReadyToMerge)
                | (SdlcRunState::ReadyToMerge, SdlcRunState::Merged)
        )
    }
}

fn is_valid_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

fn validate_run_id(run_id: &str) -> Result<()> {
    if !is_valid_id(run_id) {
        return Err(TuttiError::State(format!(
            "invalid run_id '{run_id}': only [A-Za-z0-9_-] allowed"
        )));
    }
    Ok(())
}

fn validate_step_id(step_id: &str) -> Result<()> {
    if !is_valid_id(step_id) {
        return Err(TuttiError::State(format!(
            "invalid step_id '{step_id}': only [A-Za-z0-9_-] allowed"
        )));
    }
    Ok(())
}

struct RunLedgerLockGuard {
    path: PathBuf,
}

impl Drop for RunLedgerLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn with_run_ledger_lock<T>(project_root: &Path, op: impl FnOnce() -> Result<T>) -> Result<T> {
    let lock_dir = project_root.join(".tutti").join("state").join("run-ledger");
    std::fs::create_dir_all(&lock_dir)?;
    let lock_path = lock_dir.join(".transition.lock");
    let stale_after = std::time::Duration::from_secs(30);

    let mut acquired = false;
    for _ in 0..50 {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(_) => {
                acquired = true;
                break;
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if let Ok(meta) = std::fs::metadata(&lock_path)
                    && let Ok(modified) = meta.modified()
                    && let Ok(age) = std::time::SystemTime::now().duration_since(modified)
                    && age > stale_after
                {
                    let _ = std::fs::remove_file(&lock_path);
                    continue;
                }
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Err(err) => return Err(err.into()),
        }
    }

    if !acquired {
        return Err(TuttiError::State(
            "timed out waiting for run-ledger transition lock".to_string(),
        ));
    }

    let _guard = RunLedgerLockGuard { path: lock_path };
    op()
}

/// A single state transition recorded for an SDLC run.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdlcTransitionRecord {
    pub from: SdlcRunState,
    pub to: SdlcRunState,
    pub timestamp: DateTime<Utc>,
    pub actor: String,
    #[serde(default)]
    pub reason: Option<String>,
}

/// Persisted state for an SDLC-tracked run, including transition history.
#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SdlcRunLedgerRecord {
    pub run_id: String,
    pub issue_number: u64,
    pub repository: String,
    pub workflow_name: String,
    pub state: SdlcRunState,
    pub updated_at: DateTime<Utc>,
    pub actor: String,
    #[serde(default)]
    pub transitions: Vec<SdlcTransitionRecord>,
}

/// Render a reusable PR comment summary for the provided SDLC run ledger.
///
/// The output includes the current run state and a chronological transition list
/// suitable for posting in PR status updates.
pub fn sdlc_pr_comment_summary(ledger: &SdlcRunLedgerRecord) -> Result<String> {
    let mut out = String::new();
    out.push_str(&format!(
        "SDLC run `{}` for #{} is currently `{:?}` (updated {} by {}).\n",
        ledger.run_id,
        ledger.issue_number,
        ledger.state,
        ledger.updated_at.to_rfc3339(),
        ledger.actor
    ));
    if ledger.transitions.is_empty() {
        out.push_str("No transitions recorded yet.");
        return Ok(out);
    }

    out.push_str("\nTransitions:\n");
    for transition in &ledger.transitions {
        out.push_str(&format!(
            "- {:?} → {:?} @ {} by {}{}\n",
            transition.from,
            transition.to,
            transition.timestamp.to_rfc3339(),
            transition.actor,
            transition
                .reason
                .as_ref()
                .map(|r| format!(" ({r})"))
                .unwrap_or_default()
        ));
    }
    Ok(out.trim_end().to_string())
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolicyDecisionRecord {
    pub timestamp: DateTime<Utc>,
    pub workspace: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub runtime: Option<String>,
    pub action: String,
    pub mode: String,
    pub policy: String,
    pub enforcement: String,
    pub decision: String,
    #[serde(default)]
    pub reason: Option<String>,
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
        "state/workflow-checkpoints",
        "state/workflow-intents",
        "state/workflow-outputs",
        "state/run-ledger",
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
    validate_run_id(run_id)?;
    validate_step_id(step_id)?;
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

pub fn save_workflow_checkpoint(
    project_root: &Path,
    run_id: &str,
    json: &serde_json::Value,
) -> Result<PathBuf> {
    validate_run_id(run_id)?;
    let dir = project_root
        .join(".tutti")
        .join("state")
        .join("workflow-checkpoints");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{run_id}.json"));
    let body = serde_json::to_string_pretty(json)?;
    std::fs::write(&path, body)?;
    Ok(path)
}

pub fn load_workflow_checkpoint(
    project_root: &Path,
    run_id: &str,
) -> Result<Option<serde_json::Value>> {
    validate_run_id(run_id)?;
    let path = project_root
        .join(".tutti")
        .join("state")
        .join("workflow-checkpoints")
        .join(format!("{run_id}.json"));
    if !path.exists() {
        return Ok(None);
    }
    let body = std::fs::read_to_string(path)?;
    let value = serde_json::from_str(&body).map_err(|e| TuttiError::State(e.to_string()))?;
    Ok(Some(value))
}

pub fn save_sdlc_run_ledger(project_root: &Path, ledger: &SdlcRunLedgerRecord) -> Result<PathBuf> {
    validate_run_id(&ledger.run_id)?;
    let dir = project_root.join(".tutti").join("state").join("run-ledger");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", ledger.run_id));
    let nanos = Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let tmp_path = dir.join(format!(
        "{}.json.tmp.{}.{}",
        ledger.run_id,
        std::process::id(),
        nanos
    ));
    let body = serde_json::to_string_pretty(ledger)?;
    std::fs::write(&tmp_path, body)?;
    std::fs::rename(&tmp_path, &path)?;
    Ok(path)
}

#[allow(dead_code)]
pub fn load_sdlc_run_ledger(
    project_root: &Path,
    run_id: &str,
) -> Result<Option<SdlcRunLedgerRecord>> {
    validate_run_id(run_id)?;
    let path = project_root
        .join(".tutti")
        .join("state")
        .join("run-ledger")
        .join(format!("{run_id}.json"));
    if !path.exists() {
        return Ok(None);
    }
    let body = std::fs::read_to_string(path)?;
    let record = serde_json::from_str(&body).map_err(|e| TuttiError::State(e.to_string()))?;
    Ok(Some(record))
}

#[allow(dead_code)]
pub fn transition_sdlc_run_ledger(
    project_root: &Path,
    run_id: &str,
    next: SdlcRunState,
    actor: &str,
    reason: Option<String>,
) -> Result<SdlcRunLedgerRecord> {
    validate_run_id(run_id)?;
    with_run_ledger_lock(project_root, || {
        let mut ledger = load_sdlc_run_ledger(project_root, run_id)?.ok_or_else(|| {
            TuttiError::State(format!("missing SDLC run ledger for run_id '{run_id}'"))
        })?;
        let previous = ledger.state.clone();

        if previous == next {
            return Ok(ledger);
        }

        if !previous.can_transition_to(&next) {
            return Err(TuttiError::State(format!(
                "invalid SDLC transition: {:?} -> {:?}",
                previous, next
            )));
        }

        let now = Utc::now();
        ledger.transitions.push(SdlcTransitionRecord {
            from: previous,
            to: next.clone(),
            timestamp: now,
            actor: actor.to_string(),
            reason,
        });
        ledger.state = next;
        ledger.updated_at = now;
        ledger.actor = actor.to_string();
        save_sdlc_run_ledger(project_root, &ledger)?;
        Ok(ledger)
    })
}

pub fn save_workflow_intent(
    project_root: &Path,
    run_id: &str,
    step_id: &str,
    record: &WorkflowStepIntentRecord,
) -> Result<PathBuf> {
    validate_run_id(run_id)?;
    validate_step_id(step_id)?;
    let dir = project_root
        .join(".tutti")
        .join("state")
        .join("workflow-intents")
        .join(run_id);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{step_id}.json"));
    let body = serde_json::to_string_pretty(record)?;
    std::fs::write(&path, body)?;
    Ok(path)
}

pub fn load_workflow_intent(
    project_root: &Path,
    run_id: &str,
    step_id: &str,
) -> Result<Option<WorkflowStepIntentRecord>> {
    validate_run_id(run_id)?;
    validate_step_id(step_id)?;
    let path = project_root
        .join(".tutti")
        .join("state")
        .join("workflow-intents")
        .join(run_id)
        .join(format!("{step_id}.json"));
    if !path.exists() {
        return Ok(None);
    }
    let body = std::fs::read_to_string(path)?;
    let record = serde_json::from_str(&body).map_err(|e| TuttiError::State(e.to_string()))?;
    Ok(Some(record))
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

pub fn append_policy_decision(project_root: &Path, record: &PolicyDecisionRecord) -> Result<()> {
    let state_dir = project_root.join(".tutti").join("state");
    std::fs::create_dir_all(&state_dir)?;
    let path = state_dir.join("policy-decisions.jsonl");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let line = serde_json::to_string(record)?;
    use std::io::Write;
    writeln!(file, "{line}")?;
    Ok(())
}

pub fn load_policy_decisions(project_root: &Path) -> Result<Vec<PolicyDecisionRecord>> {
    let path = project_root
        .join(".tutti")
        .join("state")
        .join("policy-decisions.jsonl");
    if !path.exists() {
        return Ok(vec![]);
    }
    let body = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    for line in body.lines().filter(|l| !l.trim().is_empty()) {
        if let Ok(record) = serde_json::from_str::<PolicyDecisionRecord>(line) {
            out.push(record);
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
        assert!(dir.join(".tutti/state/workflow-checkpoints").exists());
        assert!(dir.join(".tutti/state/workflow-intents").exists());
        assert!(dir.join(".tutti/state/run-ledger").exists());
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
            run_id: "run123".to_string(),
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
        assert!(contents.contains("\"run_id\":\"run123\""));

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
    fn workflow_checkpoint_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "tutti-test-workflow-checkpoint-{}",
            std::process::id()
        ));
        ensure_tutti_dir(&dir).unwrap();

        let value = serde_json::json!({
            "run_id": "run123",
            "workflow_name": "verify",
            "success": false
        });
        let path = save_workflow_checkpoint(&dir, "run123", &value).unwrap();
        assert!(path.exists());
        let loaded = load_workflow_checkpoint(&dir, "run123").unwrap().unwrap();
        assert_eq!(
            loaded.get("workflow_name").and_then(|v| v.as_str()),
            Some("verify")
        );

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn workflow_intent_round_trip() {
        let dir =
            std::env::temp_dir().join(format!("tutti-test-workflow-intent-{}", std::process::id()));
        ensure_tutti_dir(&dir).unwrap();

        let record = WorkflowStepIntentRecord {
            run_id: "run123".to_string(),
            workflow_name: "verify".to_string(),
            step_index: 1,
            step_id: "step-001".to_string(),
            step_type: "command".to_string(),
            planned_at: Utc::now(),
            intent: serde_json::json!({"run":"echo ok"}),
            attempt: 1,
            outcome: Some(WorkflowStepOutcomeRecord {
                completed_at: Utc::now(),
                status: "success".to_string(),
                success: true,
                exit_code: Some(0),
                timed_out: false,
                message: None,
                side_effects: None,
            }),
        };
        let path = save_workflow_intent(&dir, "run123", "step-001", &record).unwrap();
        assert!(path.exists());
        let loaded = load_workflow_intent(&dir, "run123", "step-001")
            .unwrap()
            .unwrap();
        assert_eq!(loaded.workflow_name, "verify");
        assert_eq!(loaded.step_type, "command");
        assert!(loaded.outcome.as_ref().is_some_and(|o| o.success));

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

    #[test]
    fn policy_decisions_append_and_load() {
        let dir =
            std::env::temp_dir().join(format!("tutti-test-policy-state-{}", std::process::id()));
        ensure_tutti_dir(&dir).unwrap();

        let record = PolicyDecisionRecord {
            timestamp: Utc::now(),
            workspace: "ws".to_string(),
            agent: Some("backend".to_string()),
            runtime: Some("claude-code".to_string()),
            action: "launch".to_string(),
            mode: "auto".to_string(),
            policy: "constrained".to_string(),
            enforcement: "hard".to_string(),
            decision: "allow".to_string(),
            reason: None,
            data: Some(serde_json::json!({"rules": 3})),
        };
        append_policy_decision(&dir, &record).unwrap();
        append_policy_decision(&dir, &record).unwrap();

        let loaded = load_policy_decisions(&dir).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].enforcement, "hard");

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sdlc_ledger_round_trip_and_transition() {
        let dir =
            std::env::temp_dir().join(format!("tutti-test-sdlc-ledger-{}", std::process::id()));
        ensure_tutti_dir(&dir).unwrap();

        let record = SdlcRunLedgerRecord {
            run_id: "run-ledger-1".to_string(),
            issue_number: 30,
            repository: "nutthouse/tutti".to_string(),
            workflow_name: "readiness".to_string(),
            state: SdlcRunState::Selected,
            updated_at: Utc::now(),
            actor: "wren".to_string(),
            transitions: vec![],
        };

        let path = save_sdlc_run_ledger(&dir, &record).unwrap();
        assert!(path.exists());

        let updated = transition_sdlc_run_ledger(
            &dir,
            "run-ledger-1",
            SdlcRunState::Branched,
            "wren",
            Some("created branch".to_string()),
        )
        .unwrap();

        assert_eq!(updated.state, SdlcRunState::Branched);
        assert_eq!(updated.transitions.len(), 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sdlc_ledger_rejects_invalid_transition() {
        let dir = std::env::temp_dir().join(format!(
            "tutti-test-sdlc-ledger-invalid-{}",
            std::process::id()
        ));
        ensure_tutti_dir(&dir).unwrap();

        let record = SdlcRunLedgerRecord {
            run_id: "run-ledger-2".to_string(),
            issue_number: 30,
            repository: "nutthouse/tutti".to_string(),
            workflow_name: "readiness".to_string(),
            state: SdlcRunState::Selected,
            updated_at: Utc::now(),
            actor: "wren".to_string(),
            transitions: vec![],
        };

        save_sdlc_run_ledger(&dir, &record).unwrap();
        let err =
            transition_sdlc_run_ledger(&dir, "run-ledger-2", SdlcRunState::Reviewed, "wren", None)
                .unwrap_err();

        assert!(err.to_string().contains("invalid SDLC transition"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sdlc_ledger_allows_idempotent_transition_retry() {
        let dir = std::env::temp_dir().join(format!(
            "tutti-test-sdlc-ledger-idempotent-{}",
            std::process::id()
        ));
        ensure_tutti_dir(&dir).unwrap();

        let record = SdlcRunLedgerRecord {
            run_id: "run-ledger-3".to_string(),
            issue_number: 30,
            repository: "nutthouse/tutti".to_string(),
            workflow_name: "readiness".to_string(),
            state: SdlcRunState::Branched,
            updated_at: Utc::now(),
            actor: "wren".to_string(),
            transitions: vec![],
        };

        save_sdlc_run_ledger(&dir, &record).unwrap();
        let updated = transition_sdlc_run_ledger(
            &dir,
            "run-ledger-3",
            SdlcRunState::Branched,
            "retry-agent",
            Some("network retry".to_string()),
        )
        .unwrap();

        assert_eq!(updated.state, SdlcRunState::Branched);
        assert!(updated.transitions.is_empty());
        assert_eq!(updated.actor, "wren");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_id_with_path_segments_is_rejected() {
        let dir = std::env::temp_dir().join(format!(
            "tutti-test-runid-validation-{}",
            std::process::id()
        ));
        ensure_tutti_dir(&dir).unwrap();

        let err = load_workflow_checkpoint(&dir, "../escape").unwrap_err();
        assert!(err.to_string().contains("invalid run_id"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn step_id_with_path_segments_is_rejected() {
        let dir = std::env::temp_dir().join(format!(
            "tutti-test-stepid-validation-{}",
            std::process::id()
        ));
        ensure_tutti_dir(&dir).unwrap();

        let payload = serde_json::json!({"ok": true});
        let err = save_workflow_output(&dir, "run123", "../escape", &payload).unwrap_err();
        assert!(err.to_string().contains("invalid step_id"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn run_id_allowlist_rejects_special_chars_and_accepts_safe_values() {
        assert!(validate_run_id("run_123-ABC").is_ok());

        let bad = validate_run_id("run:123").unwrap_err();
        assert!(bad.to_string().contains("only [A-Za-z0-9_-] allowed"));

        let bad_ctrl = validate_run_id("run\n123").unwrap_err();
        assert!(bad_ctrl.to_string().contains("invalid run_id"));
    }

    #[test]
    fn step_id_allowlist_rejects_special_chars_and_accepts_safe_values() {
        assert!(validate_step_id("step_1-OK").is_ok());

        let bad = validate_step_id("step*1").unwrap_err();
        assert!(bad.to_string().contains("only [A-Za-z0-9_-] allowed"));
    }

    #[test]
    fn sdlc_pr_comment_summary_renders_transitions() {
        let now = Utc::now();
        let ledger = SdlcRunLedgerRecord {
            run_id: "run-ledger-summary".to_string(),
            issue_number: 30,
            repository: "nutthouse/tutti".to_string(),
            workflow_name: "readiness".to_string(),
            state: SdlcRunState::Tested,
            updated_at: now,
            actor: "wren".to_string(),
            transitions: vec![SdlcTransitionRecord {
                from: SdlcRunState::Implemented,
                to: SdlcRunState::Tested,
                timestamp: now,
                actor: "wren".to_string(),
                reason: Some("tests passed".to_string()),
            }],
        };

        let summary = sdlc_pr_comment_summary(&ledger).unwrap();
        assert!(summary.contains("run-ledger-summary"));
        assert!(summary.contains("Transitions:"));
        assert!(summary.contains("tests passed"));
    }
}
