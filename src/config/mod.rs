use crate::error::{Result, TuttiError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

pub mod defaults;

// ── Per-project config (tutti.toml) ──

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuttiConfig {
    pub workspace: WorkspaceConfig,
    #[serde(default)]
    pub defaults: DefaultsConfig,
    #[serde(default)]
    pub launch: Option<LaunchConfig>,
    #[serde(default, rename = "agent")]
    pub agents: Vec<AgentConfig>,
    #[serde(default, rename = "tool_pack")]
    pub tool_packs: Vec<ToolPackConfig>,
    #[serde(default, rename = "workflow")]
    pub workflows: Vec<WorkflowConfig>,
    #[serde(default, rename = "hook")]
    pub hooks: Vec<HookConfig>,
    #[serde(default)]
    pub handoff: Option<HandoffConfig>,
    #[serde(default)]
    pub observe: Option<ObserveConfig>,
    #[serde(default)]
    pub budget: Option<BudgetConfig>,
    #[serde(default, rename = "webhook")]
    pub webhooks: Vec<WebhookConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceConfig {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub env: Option<WorkspaceEnv>,
    #[serde(default)]
    pub auth: Option<WorkspaceAuth>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkspaceEnv {
    #[serde(default)]
    pub git_name: Option<String>,
    #[serde(default)]
    pub git_email: Option<String>,
    /// Additional environment variables (flattened)
    #[serde(flatten)]
    pub extra: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WorkspaceAuth {
    #[serde(default)]
    pub default_profile: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DefaultsConfig {
    #[serde(default = "default_true")]
    pub worktree: bool,
    #[serde(default)]
    pub runtime: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LaunchMode {
    Safe,
    Auto,
    Unattended,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LaunchPolicyMode {
    Constrained,
    Bypass,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchConfig {
    #[serde(default = "default_launch_mode")]
    pub mode: LaunchMode,
    #[serde(default = "default_launch_policy_mode")]
    pub policy: LaunchPolicyMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub name: String,
    #[serde(default)]
    pub runtime: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub depends_on: Vec<String>,
    #[serde(default)]
    pub worktree: Option<bool>,
    #[serde(default)]
    pub fresh_worktree: Option<bool>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default)]
    pub persistent: bool,
    /// Path to a persistent memory file (relative to project root).
    #[serde(default)]
    pub memory: Option<String>,
    /// Agent-level environment variables (override workspace env).
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolPackConfig {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub required_commands: Vec<String>,
    #[serde(default)]
    pub required_env: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowConfig {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub schedule: Option<String>,
    #[serde(default, rename = "step")]
    pub steps: Vec<WorkflowStepConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WorkflowStepConfig {
    Prompt {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        depends_on: Vec<usize>,
        agent: String,
        text: String,
        #[serde(default)]
        inject_files: Vec<String>,
        #[serde(default)]
        output_json: Option<String>,
        #[serde(default)]
        wait_for_idle: Option<bool>,
        #[serde(default)]
        wait_timeout_secs: Option<u64>,
        #[serde(default)]
        startup_grace_secs: Option<u64>,
    },
    Command {
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        depends_on: Vec<usize>,
        run: String,
        #[serde(default)]
        cwd: Option<WorkflowCommandCwd>,
        #[serde(default)]
        subdir: Option<String>,
        #[serde(default)]
        agent: Option<String>,
        #[serde(default)]
        timeout_secs: Option<u64>,
        #[serde(default)]
        fail_mode: Option<WorkflowFailMode>,
        #[serde(default)]
        output_json: Option<String>,
    },
    EnsureRunning {
        #[serde(default)]
        depends_on: Vec<usize>,
        agent: String,
        #[serde(default)]
        fail_mode: Option<WorkflowFailMode>,
    },
    Workflow {
        #[serde(default)]
        depends_on: Vec<usize>,
        workflow: String,
        #[serde(default)]
        agent: Option<String>,
        #[serde(default)]
        strict: Option<bool>,
        #[serde(default)]
        fail_mode: Option<WorkflowFailMode>,
    },
    Land {
        #[serde(default)]
        depends_on: Vec<usize>,
        agent: String,
        #[serde(default)]
        pr: Option<bool>,
        #[serde(default)]
        force: Option<bool>,
        #[serde(default)]
        fail_mode: Option<WorkflowFailMode>,
    },
    Review {
        #[serde(default)]
        depends_on: Vec<usize>,
        agent: String,
        #[serde(default)]
        reviewer: Option<String>,
        #[serde(default)]
        fail_mode: Option<WorkflowFailMode>,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowCommandCwd {
    Workspace,
    AgentWorktree,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowFailMode {
    Open,
    Closed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookConfig {
    pub event: HookEvent,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub workflow_source: Option<HookWorkflowSource>,
    #[serde(default)]
    pub workflow_name: Option<String>,
    #[serde(default)]
    pub workflow: Option<String>,
    #[serde(default)]
    pub run: Option<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub fail_mode: Option<WorkflowFailMode>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookEvent {
    AgentStop,
    WorkflowComplete,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookWorkflowSource {
    Run,
    Verify,
    HookAgentStop,
    ObserveCycle,
    HookWorkflowComplete,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandoffConfig {
    #[serde(default)]
    pub auto: bool,
    #[serde(default = "default_threshold")]
    pub threshold: f64,
    #[serde(default)]
    pub include: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObserveConfig {
    /// Reserved for future web dashboard plumbing; currently parsed but not acted on.
    #[serde(default)]
    pub dashboard: bool,
    /// Reserved for future web dashboard server binding.
    #[serde(default = "default_port")]
    pub port: u16,
    /// Reserved for future dashboard cost overlays.
    #[serde(default)]
    pub track_cost: bool,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BudgetMode {
    Warn,
    Enforce,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    #[serde(default = "default_budget_mode")]
    pub mode: BudgetMode,
    #[serde(default = "default_budget_warn_threshold_pct")]
    pub warn_threshold_pct: f64,
    #[serde(default)]
    pub workspace_weekly_tokens: Option<u64>,
    #[serde(default)]
    pub agent_weekly_tokens: HashMap<String, u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebhookConfig {
    /// Source identifier (e.g. "github", "slack", "generic")
    pub source: String,
    /// Event types to match (e.g. ["issues.labeled", "push"]). Use ["*"] for all.
    #[serde(default)]
    pub events: Vec<String>,
    /// Workflow name to trigger on match
    #[serde(default)]
    pub workflow: Option<String>,
    /// Agent to send a prompt to on match (alternative to workflow)
    #[serde(default)]
    pub agent: Option<String>,
    /// Prompt to send when using agent dispatch
    #[serde(default)]
    pub prompt: Option<String>,
}

// ── Global config (~/.config/tutti/config.toml) ──

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct GlobalConfig {
    #[serde(default)]
    pub user: Option<GlobalUser>,
    #[serde(default, rename = "profile")]
    pub profiles: Vec<ProfileConfig>,
    #[serde(default, rename = "registered_workspace")]
    pub registered_workspaces: Vec<RegisteredWorkspace>,
    #[serde(default)]
    pub dashboard: Option<DashboardConfig>,
    #[serde(default)]
    pub resilience: Option<ResilienceConfig>,
    #[serde(default)]
    pub permissions: Option<PermissionsConfig>,
    #[serde(default)]
    pub serve: Option<ServeConfig>,
    #[serde(default, rename = "remote")]
    pub remotes: Vec<RemoteEntry>,
}

/// A registered remote tutti host persisted in `[[remote]]` config blocks.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteEntry {
    pub name: String,
    pub host: String,
    #[serde(default = "default_remote_port")]
    pub port: u16,
    #[serde(default)]
    pub token: Option<String>,
}

fn default_remote_port() -> u16 {
    4040
}

/// Configuration for `tt serve` remote access
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServeConfig {
    /// Bind address (default: 127.0.0.1)
    #[serde(default = "default_serve_bind")]
    pub bind: String,
    /// Authentication mode: "none" or "bearer"
    #[serde(default)]
    pub auth: ServeAuthMode,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServeAuthMode {
    #[default]
    None,
    Bearer,
}

fn default_serve_bind() -> String {
    "127.0.0.1".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalUser {
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProfileConfig {
    pub name: String,
    pub provider: String,
    pub command: String,
    #[serde(default)]
    pub max_concurrent: Option<u32>,
    #[serde(default)]
    pub monthly_budget: Option<f64>,
    #[serde(default)]
    pub priority: Option<u32>,
    /// Subscription plan: "free", "pro", "max", "team", "api"
    #[serde(default)]
    pub plan: Option<String>,
    /// Weekly reset day: "monday", "tuesday", etc.
    #[serde(default)]
    pub reset_day: Option<String>,
    /// Weekly capacity ceiling in compute-hours (e.g. 45.0 for Max)
    #[serde(default)]
    pub weekly_hours: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredWorkspace {
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardConfig {
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default)]
    pub show_all_workspaces: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResilienceConfig {
    #[serde(default)]
    pub provider_down_strategy: Option<String>,
    #[serde(default)]
    pub save_state_on_failure: bool,
    #[serde(default)]
    pub rate_limit_strategy: Option<String>,
    #[serde(default)]
    pub retry_max_attempts: Option<u32>,
    #[serde(default)]
    pub retry_initial_backoff_ms: Option<u64>,
    #[serde(default)]
    pub retry_max_backoff_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PermissionsConfig {
    #[serde(default)]
    pub allow: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn default_threshold() -> f64 {
    0.2
}

fn default_launch_mode() -> LaunchMode {
    LaunchMode::Auto
}

fn default_launch_policy_mode() -> LaunchPolicyMode {
    LaunchPolicyMode::Constrained
}

fn default_port() -> u16 {
    4040
}

fn default_budget_mode() -> BudgetMode {
    BudgetMode::Warn
}

fn default_budget_warn_threshold_pct() -> f64 {
    80.0
}

fn validate_schedule_expression(expr: &str) -> std::result::Result<(), String> {
    let fields = expr.split_whitespace().count();
    if fields != 5 {
        return Err("expected 5-field cron expression (minute hour dom month dow)".to_string());
    }
    crate::scheduler::parse_schedule(expr)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn validate_step_id_and_output(
    workflow_name: &str,
    step_index: usize,
    step_id: Option<&str>,
    output_json: Option<&str>,
    seen_ids: &mut std::collections::HashSet<String>,
) -> Result<()> {
    if output_json.is_some() && step_id.is_none() {
        return Err(TuttiError::ConfigValidation(format!(
            "workflow '{}', step {} uses output_json but is missing id",
            workflow_name, step_index
        )));
    }

    if let Some(id) = step_id {
        let trimmed = id.trim();
        if trimmed.is_empty() {
            return Err(TuttiError::ConfigValidation(format!(
                "workflow '{}', step {} has empty id",
                workflow_name, step_index
            )));
        }
        if !seen_ids.insert(trimmed.to_string()) {
            return Err(TuttiError::ConfigValidation(format!(
                "workflow '{}' has duplicate step id '{}'",
                workflow_name, trimmed
            )));
        }
    }

    if let Some(path) = output_json
        && path.trim().is_empty()
    {
        return Err(TuttiError::ConfigValidation(format!(
            "workflow '{}', step {} has empty output_json",
            workflow_name, step_index
        )));
    }

    Ok(())
}

fn step_depends_on(step: &WorkflowStepConfig) -> &[usize] {
    match step {
        WorkflowStepConfig::Prompt { depends_on, .. } => depends_on,
        WorkflowStepConfig::Command { depends_on, .. } => depends_on,
        WorkflowStepConfig::EnsureRunning { depends_on, .. } => depends_on,
        WorkflowStepConfig::Workflow { depends_on, .. } => depends_on,
        WorkflowStepConfig::Land { depends_on, .. } => depends_on,
        WorkflowStepConfig::Review { depends_on, .. } => depends_on,
    }
}

fn step_is_control(step: &WorkflowStepConfig) -> bool {
    matches!(
        step,
        WorkflowStepConfig::EnsureRunning { .. }
            | WorkflowStepConfig::Review { .. }
            | WorkflowStepConfig::Land { .. }
    )
}

impl AgentConfig {
    pub fn resolved_runtime(&self, defaults: &DefaultsConfig) -> Option<String> {
        self.runtime.clone().or_else(|| defaults.runtime.clone())
    }

    pub fn resolved_worktree(&self, defaults: &DefaultsConfig) -> bool {
        self.worktree.unwrap_or(defaults.worktree)
    }

    pub fn resolved_fresh_worktree(&self) -> bool {
        self.fresh_worktree.unwrap_or(false)
    }

    pub fn resolved_branch(&self) -> String {
        self.branch
            .clone()
            .unwrap_or_else(|| format!("tutti/{}", self.name))
    }
}

impl TuttiConfig {
    /// Walk up from `start_dir` to find tutti.toml, then parse it.
    pub fn load(start_dir: &Path) -> Result<(Self, PathBuf)> {
        let config_path = find_config(start_dir)?;
        let contents = std::fs::read_to_string(&config_path)?;
        let config: TuttiConfig =
            toml::from_str(&contents).map_err(|e| TuttiError::ConfigParse(e.to_string()))?;
        Ok((config, config_path))
    }

    /// Validate the config for logical errors.
    pub fn validate(&self) -> Result<()> {
        // Check for duplicate agent names
        let mut seen = std::collections::HashSet::new();
        for agent in &self.agents {
            if !seen.insert(&agent.name) {
                return Err(TuttiError::ConfigValidation(format!(
                    "duplicate agent name: '{}'",
                    agent.name
                )));
            }
        }

        // Check depends_on references exist
        let names: std::collections::HashSet<&str> =
            self.agents.iter().map(|a| a.name.as_str()).collect();
        for agent in &self.agents {
            for dep in &agent.depends_on {
                if !names.contains(dep.as_str()) {
                    return Err(TuttiError::ConfigValidation(format!(
                        "agent '{}' depends on '{}', which does not exist",
                        agent.name, dep
                    )));
                }
            }
            if agent.depends_on.contains(&agent.name) {
                return Err(TuttiError::ConfigValidation(format!(
                    "agent '{}' depends on itself",
                    agent.name
                )));
            }
        }

        // Check for dependency cycles
        topological_sort(&self.agents)?;

        // Check runtimes are known
        let known_runtimes = ["claude-code", "codex", "aider", "openclaw"];
        for agent in &self.agents {
            if let Some(rt) = agent.resolved_runtime(&self.defaults)
                && !known_runtimes.contains(&rt.as_str())
            {
                return Err(TuttiError::ConfigValidation(format!(
                    "agent '{}' uses unknown runtime '{rt}'",
                    agent.name
                )));
            }
        }

        // Validate memory paths are relative and don't escape project root
        for agent in &self.agents {
            if let Some(ref memory) = agent.memory {
                let trimmed = memory.trim();
                if trimmed.is_empty() {
                    return Err(TuttiError::ConfigValidation(format!(
                        "agent '{}' has empty memory path",
                        agent.name
                    )));
                }
                if Path::new(trimmed).is_absolute() {
                    return Err(TuttiError::ConfigValidation(format!(
                        "agent '{}' memory path must be relative: '{trimmed}'",
                        agent.name
                    )));
                }
                if Path::new(trimmed)
                    .components()
                    .any(|c| matches!(c, Component::ParentDir))
                {
                    return Err(TuttiError::ConfigValidation(format!(
                        "agent '{}' memory path must not contain '..': '{trimmed}'",
                        agent.name
                    )));
                }
            }
        }

        self.validate_automation()?;
        self.validate_tool_packs()?;
        self.validate_budget()?;
        self.validate_webhooks()?;

        Ok(())
    }

    fn validate_webhooks(&self) -> Result<()> {
        let agent_names: std::collections::HashSet<&str> =
            self.agents.iter().map(|a| a.name.as_str()).collect();
        let workflow_names: std::collections::HashSet<&str> =
            self.workflows.iter().map(|w| w.name.as_str()).collect();

        for (i, wh) in self.webhooks.iter().enumerate() {
            if wh.source.trim().is_empty() {
                return Err(TuttiError::ConfigValidation(format!(
                    "webhook[{i}] source cannot be empty"
                )));
            }
            if wh.workflow.is_none() && wh.agent.is_none() {
                return Err(TuttiError::ConfigValidation(format!(
                    "webhook[{i}] (source '{}') must specify either 'workflow' or 'agent'",
                    wh.source
                )));
            }
            if let Some(ref workflow) = wh.workflow
                && !workflow_names.contains(workflow.as_str())
            {
                return Err(TuttiError::ConfigValidation(format!(
                    "webhook[{i}] references unknown workflow '{workflow}'"
                )));
            }
            if let Some(ref agent) = wh.agent
                && !agent_names.contains(agent.as_str())
            {
                return Err(TuttiError::ConfigValidation(format!(
                    "webhook[{i}] references unknown agent '{agent}'"
                )));
            }
        }
        Ok(())
    }

    fn validate_automation(&self) -> Result<()> {
        let agent_names: std::collections::HashSet<&str> =
            self.agents.iter().map(|a| a.name.as_str()).collect();

        let mut workflow_names = std::collections::HashSet::new();
        for workflow in &self.workflows {
            if workflow.name.trim().is_empty() {
                return Err(TuttiError::ConfigValidation(
                    "workflow name cannot be empty".to_string(),
                ));
            }
            if !workflow_names.insert(workflow.name.as_str()) {
                return Err(TuttiError::ConfigValidation(format!(
                    "duplicate workflow name: '{}'",
                    workflow.name
                )));
            }
        }

        for workflow in &self.workflows {
            if workflow.steps.is_empty() {
                return Err(TuttiError::ConfigValidation(format!(
                    "workflow '{}' must have at least one step",
                    workflow.name
                )));
            }

            if let Some(schedule) = workflow.schedule.as_deref() {
                let trimmed = schedule.trim();
                if trimmed.is_empty() {
                    return Err(TuttiError::ConfigValidation(format!(
                        "workflow '{}' has empty schedule",
                        workflow.name
                    )));
                }
                validate_schedule_expression(trimmed).map_err(|e| {
                    TuttiError::ConfigValidation(format!(
                        "workflow '{}' has invalid schedule '{}': {e}",
                        workflow.name, trimmed
                    ))
                })?;
            }

            let mut step_ids = std::collections::HashSet::new();
            for (idx, step) in workflow.steps.iter().enumerate() {
                match step {
                    WorkflowStepConfig::Prompt {
                        id,
                        agent,
                        text,
                        inject_files,
                        output_json,
                        wait_for_idle,
                        wait_timeout_secs,
                        startup_grace_secs,
                        ..
                    } => {
                        if !wait_for_idle.unwrap_or(false)
                            && (wait_timeout_secs.is_some() || startup_grace_secs.is_some())
                        {
                            return Err(TuttiError::ConfigValidation(format!(
                                "workflow '{}', step {} sets wait_timeout_secs/startup_grace_secs but wait_for_idle is false; set wait_for_idle = true or remove the wait settings",
                                workflow.name,
                                idx + 1
                            )));
                        }
                        if !agent_names.contains(agent.as_str()) {
                            return Err(TuttiError::ConfigValidation(format!(
                                "workflow '{}', step {} references unknown agent '{}'",
                                workflow.name,
                                idx + 1,
                                agent
                            )));
                        }
                        if text.trim().is_empty() {
                            return Err(TuttiError::ConfigValidation(format!(
                                "workflow '{}', step {} has empty prompt text",
                                workflow.name,
                                idx + 1
                            )));
                        }
                        for path in inject_files {
                            let trimmed = path.trim();
                            if trimmed.is_empty() {
                                return Err(TuttiError::ConfigValidation(format!(
                                    "workflow '{}', step {} has empty inject_files entry",
                                    workflow.name,
                                    idx + 1
                                )));
                            }
                            if std::path::Path::new(trimmed).is_absolute() {
                                return Err(TuttiError::ConfigValidation(format!(
                                    "workflow '{}', step {} inject_files must be workspace-relative: '{}'",
                                    workflow.name,
                                    idx + 1,
                                    trimmed
                                )));
                            }
                        }
                        validate_step_id_and_output(
                            &workflow.name,
                            idx + 1,
                            id.as_deref(),
                            output_json.as_deref(),
                            &mut step_ids,
                        )?;
                    }
                    WorkflowStepConfig::Command {
                        id,
                        run,
                        subdir,
                        output_json,
                        ..
                    } => {
                        if run.trim().is_empty() {
                            return Err(TuttiError::ConfigValidation(format!(
                                "workflow '{}', step {} has empty command",
                                workflow.name,
                                idx + 1
                            )));
                        }
                        if let Some(subdir) = subdir.as_deref() {
                            let trimmed = subdir.trim();
                            if trimmed.is_empty() {
                                return Err(TuttiError::ConfigValidation(format!(
                                    "workflow '{}', step {} has empty subdir",
                                    workflow.name,
                                    idx + 1
                                )));
                            }
                            if std::path::Path::new(trimmed).is_absolute() {
                                return Err(TuttiError::ConfigValidation(format!(
                                    "workflow '{}', step {} subdir must be workspace-relative: '{}'",
                                    workflow.name,
                                    idx + 1,
                                    trimmed
                                )));
                            }
                        }
                        validate_step_id_and_output(
                            &workflow.name,
                            idx + 1,
                            id.as_deref(),
                            output_json.as_deref(),
                            &mut step_ids,
                        )?;
                    }
                    WorkflowStepConfig::EnsureRunning { agent, .. } => {
                        if !agent_names.contains(agent.as_str()) {
                            return Err(TuttiError::ConfigValidation(format!(
                                "workflow '{}', step {} references unknown agent '{}'",
                                workflow.name,
                                idx + 1,
                                agent
                            )));
                        }
                    }
                    WorkflowStepConfig::Workflow {
                        workflow: nested,
                        agent,
                        ..
                    } => {
                        if nested.trim().is_empty() {
                            return Err(TuttiError::ConfigValidation(format!(
                                "workflow '{}', step {} has empty nested workflow name",
                                workflow.name,
                                idx + 1
                            )));
                        }
                        if !workflow_names.contains(nested.as_str()) {
                            return Err(TuttiError::ConfigValidation(format!(
                                "workflow '{}', step {} references unknown workflow '{}'",
                                workflow.name,
                                idx + 1,
                                nested
                            )));
                        }
                        if let Some(agent_name) = agent.as_deref()
                            && !agent_names.contains(agent_name)
                        {
                            return Err(TuttiError::ConfigValidation(format!(
                                "workflow '{}', step {} references unknown agent '{}'",
                                workflow.name,
                                idx + 1,
                                agent_name
                            )));
                        }
                    }
                    WorkflowStepConfig::Land { agent, .. } => {
                        if !agent_names.contains(agent.as_str()) {
                            return Err(TuttiError::ConfigValidation(format!(
                                "workflow '{}', step {} references unknown agent '{}'",
                                workflow.name,
                                idx + 1,
                                agent
                            )));
                        }
                    }
                    WorkflowStepConfig::Review {
                        agent, reviewer, ..
                    } => {
                        if !agent_names.contains(agent.as_str()) {
                            return Err(TuttiError::ConfigValidation(format!(
                                "workflow '{}', step {} references unknown agent '{}'",
                                workflow.name,
                                idx + 1,
                                agent
                            )));
                        }
                        if let Some(reviewer_name) = reviewer.as_deref()
                            && !agent_names.contains(reviewer_name)
                        {
                            return Err(TuttiError::ConfigValidation(format!(
                                "workflow '{}', step {} references unknown reviewer agent '{}'",
                                workflow.name,
                                idx + 1,
                                reviewer_name
                            )));
                        }
                    }
                }
            }

            let step_count = workflow.steps.len();
            let explicit_dep_mode = workflow
                .steps
                .iter()
                .any(|step| !step_depends_on(step).is_empty());
            if explicit_dep_mode {
                if workflow.steps.iter().any(|step| !step_is_control(step)) {
                    return Err(TuttiError::ConfigValidation(format!(
                        "workflow '{}' uses depends_on, but only ensure_running/review/land steps currently support depends_on execution",
                        workflow.name
                    )));
                }
                let mut adjacency: Vec<Vec<usize>> = vec![vec![]; step_count];
                let mut in_degree = vec![0usize; step_count];
                for (idx, step) in workflow.steps.iter().enumerate() {
                    let normalized_deps = if step_depends_on(step).is_empty() && idx > 0 {
                        vec![idx]
                    } else {
                        step_depends_on(step).to_vec()
                    };
                    let mut seen = std::collections::HashSet::new();
                    for dep in normalized_deps {
                        if dep == 0 || dep > step_count {
                            return Err(TuttiError::ConfigValidation(format!(
                                "workflow '{}', step {} depends_on references invalid step {}",
                                workflow.name,
                                idx + 1,
                                dep
                            )));
                        }
                        if dep == idx + 1 {
                            return Err(TuttiError::ConfigValidation(format!(
                                "workflow '{}', step {} cannot depend on itself",
                                workflow.name,
                                idx + 1
                            )));
                        }
                        if !seen.insert(dep) {
                            return Err(TuttiError::ConfigValidation(format!(
                                "workflow '{}', step {} has duplicate depends_on step {}",
                                workflow.name,
                                idx + 1,
                                dep
                            )));
                        }
                        adjacency[dep - 1].push(idx);
                        in_degree[idx] += 1;
                    }
                }
                let mut queue: Vec<usize> =
                    (0..step_count).filter(|&i| in_degree[i] == 0).collect();
                let mut visited = 0usize;
                while let Some(node) = queue.pop() {
                    visited += 1;
                    for &next in &adjacency[node] {
                        in_degree[next] -= 1;
                        if in_degree[next] == 0 {
                            queue.push(next);
                        }
                    }
                }
                if visited != step_count {
                    return Err(TuttiError::ConfigValidation(format!(
                        "workflow '{}' has cyclic depends_on graph",
                        workflow.name
                    )));
                }
            }
        }

        for hook in &self.hooks {
            if let Some(agent) = hook.agent.as_deref()
                && !agent_names.contains(agent)
            {
                return Err(TuttiError::ConfigValidation(format!(
                    "hook references unknown agent '{}'",
                    agent
                )));
            }

            let workflow_set = hook.workflow.as_ref().is_some();
            let run_set = hook.run.as_ref().is_some();
            if workflow_set == run_set {
                return Err(TuttiError::ConfigValidation(
                    "hook must specify exactly one of 'workflow' or 'run'".to_string(),
                ));
            }
            if let Some(workflow_name) = hook.workflow.as_deref()
                && !workflow_names.contains(workflow_name)
            {
                return Err(TuttiError::ConfigValidation(format!(
                    "hook references unknown workflow '{}'",
                    workflow_name
                )));
            }
            if let Some(cmd) = hook.run.as_deref()
                && cmd.trim().is_empty()
            {
                return Err(TuttiError::ConfigValidation(
                    "hook run command cannot be empty".to_string(),
                ));
            }
            if hook.event != HookEvent::WorkflowComplete
                && (hook.workflow_source.is_some() || hook.workflow_name.is_some())
            {
                return Err(TuttiError::ConfigValidation(
                    "hook workflow_source/workflow_name filters require event='workflow_complete'"
                        .to_string(),
                ));
            }
            if let Some(workflow_name) = hook.workflow_name.as_deref()
                && workflow_name.trim().is_empty()
            {
                return Err(TuttiError::ConfigValidation(
                    "hook workflow_name cannot be empty".to_string(),
                ));
            }
        }

        Ok(())
    }

    fn validate_tool_packs(&self) -> Result<()> {
        let mut names = std::collections::HashSet::new();
        for pack in &self.tool_packs {
            if pack.name.trim().is_empty() {
                return Err(TuttiError::ConfigValidation(
                    "tool_pack name cannot be empty".to_string(),
                ));
            }
            if !names.insert(pack.name.as_str()) {
                return Err(TuttiError::ConfigValidation(format!(
                    "duplicate tool_pack name: '{}'",
                    pack.name
                )));
            }
            for cmd in &pack.required_commands {
                if cmd.trim().is_empty() {
                    return Err(TuttiError::ConfigValidation(format!(
                        "tool_pack '{}' has empty required_commands entry",
                        pack.name
                    )));
                }
            }
            for key in &pack.required_env {
                if key.trim().is_empty() {
                    return Err(TuttiError::ConfigValidation(format!(
                        "tool_pack '{}' has empty required_env entry",
                        pack.name
                    )));
                }
            }
        }
        Ok(())
    }

    fn validate_budget(&self) -> Result<()> {
        let Some(budget) = self.budget.as_ref() else {
            return Ok(());
        };

        if !(budget.warn_threshold_pct > 0.0 && budget.warn_threshold_pct <= 100.0) {
            return Err(TuttiError::ConfigValidation(
                "budget.warn_threshold_pct must be in (0, 100]".to_string(),
            ));
        }

        if budget.workspace_weekly_tokens == Some(0) {
            return Err(TuttiError::ConfigValidation(
                "budget.workspace_weekly_tokens must be > 0".to_string(),
            ));
        }

        let agent_names: std::collections::HashSet<&str> =
            self.agents.iter().map(|a| a.name.as_str()).collect();
        for (agent, cap) in &budget.agent_weekly_tokens {
            if agent.trim().is_empty() {
                return Err(TuttiError::ConfigValidation(
                    "budget.agent_weekly_tokens contains an empty agent key".to_string(),
                ));
            }
            if *cap == 0 {
                return Err(TuttiError::ConfigValidation(format!(
                    "budget.agent_weekly_tokens['{}'] must be > 0",
                    agent
                )));
            }
            if !agent_names.contains(agent.as_str()) {
                return Err(TuttiError::ConfigValidation(format!(
                    "budget.agent_weekly_tokens references unknown agent '{}'",
                    agent
                )));
            }
        }

        Ok(())
    }
}

/// Topological sort of agents using Kahn's algorithm.
/// Returns agents in dependency order (dependencies first).
pub fn topological_sort(agents: &[AgentConfig]) -> Result<Vec<&AgentConfig>> {
    let name_to_idx: HashMap<&str, usize> = agents
        .iter()
        .enumerate()
        .map(|(i, a)| (a.name.as_str(), i))
        .collect();

    let n = agents.len();
    let mut in_degree = vec![0usize; n];
    let mut adjacency: Vec<Vec<usize>> = vec![vec![]; n];

    for (i, agent) in agents.iter().enumerate() {
        for dep in &agent.depends_on {
            if let Some(&dep_idx) = name_to_idx.get(dep.as_str()) {
                adjacency[dep_idx].push(i);
                in_degree[i] += 1;
            }
        }
    }

    let mut queue: Vec<usize> = (0..n).filter(|&i| in_degree[i] == 0).collect();
    let mut sorted = Vec::with_capacity(n);

    while let Some(idx) = queue.pop() {
        sorted.push(&agents[idx]);
        for &neighbor in &adjacency[idx] {
            in_degree[neighbor] -= 1;
            if in_degree[neighbor] == 0 {
                queue.push(neighbor);
            }
        }
    }

    if sorted.len() != n {
        return Err(TuttiError::ConfigValidation(
            "dependency cycle detected among agents".to_string(),
        ));
    }

    Ok(sorted)
}

impl GlobalConfig {
    /// Load the global config from ~/.config/tutti/config.toml.
    pub fn load() -> Result<Self> {
        let path = global_config_path();
        if !path.exists() {
            return Ok(GlobalConfig::default());
        }
        let contents = std::fs::read_to_string(&path)?;
        let config: GlobalConfig =
            toml::from_str(&contents).map_err(|e| TuttiError::ConfigParse(e.to_string()))?;
        Ok(config)
    }

    /// Save the global config.
    pub fn save(&self) -> Result<()> {
        let path = global_config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let _lock = acquire_global_config_lock(&path)?;
        let toml_str =
            toml::to_string_pretty(self).map_err(|e| TuttiError::ConfigParse(e.to_string()))?;

        let now_nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let tmp_path =
            path.with_extension(format!("toml.tmp.{}.{}", std::process::id(), now_nanos));

        std::fs::write(&tmp_path, toml_str)?;
        std::fs::rename(&tmp_path, &path)?;
        Ok(())
    }

    /// Register a workspace in the global config (idempotent).
    pub fn register_workspace(&mut self, name: &str, path: &Path) {
        // Update existing or add new
        if let Some(existing) = self
            .registered_workspaces
            .iter_mut()
            .find(|w| w.name == name)
        {
            existing.path = path.to_path_buf();
        } else {
            self.registered_workspaces.push(RegisteredWorkspace {
                name: name.to_string(),
                path: path.to_path_buf(),
            });
        }
    }

    /// Get a profile by name.
    pub fn get_profile(&self, name: &str) -> Option<&ProfileConfig> {
        self.profiles.iter().find(|p| p.name == name)
    }
}

struct GlobalConfigLockGuard {
    lock_path: PathBuf,
}

impl Drop for GlobalConfigLockGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock_path);
    }
}

fn acquire_global_config_lock(config_path: &Path) -> Result<GlobalConfigLockGuard> {
    let lock_path = config_path.with_extension("toml.lock");
    let start = std::time::Instant::now();
    let stale_after = std::time::Duration::from_secs(30);
    let timeout = std::time::Duration::from_secs(5);

    loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock_path)
        {
            Ok(_) => {
                return Ok(GlobalConfigLockGuard { lock_path });
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                if let Ok(meta) = std::fs::metadata(&lock_path)
                    && let Ok(modified) = meta.modified()
                    && modified
                        .elapsed()
                        .map(|age| age > stale_after)
                        .unwrap_or(false)
                {
                    let _ = std::fs::remove_file(&lock_path);
                    continue;
                }

                if start.elapsed() > timeout {
                    return Err(TuttiError::State(format!(
                        "timed out acquiring global config lock at {}",
                        lock_path.display()
                    )));
                }

                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Err(e) => return Err(TuttiError::Io(e)),
        }
    }
}

/// Path to the global config file.
pub fn global_config_path() -> PathBuf {
    dirs_or_home()
        .join(".config")
        .join("tutti")
        .join("config.toml")
}

fn dirs_or_home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Walk up from `start_dir` to find tutti.toml.
fn find_config(start_dir: &Path) -> Result<PathBuf> {
    let mut dir = start_dir.to_path_buf();
    loop {
        let candidate = dir.join("tutti.toml");
        if candidate.exists() {
            return Ok(candidate);
        }
        if !dir.pop() {
            return Err(TuttiError::ConfigNotFound(start_dir.to_path_buf()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let toml_str = r#"
[workspace]
name = "test-project"

[[agent]]
name = "backend"
runtime = "claude-code"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.workspace.name, "test-project");
        assert_eq!(config.agents.len(), 1);
        assert_eq!(config.agents[0].name, "backend");
    }

    #[test]
    fn parse_full_config() {
        let toml_str = r#"
[workspace]
name = "medextract"
description = "MedExtract AI medical record extraction"

[workspace.env]
GITHUB_USER = "adamnutt"
git_name = "Adam Nutt"
git_email = "adam@medextract.com.au"

[workspace.auth]
default_profile = "claude-personal"

[defaults]
worktree = true
runtime = "claude-code"

[[agent]]
name = "site"
scope = "site/**"
prompt = "You manage the marketing site."

[[agent]]
name = "pipeline"
runtime = "claude-code"
scope = "src/**"

[[agent]]
name = "codex-tasks"
runtime = "codex"
depends_on = ["site", "pipeline"]

[handoff]
auto = true
threshold = 0.2
include = ["active_task", "file_changes"]

[observe]
dashboard = true
port = 4040
track_cost = true
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.workspace.name, "medextract");
        assert_eq!(
            config.workspace.description.as_deref(),
            Some("MedExtract AI medical record extraction")
        );
        assert_eq!(
            config.workspace.env.as_ref().unwrap().git_email.as_deref(),
            Some("adam@medextract.com.au")
        );
        assert_eq!(
            config
                .workspace
                .auth
                .as_ref()
                .unwrap()
                .default_profile
                .as_deref(),
            Some("claude-personal")
        );
        assert_eq!(config.agents.len(), 3);
        assert_eq!(config.agents[2].depends_on, vec!["site", "pipeline"]);
        assert!(config.handoff.unwrap().auto);
        assert_eq!(config.observe.unwrap().port, 4040);
    }

    #[test]
    fn parse_launch_config() {
        let toml_str = r#"
[workspace]
name = "test-project"

[launch]
mode = "unattended"
policy = "bypass"

[[agent]]
name = "backend"
runtime = "claude-code"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let launch = config.launch.expect("launch config should parse");
        assert_eq!(launch.mode, LaunchMode::Unattended);
        assert_eq!(launch.policy, LaunchPolicyMode::Bypass);
    }

    #[test]
    fn parse_launch_defaults_when_fields_missing() {
        let toml_str = r#"
[workspace]
name = "test-project"

[launch]

[[agent]]
name = "backend"
runtime = "claude-code"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let launch = config.launch.expect("launch config should parse");
        assert_eq!(launch.mode, LaunchMode::Auto);
        assert_eq!(launch.policy, LaunchPolicyMode::Constrained);
    }

    #[test]
    fn parse_persistent_agent() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "pr-monitor"
runtime = "claude-code"
persistent = true
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        assert!(config.agents[0].persistent);
    }

    #[test]
    fn parse_workflows_and_hooks() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"

[[workflow]]
name = "verify"
description = "Run checks"
schedule = "*/30 * * * *"

[[workflow.step]]
type = "prompt"
id = "scan"
agent = "backend"
text = "Check recent changes."
inject_files = [".tutti/state/snapshot.json"]
wait_for_idle = true
wait_timeout_secs = 1200
output_json = "tmp/scan.json"

[[workflow.step]]
type = "command"
id = "tests"
run = "cargo test --quiet"
cwd = "workspace"
subdir = "backend"
fail_mode = "closed"
timeout_secs = 1200
output_json = "tmp/tests.json"

[[hook]]
event = "agent_stop"
agent = "backend"
workflow = "verify"
fail_mode = "open"

[[hook]]
event = "workflow_complete"
workflow_source = "run"
workflow_name = "verify"
run = "echo done"
"#;

        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.workflows.len(), 1);
        assert_eq!(config.hooks.len(), 2);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn parse_workflow_control_steps() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"

[[agent]]
name = "reviewer"
runtime = "claude-code"

[[workflow]]
name = "verify"

[[workflow.step]]
type = "command"
run = "echo ok"

[[workflow]]
name = "autofix"

[[workflow.step]]
type = "ensure_running"
agent = "backend"

[[workflow.step]]
type = "workflow"
workflow = "verify"
agent = "backend"
strict = true

[[workflow.step]]
type = "review"
agent = "backend"
reviewer = "reviewer"

[[workflow.step]]
type = "land"
agent = "backend"
force = true
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn parse_budget_config() {
        let toml = r#"
[workspace]
name = "test-project"

[defaults]
runtime = "claude-code"

[[agent]]
name = "backend"
prompt = "You own backend."

[budget]
mode = "enforce"
warn_threshold_pct = 85
workspace_weekly_tokens = 1000000

[budget.agent_weekly_tokens]
backend = 250000
"#;
        let config: TuttiConfig = toml::from_str(toml).unwrap();
        let budget = config.budget.as_ref().expect("budget should parse");
        assert_eq!(budget.mode, BudgetMode::Enforce);
        assert_eq!(budget.warn_threshold_pct, 85.0);
        assert_eq!(budget.workspace_weekly_tokens, Some(1_000_000));
        assert_eq!(budget.agent_weekly_tokens.get("backend"), Some(&250_000));
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_budget_rejects_unknown_agent_caps() {
        let toml = r#"
[workspace]
name = "test-project"

[defaults]
runtime = "claude-code"

[[agent]]
name = "backend"
prompt = "You own backend."

[budget]
workspace_weekly_tokens = 1000000

[budget.agent_weekly_tokens]
frontend = 250000
"#;
        let config: TuttiConfig = toml::from_str(toml).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("unknown agent"));
    }

    #[test]
    fn validate_schedule_must_be_five_field_cron() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"

[[workflow]]
name = "verify"
schedule = "* * * *"

[[workflow.step]]
type = "command"
run = "echo ok"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("invalid schedule"));
    }

    #[test]
    fn validate_output_json_requires_step_id() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"

[[workflow]]
name = "verify"

[[workflow.step]]
type = "command"
run = "echo ok"
output_json = "out.json"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("missing id"));
    }

    #[test]
    fn validate_command_subdir_must_be_relative() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"

[[workflow]]
name = "verify"

[[workflow.step]]
type = "command"
run = "echo ok"
subdir = "/tmp"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("subdir must be workspace-relative")
        );
    }

    #[test]
    fn validate_command_subdir_cannot_be_empty() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"

[[workflow]]
name = "verify"

[[workflow.step]]
type = "command"
run = "echo ok"
subdir = "   "
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("empty subdir"));
    }

    #[test]
    fn validate_depends_on_rejects_non_control_steps() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"

[[workflow]]
name = "verify"

[[workflow.step]]
type = "command"
run = "echo ok"
depends_on = [1]
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("only ensure_running/review/land"));
    }

    #[test]
    fn validate_depends_on_detects_cycles_for_control_steps() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"

[[agent]]
name = "reviewer"
runtime = "claude-code"

[[workflow]]
name = "autofix"

[[workflow.step]]
type = "ensure_running"
agent = "backend"
depends_on = [2]

[[workflow.step]]
type = "review"
agent = "backend"
reviewer = "reviewer"
depends_on = [1]
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("cyclic depends_on"));
    }

    #[test]
    fn validate_nested_workflow_reference_must_exist() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"

[[workflow]]
name = "autofix"

[[workflow.step]]
type = "workflow"
workflow = "missing"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("unknown workflow"));
    }

    #[test]
    fn validate_prompt_inject_files_must_be_relative() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"

[[workflow]]
name = "verify"

[[workflow.step]]
type = "prompt"
agent = "backend"
text = "check"
inject_files = ["/tmp/snapshot.json"]
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("workspace-relative"));
    }

    #[test]
    fn memory_field_parses_from_toml() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"
memory = ".tutti/state/memory/backend.md"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.agents[0].memory.as_deref(),
            Some(".tutti/state/memory/backend.md")
        );
        config.validate().unwrap();
    }

    #[test]
    fn memory_field_defaults_to_none() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        assert!(config.agents[0].memory.is_none());
    }

    #[test]
    fn memory_path_must_be_relative() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"
memory = "/tmp/memory.md"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("must be relative"));
    }

    #[test]
    fn memory_path_rejects_traversal() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"
