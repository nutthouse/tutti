use crate::error::{Result, TuttiError};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
    pub branch: Option<String>,
    #[serde(default)]
    pub persistent: bool,
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
    },
    Command {
        #[serde(default)]
        id: Option<String>,
        run: String,
        #[serde(default)]
        cwd: Option<WorkflowCommandCwd>,
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
        agent: String,
        #[serde(default)]
        fail_mode: Option<WorkflowFailMode>,
    },
    Workflow {
        workflow: String,
        #[serde(default)]
        agent: Option<String>,
        #[serde(default)]
        strict: Option<bool>,
        #[serde(default)]
        fail_mode: Option<WorkflowFailMode>,
    },
    Land {
        agent: String,
        #[serde(default)]
        pr: Option<bool>,
        #[serde(default)]
        force: Option<bool>,
        #[serde(default)]
        fail_mode: Option<WorkflowFailMode>,
    },
    Review {
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

impl AgentConfig {
    pub fn resolved_runtime(&self, defaults: &DefaultsConfig) -> Option<String> {
        self.runtime.clone().or_else(|| defaults.runtime.clone())
    }

    pub fn resolved_worktree(&self, defaults: &DefaultsConfig) -> bool {
        self.worktree.unwrap_or(defaults.worktree)
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

        self.validate_automation()?;
        self.validate_tool_packs()?;

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
                        ..
                    } => {
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
        let toml_str =
            toml::to_string_pretty(self).map_err(|e| TuttiError::ConfigParse(e.to_string()))?;
        std::fs::write(path, toml_str)?;
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
            branch: None,
            persistent: false,
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
            branch: None,
            persistent: false,
            env: HashMap::new(),
        };
        assert_eq!(agent.resolved_branch(), "tutti/backend");
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
}