memory = "../../etc/passwd"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("must not contain '..'"));
    }

    #[test]
    fn memory_path_allows_double_dot_in_filename() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"
memory = ".tutti/state/memory/notes..md"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        config.validate().unwrap();
    }

    #[test]
    fn parse_tool_packs() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"

[[tool_pack]]
name = "analytics"
required_commands = ["bq", "jq"]
required_env = ["GCP_PROJECT"]
"#;

        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.tool_packs.len(), 1);
        assert_eq!(config.tool_packs[0].name, "analytics");
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_duplicate_tool_pack_names() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"

[[tool_pack]]
name = "analytics"

[[tool_pack]]
name = "analytics"
"#;

        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("duplicate tool_pack name"));
    }

    #[test]
    fn parse_global_config() {
        let toml_str = r#"
[user]
name = "Adam Nutt"

[[profile]]
name = "claude-personal"
provider = "anthropic"
command = "claude"
max_concurrent = 5
monthly_budget = 100.00

[[profile]]
name = "claude-work"
provider = "anthropic"
command = "claude"
max_concurrent = 10
priority = 2

[[registered_workspace]]
name = "medextract"
path = "/Users/adamnutt/Documents/GitHub/medextract"

[[registered_workspace]]
name = "4est"
path = "/Users/adamnutt/Documents/GitHub/4est"

[dashboard]
port = 4040
show_all_workspaces = true

[resilience]
provider_down_strategy = "pause"
save_state_on_failure = true
"#;
        let config: GlobalConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.user.unwrap().name, "Adam Nutt");
        assert_eq!(config.profiles.len(), 2);
        assert_eq!(config.profiles[0].name, "claude-personal");
        assert_eq!(config.profiles[0].max_concurrent, Some(5));
        assert_eq!(config.registered_workspaces.len(), 2);
        assert!(config.dashboard.unwrap().show_all_workspaces);
        assert!(config.resilience.unwrap().save_state_on_failure);
    }

    #[test]
    fn validate_duplicate_names() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "foo"
runtime = "claude-code"

[[agent]]
name = "foo"
runtime = "claude-code"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("duplicate agent name"));
    }

    #[test]
    fn validate_bad_depends_on() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "foo"
runtime = "claude-code"
depends_on = ["nonexistent"]
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("does not exist"));
    }

    #[test]
    fn validate_self_dependency() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "foo"
runtime = "claude-code"
depends_on = ["foo"]
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("depends on itself"));
    }

    #[test]
    fn validate_dependency_cycle() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "a"
runtime = "claude-code"
depends_on = ["b"]

[[agent]]
name = "b"
runtime = "claude-code"
depends_on = ["c"]

[[agent]]
name = "c"
runtime = "claude-code"
depends_on = ["a"]
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn validate_agent_env_parses() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "foo"
runtime = "claude-code"

[agent.env]
CUSTOM_KEY = "custom_value"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(
            config.agents[0].env.get("CUSTOM_KEY").map(|s| s.as_str()),
            Some("custom_value")
        );
    }

    #[test]
    fn validate_unknown_runtime() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "foo"
runtime = "invalid-runtime"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("unknown runtime"));
    }

    #[test]
    fn validate_rejects_hook_without_action() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"

[[hook]]
event = "agent_stop"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(
            err.to_string()
                .contains("exactly one of 'workflow' or 'run'")
        );
    }

    #[test]
    fn validate_rejects_unknown_hook_workflow() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"

[[workflow]]
name = "verify"

[[workflow.step]]
type = "command"
run = "echo ok"

[[hook]]
event = "agent_stop"
workflow = "missing"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("unknown workflow"));
    }

    #[test]
    fn validate_rejects_workflow_filters_on_non_workflow_event() {
        let toml_str = r#"
[workspace]
name = "test"

[[agent]]
name = "backend"
runtime = "claude-code"

[[workflow]]
name = "verify"

[[workflow.step]]
type = "command"
run = "echo ok"

[[hook]]
event = "agent_stop"
workflow = "verify"
workflow_source = "run"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("workflow_source/workflow_name"));
    }

    #[test]
    fn resolved_runtime_falls_back_to_default() {
        let defaults = DefaultsConfig {
            worktree: true,
            runtime: Some("claude-code".to_string()),
        };
        let agent = AgentConfig {
            name: "test".to_string(),
            runtime: None,
            scope: None,
            prompt: None,
            depends_on: vec![],
            worktree: None,
            fresh_worktree: None,
            branch: None,
            persistent: false,
            memory: None,
            env: HashMap::new(),
        };
        assert_eq!(
            agent.resolved_runtime(&defaults),
            Some("claude-code".to_string())
        );
    }

    #[test]
    fn resolved_branch_default() {
        let agent = AgentConfig {
            name: "backend".to_string(),
            runtime: None,
            scope: None,
            prompt: None,
            depends_on: vec![],
            worktree: None,
            fresh_worktree: None,
            branch: None,
            persistent: false,
            memory: None,
            env: HashMap::new(),
        };
        assert_eq!(agent.resolved_branch(), "tutti/backend");
    }

    #[test]
    fn resolved_fresh_worktree_defaults_false() {
        let mut agent = AgentConfig {
            name: "backend".to_string(),
            runtime: None,
            scope: None,
            prompt: None,
            depends_on: vec![],
            worktree: None,
            fresh_worktree: None,
            branch: None,
            persistent: false,
            memory: None,
            env: HashMap::new(),
        };
        assert!(!agent.resolved_fresh_worktree());
        agent.fresh_worktree = Some(true);
        assert!(agent.resolved_fresh_worktree());
    }

    #[test]
    fn global_config_register_workspace() {
        let mut config = GlobalConfig::default();
        config.register_workspace("test", Path::new("/tmp/test"));
        assert_eq!(config.registered_workspaces.len(), 1);

        // Idempotent — update path
        config.register_workspace("test", Path::new("/tmp/test2"));
        assert_eq!(config.registered_workspaces.len(), 1);
        assert_eq!(
            config.registered_workspaces[0].path,
            PathBuf::from("/tmp/test2")
        );
    }

    #[test]
    fn webhook_config_serde_round_trip() {
        let toml_str = r#"
[workspace]
name = "test"

[[webhook]]
source = "github"
events = ["issues.labeled", "push"]
workflow = "sdlc-auto"

[[webhook]]
source = "generic"
events = ["*"]
agent = "implementer"
prompt = "Handle incoming event"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.webhooks.len(), 2);

        let wh0 = &config.webhooks[0];
        assert_eq!(wh0.source, "github");
        assert_eq!(wh0.events, vec!["issues.labeled", "push"]);
        assert_eq!(wh0.workflow.as_deref(), Some("sdlc-auto"));
        assert!(wh0.agent.is_none());

        let wh1 = &config.webhooks[1];
        assert_eq!(wh1.source, "generic");
        assert_eq!(wh1.events, vec!["*"]);
        assert!(wh1.workflow.is_none());
        assert_eq!(wh1.agent.as_deref(), Some("implementer"));
        assert_eq!(wh1.prompt.as_deref(), Some("Handle incoming event"));
    }

    #[test]
    fn webhook_config_empty_is_default() {
        let toml_str = r#"
[workspace]
name = "test"
"#;
        let config: TuttiConfig = toml::from_str(toml_str).unwrap();
        assert!(config.webhooks.is_empty());
    }
}
