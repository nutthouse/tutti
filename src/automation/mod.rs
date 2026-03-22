use crate::config::{
    HookConfig, HookEvent, HookWorkflowSource, PermissionsConfig, ResilienceConfig, TuttiConfig,
    WorkflowCommandCwd, WorkflowConfig, WorkflowFailMode, WorkflowStepConfig,
};
use crate::error::{Result, TuttiError};
use crate::health;
use crate::health::WaitFailureReason;
use crate::permissions::evaluate_command_policy;
use crate::runtime::{self, AgentStatus};
use crate::session::TmuxSession;
use crate::state::{
    AutomationRunRecord, ControlEvent, VerifyLastSummary, WorkflowStepIntentRecord,
    WorkflowStepOutcomeRecord, append_automation_run, append_control_event, append_policy_decision,
    load_workflow_checkpoint, load_workflow_intent, save_verify_last_summary,
    save_workflow_checkpoint, save_workflow_intent, save_workflow_output,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

const DEFAULT_TIMEOUT_SECS: u64 = 900;
const PROMPT_CAPTURE_LINES: u32 = 200;
const DEFAULT_STARTUP_GRACE_SECS: u64 = 30;
const VALIDATION_REPAIR_MAX_ATTEMPTS: u32 = 2;

#[derive(Debug, Deserialize)]
struct WorkflowBranchState {
    branch: String,
    #[serde(default = "default_base_branch")]
    base_branch: String,
    #[serde(default)]
    base_sha: Option<String>,
}

#[derive(Debug, Deserialize)]
struct SelectedIssueState {
    issue_number: u64,
    title: String,
}

fn default_base_branch() -> String {
    "main".to_string()
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionOrigin {
    Run,
    Verify,
    HookAgentStop,
    ObserveCycle,
    HookWorkflowComplete,
}

impl ExecutionOrigin {
    fn as_str(self) -> &'static str {
        match self {
            ExecutionOrigin::Run => "run",
            ExecutionOrigin::Verify => "verify",
            ExecutionOrigin::HookAgentStop => "hook_agent_stop",
            ExecutionOrigin::ObserveCycle => "observe_cycle",
            ExecutionOrigin::HookWorkflowComplete => "hook_workflow_complete",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExecuteOptions {
    pub strict: bool,
    pub force_open_commands: bool,
    pub command_policy: Option<PermissionsConfig>,
    pub retry_policy: Option<RetryPolicy>,
    pub origin: ExecutionOrigin,
    pub hook_event: Option<String>,
    pub hook_agent: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub initial_backoff_ms: u64,
    pub max_backoff_ms: u64,
}

impl RetryPolicy {
    fn normalized(max_attempts: u32, initial_backoff_ms: u64, max_backoff_ms: u64) -> Option<Self> {
        let attempts = max_attempts.max(1);
        if attempts <= 1 {
            return None;
        }
        let initial = initial_backoff_ms.max(1);
        let max_backoff = max_backoff_ms.max(initial);
        Some(Self {
            max_attempts: attempts,
            initial_backoff_ms: initial,
            max_backoff_ms: max_backoff,
        })
    }
}

pub fn retry_policy_from_resilience(resilience: Option<&ResilienceConfig>) -> Option<RetryPolicy> {
    let resilience = resilience?;
    let max_attempts = resilience.retry_max_attempts.unwrap_or(1);
    let initial_backoff_ms = resilience.retry_initial_backoff_ms.unwrap_or(500);
    let max_backoff_ms = resilience.retry_max_backoff_ms.unwrap_or(5_000);
    RetryPolicy::normalized(max_attempts, initial_backoff_ms, max_backoff_ms)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HookEventPayload {
    pub workspace_name: String,
    pub project_root: PathBuf,
    pub agent_name: String,
    pub runtime: String,
    pub session_name: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowCompletePayload {
    pub workspace_name: String,
    pub project_root: PathBuf,
    pub workflow_name: String,
    pub workflow_source: String,
    pub success: bool,
    pub agent_scope: Option<String>,
}

#[derive(Debug, Clone)]
pub struct WorkflowResolver<'a> {
    config: &'a TuttiConfig,
    project_root: &'a Path,
}

impl<'a> WorkflowResolver<'a> {
    pub fn new(config: &'a TuttiConfig, project_root: &'a Path) -> Self {
        Self {
            config,
            project_root,
        }
    }

    pub fn resolve(
        &self,
        workflow_name: &str,
        agent_override: Option<&str>,
        options: &ExecuteOptions,
    ) -> Result<ResolvedWorkflow> {
        if let Some(agent) = agent_override {
            self.ensure_agent_exists(agent)?;
        }

        let workflow = self
            .config
            .workflows
            .iter()
            .find(|w| w.name == workflow_name)
            .ok_or_else(|| {
                TuttiError::ConfigValidation(format!("workflow '{}' not found", workflow_name))
            })?;

        self.resolve_workflow(workflow, agent_override, options)
    }

    fn resolve_workflow(
        &self,
        workflow: &WorkflowConfig,
        agent_override: Option<&str>,
        options: &ExecuteOptions,
    ) -> Result<ResolvedWorkflow> {
        let mut steps = Vec::with_capacity(workflow.steps.len());

        for step in &workflow.steps {
            match step {
                WorkflowStepConfig::Prompt {
                    id,
                    depends_on,
                    agent,
                    text,
                    inject_files,
                    output_json,
                    wait_for_idle,
                    wait_timeout_secs,
                    startup_grace_secs,
                    artifact_glob,
                    artifact_name,
                } => {
                    let effective_agent = agent_override.unwrap_or(agent.as_str());
                    self.ensure_agent_exists(effective_agent)?;
                    let session_name =
                        TmuxSession::session_name(&self.config.workspace.name, effective_agent);
                    let runtime = self
                        .config
                        .agents
                        .iter()
                        .find(|a| a.name == effective_agent)
                        .and_then(|a| a.resolved_runtime(&self.config.defaults))
                        .unwrap_or_else(|| "unknown".to_string());
                    steps.push(ResolvedStep::Prompt {
                        step_id: id.clone(),
                        depends_on: depends_on.clone(),
                        agent: effective_agent.to_string(),
                        text: text.clone(),
                        runtime,
                        session_name,
                        inject_files: self
                            .resolve_prompt_injected_files(effective_agent, inject_files),
                        inject_files_raw: inject_files.clone(),
                        output_json: self
                            .resolve_prompt_output_path(effective_agent, output_json)?,
                        wait_for_idle: wait_for_idle.unwrap_or(false),
                        wait_timeout_secs: wait_timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
                        startup_grace_secs: startup_grace_secs
                            .unwrap_or(DEFAULT_STARTUP_GRACE_SECS),
                        artifact_glob: artifact_glob.clone(),
                        artifact_name: artifact_name.clone(),
                    });
                }
                WorkflowStepConfig::Command {
                    id,
                    depends_on,
                    run,
                    cwd,
                    subdir,
                    agent,
                    timeout_secs,
                    fail_mode,
                    output_json,
                } => {
                    let effective_agent = agent_override.or(agent.as_deref());
                    if let Some(agent_name) = effective_agent {
                        self.ensure_agent_exists(agent_name)?;
                    }

                    let cwd_mode = cwd.unwrap_or(WorkflowCommandCwd::Workspace);
                    let mut resolved_cwd = match cwd_mode {
                        WorkflowCommandCwd::Workspace => self.project_root.to_path_buf(),
                        WorkflowCommandCwd::AgentWorktree => {
                            let agent_name = effective_agent.ok_or_else(|| {
                                TuttiError::ConfigValidation(
                                    "command step with cwd='agent_worktree' requires agent (step or --agent)"
                                        .to_string(),
                                )
                            })?;
                            let path = self
                                .project_root
                                .join(".tutti")
                                .join("worktrees")
                                .join(agent_name);
                            if !path.exists() {
                                return Err(TuttiError::ConfigValidation(format!(
                                    "agent worktree not found for '{}': {}",
                                    agent_name,
                                    path.display()
                                )));
                            }
                            path
                        }
                    };

                    if let Some(subdir) = subdir.as_deref() {
                        let subdir = subdir.trim();
                        resolved_cwd = resolved_cwd.join(subdir);
                        if !resolved_cwd.exists() {
                            return Err(TuttiError::ConfigValidation(format!(
                                "command step subdir does not exist: {}",
                                resolved_cwd.display()
                            )));
                        }
                    }

                    let output_json = resolve_optional_path(&resolved_cwd, output_json.as_deref());
                    steps.push(ResolvedStep::Command {
                        step_id: id.clone(),
                        depends_on: depends_on.clone(),
                        run: run.clone(),
                        cwd: resolved_cwd,
                        agent: effective_agent.map(|s| s.to_string()),
                        timeout_secs: timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
                        fail_mode: effective_fail_mode(
                            *fail_mode,
                            options.strict,
                            options.force_open_commands,
                        ),
                        output_json,
                    });
                }
                WorkflowStepConfig::EnsureRunning {
                    depends_on,
                    agent,
                    fail_mode,
                } => {
                    let effective_agent = agent_override.unwrap_or(agent.as_str());
                    self.ensure_agent_exists(effective_agent)?;
                    let session_name =
                        TmuxSession::session_name(&self.config.workspace.name, effective_agent);
                    steps.push(ResolvedStep::EnsureRunning {
                        depends_on: depends_on.clone(),
                        agent: effective_agent.to_string(),
                        session_name,
                        fail_mode: effective_control_fail_mode(*fail_mode, options.strict),
                    });
                }
                WorkflowStepConfig::Workflow {
                    depends_on,
                    workflow,
                    agent,
                    strict,
                    fail_mode,
                } => {
                    if let Some(agent_name) = agent_override.or(agent.as_deref()) {
                        self.ensure_agent_exists(agent_name)?;
                    }
                    steps.push(ResolvedStep::Workflow {
                        depends_on: depends_on.clone(),
                        workflow: workflow.clone(),
                        agent_override: agent_override.or(agent.as_deref()).map(|s| s.to_string()),
                        strict: strict.unwrap_or(options.strict),
                        fail_mode: effective_control_fail_mode(*fail_mode, options.strict),
                    });
                }
                WorkflowStepConfig::Land {
                    depends_on,
                    agent,
                    pr,
                    force,
                    fail_mode,
                } => {
                    let effective_agent = agent_override.unwrap_or(agent.as_str());
                    self.ensure_agent_exists(effective_agent)?;
                    steps.push(ResolvedStep::Land {
                        depends_on: depends_on.clone(),
                        agent: effective_agent.to_string(),
                        pr: pr.unwrap_or(false),
                        force: force.unwrap_or(false),
                        fail_mode: effective_control_fail_mode(*fail_mode, options.strict),
                    });
                }
                WorkflowStepConfig::Review {
                    depends_on,
                    agent,
                    reviewer,
                    fail_mode,
                } => {
                    let effective_agent = agent_override.unwrap_or(agent.as_str());
                    self.ensure_agent_exists(effective_agent)?;
                    let resolved_reviewer = reviewer.clone().unwrap_or_else(|| "reviewer".into());
                    self.ensure_agent_exists(&resolved_reviewer)?;
                    steps.push(ResolvedStep::Review {
                        depends_on: depends_on.clone(),
                        agent: effective_agent.to_string(),
                        reviewer: resolved_reviewer,
                        fail_mode: effective_control_fail_mode(*fail_mode, options.strict),
                    });
                }
            }
        }

        Ok(ResolvedWorkflow {
            name: workflow.name.clone(),
            description: workflow.description.clone(),
            steps,
        })
    }

    fn ensure_agent_exists(&self, agent_name: &str) -> Result<()> {
        if self.config.agents.iter().any(|a| a.name == agent_name) {
            Ok(())
        } else {
            Err(TuttiError::ConfigValidation(format!(
                "unknown agent '{}'",
                agent_name
            )))
        }
    }

    fn resolve_prompt_output_path(
        &self,
        agent_name: &str,
        output_json: &Option<String>,
    ) -> Result<Option<PathBuf>> {
        let Some(path) = output_json.as_deref() else {
            return Ok(None);
        };
        let as_path = Path::new(path);
        if as_path.is_absolute() {
            return Ok(Some(as_path.to_path_buf()));
        }

        let worktree = self
            .project_root
            .join(".tutti")
            .join("worktrees")
            .join(agent_name);
        if worktree.exists() {
            return Ok(Some(worktree.join(as_path)));
        }
        Ok(Some(self.project_root.join(as_path)))
    }

    fn resolve_prompt_injected_files(
        &self,
        agent_name: &str,
        inject_files: &[String],
    ) -> Vec<PromptInjectedFile> {
        let agent_uses_worktree = self
            .config
            .agents
            .iter()
            .find(|a| a.name == agent_name)
            .is_some_and(|a| a.resolved_worktree(&self.config.defaults));
        let destination_root = if agent_uses_worktree {
            self.project_root
                .join(".tutti")
                .join("worktrees")
                .join(agent_name)
        } else {
            self.project_root.to_path_buf()
        };

        inject_files
            .iter()
            .map(|relative| {
                let rel = Path::new(relative);
                PromptInjectedFile {
                    source: self.project_root.join(rel),
                    destination: destination_root.join(rel),
                }
            })
            .collect()
    }
}

fn resolve_optional_path(cwd: &Path, maybe: Option<&str>) -> Option<PathBuf> {
    let path = maybe?;
    let as_path = Path::new(path);
    if as_path.is_absolute() {
        Some(as_path.to_path_buf())
    } else {
        Some(cwd.join(as_path))
    }
}

fn effective_fail_mode(
    configured: Option<WorkflowFailMode>,
    strict: bool,
    force_open_commands: bool,
) -> WorkflowFailMode {
    if strict {
        return WorkflowFailMode::Closed;
    }
    if force_open_commands {
        return WorkflowFailMode::Open;
    }
    configured.unwrap_or(WorkflowFailMode::Open)
}

fn effective_control_fail_mode(
    configured: Option<WorkflowFailMode>,
    strict: bool,
) -> WorkflowFailMode {
    if strict {
        WorkflowFailMode::Closed
    } else {
        configured.unwrap_or(WorkflowFailMode::Closed)
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedWorkflow {
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<ResolvedStep>,
}

#[derive(Debug, Clone)]
pub enum ResolvedStep {
    Prompt {
        step_id: Option<String>,
        depends_on: Vec<usize>,
        agent: String,
        text: String,
        runtime: String,
        session_name: String,
        inject_files: Vec<PromptInjectedFile>,
        inject_files_raw: Vec<String>,
        output_json: Option<PathBuf>,
        wait_for_idle: bool,
        wait_timeout_secs: u64,
        startup_grace_secs: u64,
        artifact_glob: Option<String>,
        artifact_name: Option<String>,
    },
    Command {
        step_id: Option<String>,
        depends_on: Vec<usize>,
        run: String,
        cwd: PathBuf,
        agent: Option<String>,
        timeout_secs: u64,
        fail_mode: WorkflowFailMode,
        output_json: Option<PathBuf>,
    },
    EnsureRunning {
        depends_on: Vec<usize>,
        agent: String,
        session_name: String,
        fail_mode: WorkflowFailMode,
    },
    Workflow {
        depends_on: Vec<usize>,
        workflow: String,
        agent_override: Option<String>,
        strict: bool,
        fail_mode: WorkflowFailMode,
    },
    Land {
        depends_on: Vec<usize>,
        agent: String,
        pr: bool,
        force: bool,
        fail_mode: WorkflowFailMode,
    },
    Review {
        depends_on: Vec<usize>,
        agent: String,
        reviewer: String,
        fail_mode: WorkflowFailMode,
    },
}

fn step_depends_on(step: &ResolvedStep) -> &[usize] {
    match step {
        ResolvedStep::Prompt { depends_on, .. } => depends_on,
        ResolvedStep::Command { depends_on, .. } => depends_on,
        ResolvedStep::EnsureRunning { depends_on, .. } => depends_on,
        ResolvedStep::Workflow { depends_on, .. } => depends_on,
        ResolvedStep::Land { depends_on, .. } => depends_on,
        ResolvedStep::Review { depends_on, .. } => depends_on,
    }
}

fn step_is_control(step: &ResolvedStep) -> bool {
    matches!(
        step,
        ResolvedStep::EnsureRunning { .. }
            | ResolvedStep::Review { .. }
            | ResolvedStep::Land { .. }
    )
}

fn step_type_name(step: &ResolvedStep) -> &'static str {
    match step {
        ResolvedStep::Prompt { .. } => "prompt",
        ResolvedStep::Command { .. } => "command",
        ResolvedStep::EnsureRunning { .. } => "ensure_running",
        ResolvedStep::Workflow { .. } => "workflow",
        ResolvedStep::Land { .. } => "land",
        ResolvedStep::Review { .. } => "review",
    }
}

fn step_agent_name(step: &ResolvedStep) -> Option<&str> {
    match step {
        ResolvedStep::Prompt { agent, .. } => Some(agent),
        ResolvedStep::Command { agent, .. } => agent.as_deref(),
        ResolvedStep::EnsureRunning { agent, .. } => Some(agent),
        ResolvedStep::Workflow { .. } => None,
        ResolvedStep::Land { agent, .. } => Some(agent),
        ResolvedStep::Review { reviewer, .. } => Some(reviewer),
    }
}

fn sanitize_step_key(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn step_file_key(step: &ResolvedStep, step_index: usize) -> String {
    let base = match step {
        ResolvedStep::Prompt { step_id, .. } | ResolvedStep::Command { step_id, .. } => {
            step_id.as_deref().unwrap_or_else(|| step_type_name(step))
        }
        _ => step_type_name(step),
    };
    format!("{step_index:03}-{}", sanitize_step_key(base))
}

#[derive(Debug, Clone)]
pub struct PromptInjectedFile {
    source: PathBuf,
    destination: PathBuf,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    Success,
    Warning,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StepResult {
    pub index: usize,
    pub step_type: String,
    pub status: StepStatus,
    pub duration_ms: u64,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub message: Option<String>,
    pub stdout: Option<String>,
    pub stderr: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    pub run_id: String,
    pub workflow_name: String,
    pub strict: bool,
    pub success: bool,
    pub started_at: chrono::DateTime<Utc>,
    pub finished_at: chrono::DateTime<Utc>,
    pub failed_steps: Vec<usize>,
    pub step_results: Vec<StepResult>,
    pub output_files: HashMap<String, String>,
}

impl ExecutionResult {
    pub fn warning_count(&self) -> usize {
        self.step_results
            .iter()
            .filter(|s| s.status == StepStatus::Warning)
            .count()
    }
}

#[derive(Debug, Clone)]
pub struct ResumeContext {
    pub run_id: String,
    pub workflow_name: String,
    pub strict: bool,
    pub agent_scope: Option<String>,
    pub started_at: chrono::DateTime<Utc>,
    pub failed_steps: Vec<usize>,
    pub completed_steps: HashSet<usize>,
    pub step_results: Vec<StepResult>,
    pub output_files: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkflowCheckpointRecord {
    run_id: String,
    workflow_name: String,
    strict: bool,
    origin: String,
    agent_scope: Option<String>,
    started_at: chrono::DateTime<Utc>,
    finished_at: chrono::DateTime<Utc>,
    success: bool,
    failed_steps: Vec<usize>,
    step_results: Vec<StepResult>,
    output_files: HashMap<String, String>,
}

pub fn load_resume_context(project_root: &Path, run_id: &str) -> Result<Option<ResumeContext>> {
    let Some(raw) = load_workflow_checkpoint(project_root, run_id)? else {
        return Ok(None);
    };
    let record: WorkflowCheckpointRecord =
        serde_json::from_value(raw).map_err(|e| TuttiError::State(e.to_string()))?;

    if record.success {
        return Err(TuttiError::ConfigValidation(format!(
            "run '{}' already succeeded; nothing to resume",
            run_id
        )));
    }

    let step_results: Vec<StepResult> = record
        .step_results
        .into_iter()
        .filter(|s| s.status != StepStatus::Failed)
        .collect();
    let completed_steps: HashSet<usize> = step_results.iter().map(|s| s.index).collect();

    Ok(Some(ResumeContext {
        run_id: record.run_id,
        workflow_name: record.workflow_name,
        strict: record.strict,
        agent_scope: record.agent_scope,
        started_at: record.started_at,
        failed_steps: record.failed_steps,
        completed_steps,
        step_results,
        output_files: record.output_files,
    }))
}

pub struct WorkflowExecutor<'a> {
    config: &'a TuttiConfig,
    project_root: &'a Path,
}

impl<'a> WorkflowExecutor<'a> {
    pub fn new(config: &'a TuttiConfig, project_root: &'a Path) -> Self {
        Self {
            config,
            project_root,
        }
    }

    pub fn execute(
        &self,
        workflow: &ResolvedWorkflow,
        options: &ExecuteOptions,
        agent_scope: Option<&str>,
        run_id: Option<&str>,
        resume: Option<&ResumeContext>,
    ) -> Result<ExecutionResult> {
        let started_at = resume.map(|r| r.started_at).unwrap_or_else(Utc::now);
        let run_id = run_id
            .map(ToString::to_string)
            .or_else(|| resume.map(|r| r.run_id.clone()))
            .unwrap_or_else(generate_run_id);
        let mut success = true;
        let mut failed_steps = Vec::new();
        let mut step_results = resume
            .map(|r| r.step_results.clone())
            .unwrap_or_else(|| Vec::with_capacity(workflow.steps.len()));
        let mut output_files = resume.map(|r| r.output_files.clone()).unwrap_or_default();
        let mut outputs = if let Some(ctx) = resume {
            load_resume_outputs(&ctx.output_files)?
        } else {
            HashMap::<String, StepOutputValue>::new()
        };
        let completed_steps = resume
            .map(|r| r.completed_steps.clone())
            .unwrap_or_default();
        let mut attempted_steps = HashSet::new();
        let explicit_dep_mode = workflow
            .steps
            .iter()
            .any(|step| !step_depends_on(step).is_empty());

        if explicit_dep_mode {
            if workflow.steps.iter().any(|step| !step_is_control(step)) {
                return Err(TuttiError::ConfigValidation(
                    "depends_on execution currently supports ensure_running/review/land steps only"
                        .to_string(),
                ));
            }
            let dep_graph = build_normalized_dependencies(&workflow.steps);
            let dag = execute_control_dag(
                self.config,
                self.project_root,
                &run_id,
                &workflow.name,
                &workflow.steps,
                &dep_graph,
                &completed_steps,
            )?;
            success = dag.success;
            failed_steps.extend(dag.failed_steps);
            for result in &dag.step_results {
                attempted_steps.insert(result.index);
            }
            step_results.extend(dag.step_results);
        } else {
            for (idx, step) in workflow.steps.iter().enumerate() {
                let step_index = idx + 1;
                if completed_steps.contains(&step_index) {
                    continue;
                }
                let prior_unsuccessful_intent = if resume.is_some() {
                    load_unsuccessful_step_intent(self.project_root, &run_id, step, step_index)?
                } else {
                    None
                };
                attempted_steps.insert(step_index);
                let _ = append_control_event(
                    self.project_root,
                    &ControlEvent {
                        event: "workflow.step.started".to_string(),
                        workspace: self.config.workspace.name.clone(),
                        agent: step_agent_name(step).map(|s| s.to_string()),
                        timestamp: Utc::now(),
                        correlation_id: run_id.clone(),
                        data: Some({
                            let mut d = json!({
                                "workflow_name": workflow.name,
                                "step_index": step_index,
                                "step_type": step_type_name(step),
                                "total_steps": workflow.steps.len()
                            });
                            if let ResolvedStep::Prompt {
                                artifact_name: Some(name),
                                ..
                            } = step
                            {
                                d["artifact_name"] = json!(name);
                            }
                            d
                        }),
                    },
                );
                if let Err(err) =
                    record_step_intent(self.project_root, &run_id, &workflow.name, step_index, step)
                {
                    failed_steps.push(step_index);
                    success = false;
                    step_results.push(StepResult {
                        index: step_index,
                        step_type: step_type_name(step).to_string(),
                        status: StepStatus::Failed,
                        duration_ms: 0,
                        exit_code: None,
                        timed_out: false,
                        message: Some(format!("failed to write step intent: {err}")),
                        stdout: None,
                        stderr: None,
                    });
                    break;
                }
                match step {
                    ResolvedStep::Prompt {
                        agent,
                        step_id,
                        text,
                        inject_files,
                        inject_files_raw,
                        session_name,
                        runtime: step_runtime,
                        wait_timeout_secs: step_wait_timeout,
                        artifact_glob,
                        artifact_name,
                        ..
                    } => {
                        let started = std::time::Instant::now();
                        let mut _auto_started_session = false;
                        let rendered = match render_template(text, &outputs, false) {
                            Ok(v) => v,
                            Err(e) => {
                                failed_steps.push(step_index);
                                success = false;
                                step_results.push(StepResult {
                                    index: step_index,
                                    step_type: "prompt".to_string(),
                                    status: StepStatus::Failed,
                                    duration_ms: started.elapsed().as_millis() as u64,
                                    exit_code: None,
                                    timed_out: false,
                                    message: Some(e.to_string()),
                                    stdout: None,
                                    stderr: None,
                                });
                                break;
                            }
                        };

                        if !TmuxSession::session_exists(session_name) {
                            match start_and_wait_ready(
                                self.project_root,
                                self.config,
                                agent,
                                session_name,
                            ) {
                                Ok(()) => _auto_started_session = true,
                                Err(e) => {
                                    failed_steps.push(step_index);
                                    success = false;
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "prompt".to_string(),
                                        status: StepStatus::Failed,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: None,
                                        timed_out: false,
                                        message: Some(format!(
                                            "target session is not running and auto-start failed: {} ({e})",
                                            session_name
                                        )),
                                        stdout: None,
                                        stderr: None,
                                    });
                                    break;
                                }
                            }
                        }

                        // Template-expand inject_files (supports {{output.step_id.path}}).
                        // We expand the RAW strings first, then resolve paths. If an expanded
                        // path is absolute (e.g. from an artifact output), use it directly
                        // instead of joining to project_root.
                        let expanded_inject_files: Vec<PromptInjectedFile> = match inject_files_raw
                            .iter()
                            .zip(inject_files.iter())
                            .map(|(raw, resolved)| {
                                let expanded = render_template(raw, &outputs, false)?;
                                let expanded_path = PathBuf::from(&expanded);
                                if expanded_path.is_absolute() {
                                    // Absolute path from artifact output — use directly
                                    let fname = expanded_path.file_name().ok_or_else(|| {
                                        TuttiError::ConfigValidation(format!(
                                            "inject_files expanded path has no filename: {}",
                                            expanded_path.display()
                                        ))
                                    })?;
                                    Ok(PromptInjectedFile {
                                        source: expanded_path.clone(),
                                        destination: resolved
                                            .destination
                                            .parent()
                                            .unwrap_or(Path::new("."))
                                            .join(fname),
                                    })
                                } else {
                                    // Relative path — use the pre-resolved version
                                    Ok(resolved.clone())
                                }
                            })
                            .collect::<Result<Vec<_>>>()
                        {
                            Ok(v) => v,
                            Err(e) => {
                                failed_steps.push(step_index);
                                success = false;
                                step_results.push(StepResult {
                                    index: step_index,
                                    step_type: "prompt".to_string(),
                                    status: StepStatus::Failed,
                                    duration_ms: started.elapsed().as_millis() as u64,
                                    exit_code: None,
                                    timed_out: false,
                                    message: Some(format!(
                                        "inject_files template expansion failed: {e}"
                                    )),
                                    stdout: None,
                                    stderr: None,
                                });
                                break;
                            }
                        };
                        if let Err(e) = inject_prompt_files(&expanded_inject_files) {
                            failed_steps.push(step_index);
                            success = false;
                            step_results.push(StepResult {
                                index: step_index,
                                step_type: "prompt".to_string(),
                                status: StepStatus::Failed,
                                duration_ms: started.elapsed().as_millis() as u64,
                                exit_code: None,
                                timed_out: false,
                                message: Some(e.to_string()),
                                stdout: None,
                                stderr: None,
                            });
                            break;
                        }

                        // Remove stale output_json before sending prompt so wait
                        // helpers don't short-circuit on a leftover file from a
                        // previous attempt.
                        if let ResolvedStep::Prompt {
                            output_json: Some(path),
                            ..
                        } = step
                        {
                            let _ = std::fs::remove_file(path);
                        }

                        // Pre-step artifact snapshot: record existing files before sending
                        let artifact_pre_snapshot = if let (Some(glob_pat), Some(_)) =
                            (artifact_glob.as_deref(), artifact_name.as_deref())
                        {
                            match expand_artifact_glob(glob_pat, &self.config.workspace.name, agent)
                            {
                                Ok(expanded) => match snapshot_artifact_glob(&expanded) {
                                    Ok(snap) => Some((expanded.clone(), snap)),
                                    Err(e) => {
                                        failed_steps.push(step_index);
                                        success = false;
                                        step_results.push(StepResult {
                                            index: step_index,
                                            step_type: "prompt".to_string(),
                                            status: StepStatus::Failed,
                                            duration_ms: started.elapsed().as_millis() as u64,
                                            exit_code: None,
                                            timed_out: false,
                                            message: Some(format!(
                                                "artifact pre-snapshot failed: {e}"
                                            )),
                                            stdout: None,
                                            stderr: None,
                                        });
                                        break;
                                    }
                                },
                                Err(e) => {
                                    failed_steps.push(step_index);
                                    success = false;
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "prompt".to_string(),
                                        status: StepStatus::Failed,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: None,
                                        timed_out: false,
                                        message: Some(format!(
                                            "artifact_glob expansion failed: {e}"
                                        )),
                                        stdout: None,
                                        stderr: None,
                                    });
                                    break;
                                }
                            }
                        } else {
                            None
                        };

                        let baseline_pane_hash =
                            TmuxSession::capture_pane(session_name, PROMPT_CAPTURE_LINES)
                                .ok()
                                .map(|output| hash_output(&output));

                        if let Err(e) = TmuxSession::send_text(session_name, &rendered) {
                            failed_steps.push(step_index);
                            success = false;
                            step_results.push(StepResult {
                                index: step_index,
                                step_type: "prompt".to_string(),
                                status: StepStatus::Failed,
                                duration_ms: started.elapsed().as_millis() as u64,
                                exit_code: None,
                                timed_out: false,
                                message: Some(e.to_string()),
                                stdout: None,
                                stderr: None,
                            });
                            break;
                        }

                        if let ResolvedStep::Prompt {
                            runtime,
                            output_json,
                            wait_for_idle,
                            wait_timeout_secs,
                            startup_grace_secs,
                            artifact_glob: step_artifact_glob,
                            artifact_name: step_artifact_name,
                            ..
                        } = step
                        {
                            if runtime == "codex" || runtime == "claude-code" {
                                maybe_submit_buffered_prompt(session_name, &rendered)?;
                            }

                            // Artifact-polling mode: artifact_glob set but wait_for_idle is false.
                            // Poll for the artifact file instead of idle detection.
                            // This supports interactive skills (e.g. /office-hours) where the
                            // agent goes idle while waiting for human input.
                            let use_artifact_polling = step_artifact_glob.is_some()
                                && step_artifact_name.is_some()
                                && !*wait_for_idle;

                            if use_artifact_polling
                                && let Some((ref expanded_pattern, ref pre_snap)) =
                                    artifact_pre_snapshot
                            {
                                    let poll_interval = Duration::from_secs(5);
                                    let deadline = Duration::from_secs(*wait_timeout_secs);
                                    let poll_start = std::time::Instant::now();
                                    let art_name = step_artifact_name.as_deref().unwrap();

                                    eprintln!(
                                        "  artifact-polling mode: waiting up to {}s for new file matching '{}'",
                                        wait_timeout_secs, expanded_pattern
                                    );

                                    let mut artifact_found = false;
                                    while poll_start.elapsed() < deadline {
                                        std::thread::sleep(poll_interval);

                                        // Check if session is still alive
                                        if !TmuxSession::session_exists(session_name) {
                                            // Session exited — check if artifact was produced
                                            // before breaking
                                            if let Ok(artifact_path) = capture_artifact(
                                                pre_snap,
                                                expanded_pattern,
                                                art_name,
                                            ) {
                                                match store_artifact_output(
                                                    self.project_root,
                                                    &run_id,
                                                    art_name,
                                                    &artifact_path,
                                                ) {
                                                    Ok(saved) => {
                                                        output_files.insert(
                                                            art_name.to_string(),
                                                            saved.path.display().to_string(),
                                                        );
                                                        outputs.insert(art_name.to_string(), saved);
                                                        artifact_found = true;
                                                    }
                                                    Err(e) => {
                                                        failed_steps.push(step_index);
                                                        success = false;
                                                        step_results.push(StepResult {
                                                            index: step_index,
                                                            step_type: "prompt".to_string(),
                                                            status: StepStatus::Failed,
                                                            duration_ms: started.elapsed().as_millis()
                                                                as u64,
                                                            exit_code: None,
                                                            timed_out: false,
                                                            message: Some(format!(
                                                                "artifact store failed for '{}': {e}",
                                                                art_name
                                                            )),
                                                            stdout: None,
                                                            stderr: None,
                                                        });
                                                        break;
                                                    }
                                                }
                                            }
                                            break;
                                        }

                                        // Check if new artifact file has appeared
                                        if let Ok(artifact_path) =
                                            capture_artifact(pre_snap, expanded_pattern, art_name)
                                        {
                                            match store_artifact_output(
                                                self.project_root,
                                                &run_id,
                                                art_name,
                                                &artifact_path,
                                            ) {
                                                Ok(saved) => {
                                                    output_files.insert(
                                                        art_name.to_string(),
                                                        saved.path.display().to_string(),
                                                    );
                                                    outputs.insert(art_name.to_string(), saved);
                                                    artifact_found = true;
                                                }
                                                Err(e) => {
                                                    failed_steps.push(step_index);
                                                    success = false;
                                                    step_results.push(StepResult {
                                                        index: step_index,
                                                        step_type: "prompt".to_string(),
                                                        status: StepStatus::Failed,
                                                        duration_ms: started.elapsed().as_millis()
                                                            as u64,
                                                        exit_code: None,
                                                        timed_out: false,
                                                        message: Some(format!(
                                                            "artifact store failed for '{}': {e}",
                                                            art_name
                                                        )),
                                                        stdout: None,
                                                        stderr: None,
                                                    });
                                                }
                                            }
                                            break;
                                        }
                                    }

                                    if !artifact_found && success {
                                        failed_steps.push(step_index);
                                        success = false;
                                        step_results.push(StepResult {
                                            index: step_index,
                                            step_type: "prompt".to_string(),
                                            status: StepStatus::Failed,
                                            duration_ms: started.elapsed().as_millis() as u64,
                                            exit_code: None,
                                            timed_out: true,
                                            message: Some(format!(
                                                "artifact '{}' not found after {}s of polling",
                                                art_name, wait_timeout_secs
                                            )),
                                            stdout: None,
                                            stderr: None,
                                        });
                                        break;
                                    }

                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "prompt".to_string(),
                                        status: if artifact_found {
                                            StepStatus::Success
                                        } else {
                                            StepStatus::Failed
                                        },
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: if artifact_found { Some(0) } else { None },
                                        timed_out: false,
                                        message: Some(if artifact_found {
                                            format!(
                                                "artifact '{}' captured via polling after {}s",
                                                art_name,
                                                poll_start.elapsed().as_secs()
                                            )
                                        } else {
                                            format!("artifact '{}' failed", art_name)
                                        }),
                                        stdout: None,
                                        stderr: None,
                                    });
                                    continue;
                            }

                            if *wait_for_idle
                                && !wait_for_prompt_activity_or_output(
                                    runtime,
                                    session_name,
                                    &rendered,
                                    baseline_pane_hash,
                                    output_json.as_deref(),
                                    Duration::from_secs((*startup_grace_secs).max(20)),
                                )?
                            {
                                failed_steps.push(step_index);
                                success = false;
                                step_results.push(StepResult {
                                    index: step_index,
                                    step_type: "prompt".to_string(),
                                    status: StepStatus::Failed,
                                    duration_ms: started.elapsed().as_millis() as u64,
                                    exit_code: None,
                                    timed_out: true,
                                    message: Some(format!(
                                        "prompt did not start activity or produce output within {}s",
                                        (*startup_grace_secs).max(20)
                                    )),
                                    stdout: None,
                                    stderr: None,
                                });
                                break;
                            }
                            if *wait_for_idle {
                                let wait = health::wait_for_agent_idle(
                                    runtime,
                                    session_name,
                                    Duration::from_secs((*wait_timeout_secs).max(1)),
                                    Duration::from_secs(5),
                                    Duration::from_secs(*startup_grace_secs),
                                )?;
                                if !wait.is_completed() {
                                    if let Some(path) = output_json.as_ref()
                                        && path.exists()
                                    {
                                        if let (Some(id), Some(path)) =
                                            (step_id.as_deref(), output_json.as_ref())
                                        {
                                            match load_and_store_output(
                                                self.project_root,
                                                &run_id,
                                                id,
                                                path,
                                            ) {
                                                Ok(saved) => {
                                                    output_files.insert(
                                                        id.to_string(),
                                                        saved.path.display().to_string(),
                                                    );
                                                    outputs.insert(id.to_string(), saved);
                                                }
                                                Err(e) => {
                                                    failed_steps.push(step_index);
                                                    success = false;
                                                    step_results.push(StepResult {
                                                        index: step_index,
                                                        step_type: "prompt".to_string(),
                                                        status: StepStatus::Failed,
                                                        duration_ms: started.elapsed().as_millis()
                                                            as u64,
                                                        exit_code: None,
                                                        timed_out: false,
                                                        message: Some(e.to_string()),
                                                        stdout: None,
                                                        stderr: None,
                                                    });
                                                    break;
                                                }
                                            }
                                        }

                                        step_results.push(StepResult {
                                            index: step_index,
                                            step_type: "prompt".to_string(),
                                            status: StepStatus::Success,
                                            duration_ms: started.elapsed().as_millis() as u64,
                                            exit_code: Some(0),
                                            timed_out: false,
                                            message: Some(
                                                "output_json detected after wait_for_idle timeout"
                                                    .to_string(),
                                            ),
                                            stdout: None,
                                            stderr: None,
                                        });
                                        continue;
                                    }
                                    let (timed_out, message) = match wait.failure_reason {
                                        Some(WaitFailureReason::IdleTimeout) => (
                                            true,
                                            format!(
                                                "wait_for_idle timed out after {}s",
                                                wait_timeout_secs
                                            ),
                                        ),
                                        Some(WaitFailureReason::AuthFailed) => (
                                            false,
                                            format!(
                                                "wait_for_idle auth_failed: {}",
                                                wait.detail.as_deref().unwrap_or("unknown")
                                            ),
                                        ),
                                        Some(WaitFailureReason::SessionExited) => (
                                            false,
                                            "wait_for_idle failed: target session exited"
                                                .to_string(),
                                        ),
                                        None => {
                                            (false, "wait_for_idle failed: unknown".to_string())
                                        }
                                    };
                                    failed_steps.push(step_index);
                                    success = false;
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "prompt".to_string(),
                                        status: StepStatus::Failed,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: None,
                                        timed_out,
                                        message: Some(message),
                                        stdout: None,
                                        stderr: None,
                                    });
                                    break;
                                }
                            }
                            if let (Some(id), Some(path)) =
                                (step_id.as_deref(), output_json.as_ref())
                            {
                                wait_for_prompt_output_file(
                                    session_name,
                                    runtime,
                                    &rendered,
                                    path,
                                    Duration::from_secs(15),
                                )?;
                                if !path.exists() {
                                    let retry_prompt = format!(
                                        "You returned without writing the required output file at {}. Do not explain or summarize. Write that JSON file now, fully overwrite it, then reply with a brief confirmation.",
                                        path.display()
                                    );
                                    if !TmuxSession::session_exists(session_name) {
                                        start_and_wait_ready(
                                            self.project_root,
                                            self.config,
                                            agent,
                                            session_name,
                                        )?;
                                    }
                                    TmuxSession::send_text(session_name, &retry_prompt)?;
                                    maybe_submit_buffered_prompt(session_name, &retry_prompt)?;

                                    wait_for_prompt_output_file(
                                        session_name,
                                        runtime,
                                        &retry_prompt,
                                        path,
                                        Duration::from_secs(120),
                                    )?;
                                    if !path.exists() {
                                        failed_steps.push(step_index);
                                        success = false;
                                        step_results.push(StepResult {
                                            index: step_index,
                                            step_type: "prompt".to_string(),
                                            status: StepStatus::Failed,
                                            duration_ms: started.elapsed().as_millis() as u64,
                                            exit_code: None,
                                            timed_out: true,
                                            message: Some(format!(
                                                "{id} retry did not produce output within 120s"
                                            )),
                                            stdout: None,
                                            stderr: None,
                                        });
                                        break;
                                    }
                                }
                                match load_and_store_output(self.project_root, &run_id, id, path) {
                                    Ok(saved) => {
                                        output_files.insert(
                                            id.to_string(),
                                            saved.path.display().to_string(),
                                        );
                                        outputs.insert(id.to_string(), saved);
                                    }
                                    Err(e) => {
                                        failed_steps.push(step_index);
                                        success = false;
                                        step_results.push(StepResult {
                                            index: step_index,
                                            step_type: "prompt".to_string(),
                                            status: StepStatus::Failed,
                                            duration_ms: started.elapsed().as_millis() as u64,
                                            exit_code: None,
                                            timed_out: false,
                                            message: Some(e.to_string()),
                                            stdout: None,
                                            stderr: None,
                                        });
                                        break;
                                    }
                                }

                                if step_id.as_deref() == Some("implement_code")
                                    && !prompt_step_has_branch_progress(self.project_root, agent)?
                                {
                                    if prompt_step_has_worktree_changes(self.project_root, agent)?
                                        && finalize_implementer_worktree_changes(
                                            self.project_root,
                                            agent,
                                        )?
                                        && prompt_step_has_branch_progress(
                                            self.project_root,
                                            agent,
                                        )?
                                    {
                                        continue;
                                    }
                                    let retry_prompt = "Your previous attempt produced no commit beyond the branch baseline in .tutti/state/auto/branch.json. You are not done yet. If your worktree already has local code changes, do not keep exploring the repo. Review only the existing diff, keep the smallest valid slice, then stage it, commit it, and push it to the target branch now. If there are no useful local changes yet, make the smallest coherent code change now, commit it, and push it. If no valid code change is possible, reply with 'BLOCKED:' and the exact reason.";
                                    if !TmuxSession::session_exists(session_name) {
                                        start_and_wait_ready(
                                            self.project_root,
                                            self.config,
                                            agent,
                                            session_name,
                                        )?;
                                    }
                                    TmuxSession::send_text(session_name, retry_prompt)?;
                                    maybe_submit_buffered_prompt(session_name, retry_prompt)?;

                                    if !wait_for_prompt_activity_or_output(
                                        runtime,
                                        session_name,
                                        retry_prompt,
                                        None,
                                        output_json.as_deref(),
                                        Duration::from_secs(60),
                                    )? {
                                        failed_steps.push(step_index);
                                        success = false;
                                        step_results.push(StepResult {
                                            index: step_index,
                                            step_type: "prompt".to_string(),
                                            status: StepStatus::Failed,
                                            duration_ms: started.elapsed().as_millis() as u64,
                                            exit_code: None,
                                            timed_out: true,
                                            message: Some(
                                                "implement_code retry did not start activity or produce output within 60s"
                                                    .to_string(),
                                            ),
                                            stdout: None,
                                            stderr: None,
                                        });
                                        break;
                                    }

                                    let retry_wait = health::wait_for_agent_idle(
                                        runtime,
                                        session_name,
                                        Duration::from_secs((*wait_timeout_secs).max(1)),
                                        Duration::from_secs(5),
                                        Duration::from_secs(10),
                                    )?;
                                    if !retry_wait.is_completed()
                                        || !prompt_step_has_branch_progress(
                                            self.project_root,
                                            agent,
                                        )?
                                    {
                                        failed_steps.push(step_index);
                                        success = false;
                                        step_results.push(StepResult {
                                            index: step_index,
                                            step_type: "prompt".to_string(),
                                            status: StepStatus::Failed,
                                            duration_ms: started.elapsed().as_millis() as u64,
                                            exit_code: None,
                                            timed_out: false,
                                            message: Some(
                                                "implement_code completed without a commit beyond the branch baseline"
                                                    .to_string(),
                                            ),
                                            stdout: None,
                                            stderr: None,
                                        });
                                        break;
                                    }
                                }
                            }
                        }

                        if step_id.as_deref() == Some("implement_code")
                            && !prompt_step_has_branch_progress(self.project_root, agent)?
                        {
                            if prompt_step_has_worktree_changes(self.project_root, agent)?
                                && finalize_implementer_worktree_changes(self.project_root, agent)?
                                && prompt_step_has_branch_progress(self.project_root, agent)?
                            {
                                // Run artifact capture before early success exit
                                if let (Some((expanded_pattern, pre_snap)), Some(art_name)) =
                                    (artifact_pre_snapshot.as_ref(), artifact_name.as_deref())
                                {
                                    std::thread::sleep(Duration::from_secs(2));
                                    if let Ok(artifact_path) =
                                        capture_artifact(pre_snap, expanded_pattern, art_name)
                                        && let Ok(saved) = store_artifact_output(
                                            self.project_root,
                                            &run_id,
                                            art_name,
                                            &artifact_path,
                                        )
                                    {
                                        output_files.insert(
                                            art_name.to_string(),
                                            saved.path.display().to_string(),
                                        );
                                        outputs.insert(art_name.to_string(), saved);
                                    }
                                }
                                step_results.push(StepResult {
                                    index: step_index,
                                    step_type: "prompt".to_string(),
                                    status: StepStatus::Success,
                                    duration_ms: started.elapsed().as_millis() as u64,
                                    exit_code: Some(0),
                                    timed_out: false,
                                    message: Some(
                                        "implement_code diff finalized and pushed by Tutti"
                                            .to_string(),
                                    ),
                                    stdout: None,
                                    stderr: None,
                                });
                                continue;
                            }
                            let retry_prompt = "You still have not produced a commit beyond the branch baseline in .tutti/state/auto/branch.json. If your worktree already has local code changes, stop exploring and commit the smallest valid diff now. Otherwise make the smallest coherent code change now, commit it, and push it to the target branch. If no valid code change is possible, reply with 'BLOCKED:' and the exact reason.";
                            if !TmuxSession::session_exists(session_name) {
                                start_and_wait_ready(
                                    self.project_root,
                                    self.config,
                                    agent,
                                    session_name,
                                )?;
                            }
                            TmuxSession::send_text(session_name, retry_prompt)?;
                            maybe_submit_buffered_prompt(session_name, retry_prompt)?;

                            if !wait_for_prompt_activity_or_output(
                                step_runtime,
                                session_name,
                                retry_prompt,
                                None,
                                None,
                                Duration::from_secs(60),
                            )? {
                                if prompt_step_has_worktree_changes(self.project_root, agent)?
                                    && finalize_implementer_worktree_changes(
                                        self.project_root,
                                        agent,
                                    )?
                                    && prompt_step_has_branch_progress(self.project_root, agent)?
                                {
                                    // Run artifact capture before early success exit
                                    if let (Some((expanded_pattern, pre_snap)), Some(art_name)) =
                                        (artifact_pre_snapshot.as_ref(), artifact_name.as_deref())
                                    {
                                        std::thread::sleep(Duration::from_secs(2));
                                        if let Ok(artifact_path) =
                                            capture_artifact(pre_snap, expanded_pattern, art_name)
                                            && let Ok(saved) = store_artifact_output(
                                                self.project_root,
                                                &run_id,
                                                art_name,
                                                &artifact_path,
                                            )
                                        {
                                            output_files.insert(
                                                art_name.to_string(),
                                                saved.path.display().to_string(),
                                            );
                                            outputs.insert(art_name.to_string(), saved);
                                        }
                                    }
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "prompt".to_string(),
                                        status: StepStatus::Success,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: Some(0),
                                        timed_out: false,
                                        message: Some(
                                            "implement_code retry diff finalized and pushed by Tutti"
                                                .to_string(),
                                        ),
                                        stdout: None,
                                        stderr: None,
                                    });
                                    continue;
                                }
                                failed_steps.push(step_index);
                                success = false;
                                step_results.push(StepResult {
                                    index: step_index,
                                    step_type: "prompt".to_string(),
                                    status: StepStatus::Failed,
                                    duration_ms: started.elapsed().as_millis() as u64,
                                    exit_code: None,
                                    timed_out: true,
                                    message: Some(
                                        "implement_code retry did not start activity within 60s"
                                            .to_string(),
                                    ),
                                    stdout: None,
                                    stderr: None,
                                });
                                break;
                            }

                            let retry_wait = health::wait_for_agent_idle(
                                step_runtime,
                                session_name,
                                Duration::from_secs((*step_wait_timeout).max(1)),
                                Duration::from_secs(5),
                                Duration::from_secs(10),
                            )?;
                            if !retry_wait.is_completed()
                                || !prompt_step_has_branch_progress(self.project_root, agent)?
                            {
                                failed_steps.push(step_index);
                                success = false;
                                step_results.push(StepResult {
                                    index: step_index,
                                    step_type: "prompt".to_string(),
                                    status: StepStatus::Failed,
                                    duration_ms: started.elapsed().as_millis() as u64,
                                    exit_code: None,
                                    timed_out: false,
                                    message: Some(
                                        "implement_code completed without a commit beyond the branch baseline"
                                            .to_string(),
                                    ),
                                    stdout: None,
                                    stderr: None,
                                });
                                break;
                            }
                        }

                        // Post-step artifact capture
                        if let (Some((expanded_pattern, pre_snap)), Some(art_name)) =
                            (artifact_pre_snapshot.as_ref(), artifact_name.as_deref())
                        {
                            // Brief settle time for filesystem I/O
                            std::thread::sleep(Duration::from_secs(2));

                            match capture_artifact(pre_snap, expanded_pattern, art_name) {
                                Ok(artifact_path) => {
                                    match store_artifact_output(
                                        self.project_root,
                                        &run_id,
                                        art_name,
                                        &artifact_path,
                                    ) {
                                        Ok(saved) => {
                                            output_files.insert(
                                                art_name.to_string(),
                                                saved.path.display().to_string(),
                                            );
                                            outputs.insert(art_name.to_string(), saved);
                                        }
                                        Err(e) => {
                                            failed_steps.push(step_index);
                                            success = false;
                                            step_results.push(StepResult {
                                                index: step_index,
                                                step_type: "prompt".to_string(),
                                                status: StepStatus::Failed,
                                                duration_ms: started.elapsed().as_millis() as u64,
                                                exit_code: None,
                                                timed_out: false,
                                                message: Some(format!(
                                                    "artifact capture failed for '{}': {e}",
                                                    art_name
                                                )),
                                                stdout: None,
                                                stderr: None,
                                            });
                                            break;
                                        }
                                    }
                                }
                                Err(e) => {
                                    failed_steps.push(step_index);
                                    success = false;
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "prompt".to_string(),
                                        status: StepStatus::Failed,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: None,
                                        timed_out: false,
                                        message: Some(e.to_string()),
                                        stdout: None,
                                        stderr: None,
                                    });
                                    break;
                                }
                            }
                        }

                        step_results.push(StepResult {
                            index: step_index,
                            step_type: "prompt".to_string(),
                            status: StepStatus::Success,
                            duration_ms: started.elapsed().as_millis() as u64,
                            exit_code: Some(0),
                            timed_out: false,
                            message: None,
                            stdout: None,
                            stderr: None,
                        });
                    }
                    ResolvedStep::Command {
                        step_id,
                        run,
                        cwd,
                        agent,
                        timeout_secs,
                        fail_mode,
                        ..
                    } => {
                        let started = std::time::Instant::now();
                        let rendered = match render_template(run, &outputs, true) {
                            Ok(v) => v,
                            Err(e) => match fail_mode {
                                WorkflowFailMode::Open => {
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "command".to_string(),
                                        status: StepStatus::Warning,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: None,
                                        timed_out: false,
                                        message: Some(e.to_string()),
                                        stdout: None,
                                        stderr: None,
                                    });
                                    continue;
                                }
                                WorkflowFailMode::Closed => {
                                    failed_steps.push(step_index);
                                    success = false;
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "command".to_string(),
                                        status: StepStatus::Failed,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: None,
                                        timed_out: false,
                                        message: Some(e.to_string()),
                                        stdout: None,
                                        stderr: None,
                                    });
                                    break;
                                }
                            },
                        };

                        let policy_ctx = WorkflowCommandPolicyContext {
                            project_root: self.project_root,
                            workspace: &self.config.workspace.name,
                            workflow_name: &workflow.name,
                            run_id: &run_id,
                            step_index,
                            agent: agent.as_deref(),
                            policy: options.command_policy.as_ref(),
                        };
                        let policy_decision =
                            evaluate_workflow_command_policy(policy_ctx, &rendered);
                        if !policy_decision.allowed {
                            let mut message = format!(
                                "command blocked by permissions policy: '{}'",
                                policy_decision.command
                            );
                            if let Some(rule) = policy_decision.suggested_rule.as_deref() {
                                message.push_str(&format!(" (hint: add allow rule '{rule}')"));
                            }
                            match fail_mode {
                                WorkflowFailMode::Open => {
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "command".to_string(),
                                        status: StepStatus::Warning,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: None,
                                        timed_out: false,
                                        message: Some(message),
                                        stdout: None,
                                        stderr: None,
                                    });
                                    continue;
                                }
                                WorkflowFailMode::Closed => {
                                    failed_steps.push(step_index);
                                    success = false;
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "command".to_string(),
                                        status: StepStatus::Failed,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: None,
                                        timed_out: false,
                                        message: Some(message),
                                        stdout: None,
                                        stderr: None,
                                    });
                                    break;
                                }
                            }
                        }

                        let mut cmd_result = run_shell_command_with_retry(
                            &rendered,
                            cwd,
                            *timeout_secs,
                            options.retry_policy.as_ref(),
                        );

                        if let Ok(result) = &cmd_result {
                            let failed = result.outcome.timed_out
                                || result.outcome.exit_code.unwrap_or(1) != 0;
                            if failed
                                && matches!(fail_mode, WorkflowFailMode::Closed)
                                && step_uses_validation_repair(step_id.as_deref())
                                && prompt_step_has_branch_progress(
                                    self.project_root,
                                    "implementer",
                                )?
                                && let Some(repaired) = attempt_validation_repair(
                                    self.config,
                                    self.project_root,
                                    &run_id,
                                    step_id.as_deref().unwrap_or("validation"),
                                    &rendered,
                                    cwd,
                                    *timeout_secs,
                                    options.retry_policy.as_ref(),
                                    &result.outcome,
                                )?
                            {
                                cmd_result = Ok(repaired);
                            }
                        }

                        match cmd_result {
                            Ok(cmd_outcome) => {
                                let outcome = cmd_outcome.outcome;
                                let failed =
                                    outcome.timed_out || outcome.exit_code.unwrap_or(1) != 0;
                                if failed {
                                    let base_message = if outcome.timed_out {
                                        format!(
                                            "command timed out after {}s: {}",
                                            timeout_secs, rendered
                                        )
                                    } else {
                                        format!(
                                            "command failed (exit {}): {}",
                                            outcome.exit_code.unwrap_or(-1),
                                            rendered
                                        )
                                    };
                                    let message = if cmd_outcome.attempts > 1 {
                                        format!(
                                            "{base_message} (after {} attempts)",
                                            cmd_outcome.attempts
                                        )
                                    } else {
                                        base_message
                                    };

                                    match fail_mode {
                                        WorkflowFailMode::Open => {
                                            step_results.push(StepResult {
                                                index: step_index,
                                                step_type: "command".to_string(),
                                                status: StepStatus::Warning,
                                                duration_ms: started.elapsed().as_millis() as u64,
                                                exit_code: outcome.exit_code,
                                                timed_out: outcome.timed_out,
                                                message: Some(message),
                                                stdout: Some(outcome.stdout),
                                                stderr: Some(outcome.stderr),
                                            });
                                        }
                                        WorkflowFailMode::Closed => {
                                            failed_steps.push(step_index);
                                            success = false;
                                            step_results.push(StepResult {
                                                index: step_index,
                                                step_type: "command".to_string(),
                                                status: StepStatus::Failed,
                                                duration_ms: started.elapsed().as_millis() as u64,
                                                exit_code: outcome.exit_code,
                                                timed_out: outcome.timed_out,
                                                message: Some(message),
                                                stdout: Some(outcome.stdout),
                                                stderr: Some(outcome.stderr),
                                            });
                                            break;
                                        }
                                    }
                                } else {
                                    if let ResolvedStep::Command { output_json, .. } = step
                                        && let (Some(id), Some(path)) =
                                            (step_id.as_deref(), output_json.as_ref())
                                    {
                                        match load_and_store_output(
                                            self.project_root,
                                            &run_id,
                                            id,
                                            path,
                                        ) {
                                            Ok(saved) => {
                                                output_files.insert(
                                                    id.to_string(),
                                                    saved.path.display().to_string(),
                                                );
                                                outputs.insert(id.to_string(), saved);
                                            }
                                            Err(e) => match fail_mode {
                                                WorkflowFailMode::Open => {
                                                    step_results.push(StepResult {
                                                        index: step_index,
                                                        step_type: "command".to_string(),
                                                        status: StepStatus::Warning,
                                                        duration_ms: started.elapsed().as_millis()
                                                            as u64,
                                                        exit_code: outcome.exit_code,
                                                        timed_out: false,
                                                        message: Some(e.to_string()),
                                                        stdout: Some(outcome.stdout),
                                                        stderr: Some(outcome.stderr),
                                                    });
                                                    continue;
                                                }
                                                WorkflowFailMode::Closed => {
                                                    failed_steps.push(step_index);
                                                    success = false;
                                                    step_results.push(StepResult {
                                                        index: step_index,
                                                        step_type: "command".to_string(),
                                                        status: StepStatus::Failed,
                                                        duration_ms: started.elapsed().as_millis()
                                                            as u64,
                                                        exit_code: outcome.exit_code,
                                                        timed_out: false,
                                                        message: Some(e.to_string()),
                                                        stdout: Some(outcome.stdout),
                                                        stderr: Some(outcome.stderr),
                                                    });
                                                    break;
                                                }
                                            },
                                        }
                                    }
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "command".to_string(),
                                        status: StepStatus::Success,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: outcome.exit_code,
                                        timed_out: false,
                                        message: None,
                                        stdout: Some(outcome.stdout),
                                        stderr: Some(outcome.stderr),
                                    });
                                }
                            }
                            Err(e) => match fail_mode {
                                WorkflowFailMode::Open => {
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "command".to_string(),
                                        status: StepStatus::Warning,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: None,
                                        timed_out: false,
                                        message: Some(e.to_string()),
                                        stdout: None,
                                        stderr: None,
                                    });
                                }
                                WorkflowFailMode::Closed => {
                                    failed_steps.push(step_index);
                                    success = false;
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "command".to_string(),
                                        status: StepStatus::Failed,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: None,
                                        timed_out: false,
                                        message: Some(e.to_string()),
                                        stdout: None,
                                        stderr: None,
                                    });
                                    break;
                                }
                            },
                        }
                    }
                    ResolvedStep::EnsureRunning {
                        agent,
                        session_name,
                        fail_mode,
                        ..
                    } => {
                        let started = std::time::Instant::now();
                        if TmuxSession::session_exists(session_name) {
                            step_results.push(StepResult {
                                index: step_index,
                                step_type: "ensure_running".to_string(),
                                status: StepStatus::Success,
                                duration_ms: started.elapsed().as_millis() as u64,
                                exit_code: Some(0),
                                timed_out: false,
                                message: Some(format!("agent '{}' already running", agent)),
                                stdout: None,
                                stderr: None,
                            });
                            continue;
                        }

                        match start_and_wait_ready(
                            self.project_root,
                            self.config,
                            agent,
                            session_name,
                        ) {
                            Ok(()) => {
                                step_results.push(StepResult {
                                    index: step_index,
                                    step_type: "ensure_running".to_string(),
                                    status: StepStatus::Success,
                                    duration_ms: started.elapsed().as_millis() as u64,
                                    exit_code: Some(0),
                                    timed_out: false,
                                    message: Some(format!("started agent '{}'", agent)),
                                    stdout: None,
                                    stderr: None,
                                });
                            }
                            Err(e) => {
                                let message = format!("failed to start '{}': {e}", agent);
                                if *fail_mode == WorkflowFailMode::Open {
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "ensure_running".to_string(),
                                        status: StepStatus::Warning,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: None,
                                        timed_out: false,
                                        message: Some(message),
                                        stdout: None,
                                        stderr: None,
                                    });
                                } else {
                                    failed_steps.push(step_index);
                                    success = false;
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "ensure_running".to_string(),
                                        status: StepStatus::Failed,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: None,
                                        timed_out: false,
                                        message: Some(message),
                                        stdout: None,
                                        stderr: None,
                                    });
                                    break;
                                }
                            }
                        }
                    }
                    ResolvedStep::Workflow {
                        workflow: nested_workflow,
                        agent_override,
                        strict,
                        fail_mode,
                        ..
                    } => {
                        let started = std::time::Instant::now();
                        let nested_options = ExecuteOptions {
                            strict: *strict,
                            force_open_commands: options.force_open_commands,
                            command_policy: options.command_policy.clone(),
                            retry_policy: options.retry_policy,
                            origin: options.origin,
                            hook_event: options.hook_event.clone(),
                            hook_agent: options.hook_agent.clone(),
                        };
                        let nested_result = WorkflowResolver::new(self.config, self.project_root)
                            .resolve(nested_workflow, agent_override.as_deref(), &nested_options)
                            .and_then(|resolved_nested| {
                                execute_workflow_with_hooks(
                                    self.config,
                                    self.project_root,
                                    &resolved_nested,
                                    &nested_options,
                                    agent_override.as_deref(),
                                    None,
                                )
                            });

                        match nested_result {
                            Ok(nested) => {
                                if nested.success {
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "workflow".to_string(),
                                        status: StepStatus::Success,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: Some(0),
                                        timed_out: false,
                                        message: Some(format!(
                                            "nested workflow '{}' succeeded",
                                            nested.workflow_name
                                        )),
                                        stdout: None,
                                        stderr: None,
                                    });
                                } else if *fail_mode == WorkflowFailMode::Open {
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "workflow".to_string(),
                                        status: StepStatus::Warning,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: Some(1),
                                        timed_out: false,
                                        message: Some(format!(
                                            "nested workflow '{}' failed",
                                            nested.workflow_name
                                        )),
                                        stdout: None,
                                        stderr: None,
                                    });
                                } else {
                                    failed_steps.push(step_index);
                                    success = false;
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "workflow".to_string(),
                                        status: StepStatus::Failed,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: Some(1),
                                        timed_out: false,
                                        message: Some(format!(
                                            "nested workflow '{}' failed",
                                            nested.workflow_name
                                        )),
                                        stdout: None,
                                        stderr: None,
                                    });
                                    break;
                                }
                            }
                            Err(e) => {
                                if *fail_mode == WorkflowFailMode::Open {
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "workflow".to_string(),
                                        status: StepStatus::Warning,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: Some(1),
                                        timed_out: false,
                                        message: Some(e.to_string()),
                                        stdout: None,
                                        stderr: None,
                                    });
                                } else {
                                    failed_steps.push(step_index);
                                    success = false;
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "workflow".to_string(),
                                        status: StepStatus::Failed,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: Some(1),
                                        timed_out: false,
                                        message: Some(e.to_string()),
                                        stdout: None,
                                        stderr: None,
                                    });
                                    break;
                                }
                            }
                        }
                    }
                    ResolvedStep::Land {
                        agent,
                        pr,
                        force,
                        fail_mode,
                        ..
                    } => {
                        let started = std::time::Instant::now();
                        if let Some(intent) = prior_unsuccessful_intent.as_ref()
                            && let Some(message) = should_skip_land_replay(
                                self.config,
                                self.project_root,
                                agent,
                                intent,
                            )
                        {
                            step_results.push(StepResult {
                                index: step_index,
                                step_type: "land".to_string(),
                                status: StepStatus::Success,
                                duration_ms: started.elapsed().as_millis() as u64,
                                exit_code: Some(0),
                                timed_out: false,
                                message: Some(message),
                                stdout: None,
                                stderr: None,
                            });
                            continue;
                        }
                        if let Err(e) =
                            ensure_agent_session_running(self.config, self.project_root, agent)
                        {
                            if *fail_mode == WorkflowFailMode::Open {
                                step_results.push(StepResult {
                                    index: step_index,
                                    step_type: "land".to_string(),
                                    status: StepStatus::Warning,
                                    duration_ms: started.elapsed().as_millis() as u64,
                                    exit_code: Some(1),
                                    timed_out: false,
                                    message: Some(format!(
                                        "failed to auto-start '{}' before land: {e}",
                                        agent
                                    )),
                                    stdout: None,
                                    stderr: None,
                                });
                                continue;
                            } else {
                                failed_steps.push(step_index);
                                success = false;
                                step_results.push(StepResult {
                                    index: step_index,
                                    step_type: "land".to_string(),
                                    status: StepStatus::Failed,
                                    duration_ms: started.elapsed().as_millis() as u64,
                                    exit_code: Some(1),
                                    timed_out: false,
                                    message: Some(format!(
                                        "failed to auto-start '{}' before land: {e}",
                                        agent
                                    )),
                                    stdout: None,
                                    stderr: None,
                                });
                                break;
                            }
                        }

                        match with_project_root(self.project_root, || {
                            crate::cli::land::run_with_options(agent, *pr, *force, true)
                        }) {
                            Ok(()) => step_results.push(StepResult {
                                index: step_index,
                                step_type: "land".to_string(),
                                status: StepStatus::Success,
                                duration_ms: started.elapsed().as_millis() as u64,
                                exit_code: Some(0),
                                timed_out: false,
                                message: Some(format!("landed '{}'", agent)),
                                stdout: None,
                                stderr: None,
                            }),
                            Err(e) => {
                                if *fail_mode == WorkflowFailMode::Open {
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "land".to_string(),
                                        status: StepStatus::Warning,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: Some(1),
                                        timed_out: false,
                                        message: Some(e.to_string()),
                                        stdout: None,
                                        stderr: None,
                                    });
                                } else {
                                    failed_steps.push(step_index);
                                    success = false;
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "land".to_string(),
                                        status: StepStatus::Failed,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: Some(1),
                                        timed_out: false,
                                        message: Some(e.to_string()),
                                        stdout: None,
                                        stderr: None,
                                    });
                                    break;
                                }
                            }
                        }
                    }
                    ResolvedStep::Review {
                        agent,
                        reviewer,
                        fail_mode,
                        ..
                    } => {
                        let started = std::time::Instant::now();
                        if let Some(intent) = prior_unsuccessful_intent.as_ref()
                            && let Some(packet) = review_packet_exists_since(
                                self.project_root,
                                agent,
                                intent.planned_at,
                            )?
                        {
                            step_results.push(StepResult {
                                index: step_index,
                                step_type: "review".to_string(),
                                status: StepStatus::Success,
                                duration_ms: started.elapsed().as_millis() as u64,
                                exit_code: Some(0),
                                timed_out: false,
                                message: Some(format!(
                                    "resume guard: existing review packet detected at {}; skipping replay",
                                    packet.display()
                                )),
                                stdout: None,
                                stderr: None,
                            });
                            continue;
                        }
                        if let Err(e) =
                            ensure_agent_session_running(self.config, self.project_root, reviewer)
                        {
                            if *fail_mode == WorkflowFailMode::Open {
                                step_results.push(StepResult {
                                    index: step_index,
                                    step_type: "review".to_string(),
                                    status: StepStatus::Warning,
                                    duration_ms: started.elapsed().as_millis() as u64,
                                    exit_code: Some(1),
                                    timed_out: false,
                                    message: Some(format!(
                                        "failed to auto-start reviewer '{}': {e}",
                                        reviewer
                                    )),
                                    stdout: None,
                                    stderr: None,
                                });
                                continue;
                            } else {
                                failed_steps.push(step_index);
                                success = false;
                                step_results.push(StepResult {
                                    index: step_index,
                                    step_type: "review".to_string(),
                                    status: StepStatus::Failed,
                                    duration_ms: started.elapsed().as_millis() as u64,
                                    exit_code: Some(1),
                                    timed_out: false,
                                    message: Some(format!(
                                        "failed to auto-start reviewer '{}': {e}",
                                        reviewer
                                    )),
                                    stdout: None,
                                    stderr: None,
                                });
                                break;
                            }
                        }
                        match with_project_root(self.project_root, || {
                            crate::cli::review::run(agent, reviewer)
                        }) {
                            Ok(()) => step_results.push(StepResult {
                                index: step_index,
                                step_type: "review".to_string(),
                                status: StepStatus::Success,
                                duration_ms: started.elapsed().as_millis() as u64,
                                exit_code: Some(0),
                                timed_out: false,
                                message: Some(format!("sent '{}' for review", agent)),
                                stdout: None,
                                stderr: None,
                            }),
                            Err(e) => {
                                if *fail_mode == WorkflowFailMode::Open {
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "review".to_string(),
                                        status: StepStatus::Warning,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: Some(1),
                                        timed_out: false,
                                        message: Some(e.to_string()),
                                        stdout: None,
                                        stderr: None,
                                    });
                                } else {
                                    failed_steps.push(step_index);
                                    success = false;
                                    step_results.push(StepResult {
                                        index: step_index,
                                        step_type: "review".to_string(),
                                        status: StepStatus::Failed,
                                        duration_ms: started.elapsed().as_millis() as u64,
                                        exit_code: Some(1),
                                        timed_out: false,
                                        message: Some(e.to_string()),
                                        stdout: None,
                                        stderr: None,
                                    });
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }

        // Emit step completion events for all steps attempted in this run
        for sr in &step_results {
            if !attempted_steps.contains(&sr.index) {
                continue;
            }
            let step_agent = workflow
                .steps
                .get(sr.index.saturating_sub(1))
                .and_then(|s| step_agent_name(s).map(|a| a.to_string()));
            let event_name = match sr.status {
                StepStatus::Success => "workflow.step.completed",
                StepStatus::Warning => "workflow.step.warning",
                StepStatus::Failed => "workflow.step.failed",
            };
            let _ = append_control_event(
                self.project_root,
                &ControlEvent {
                    event: event_name.to_string(),
                    workspace: self.config.workspace.name.clone(),
                    agent: step_agent,
                    timestamp: Utc::now(),
                    correlation_id: run_id.clone(),
                    data: Some(json!({
                        "workflow_name": workflow.name,
                        "step_index": sr.index,
                        "step_type": sr.step_type,
                        "total_steps": workflow.steps.len(),
                        "duration_ms": sr.duration_ms,
                        "timed_out": sr.timed_out,
                        "message": sr.message
                    })),
                },
            );
        }

        let result = ExecutionResult {
            run_id,
            workflow_name: workflow.name.clone(),
            strict: options.strict,
            success,
            started_at,
            finished_at: Utc::now(),
            failed_steps,
            step_results,
            output_files,
        };

        for step_index in attempted_steps {
            if let Some(step) = workflow.steps.get(step_index.saturating_sub(1))
                && let Some(step_result) =
                    result.step_results.iter().find(|s| s.index == step_index)
                && let Err(err) = record_step_outcome(
                    self.project_root,
                    &result.run_id,
                    &workflow.name,
                    step_index,
                    step,
                    step_result,
                )
            {
                eprintln!(
                    "warn: failed to persist workflow step outcome for run '{}' step {}: {}",
                    result.run_id, step_index, err
                );
            }
        }

        append_automation_run(
            self.project_root,
            &AutomationRunRecord {
                workflow_name: result.workflow_name.clone(),
                timestamp: Utc::now(),
                trigger: options.origin.as_str().to_string(),
                success: result.success,
                strict: options.strict,
                failed_steps: result.failed_steps.clone(),
                warning_count: result.warning_count(),
                agent_scope: agent_scope.map(|s| s.to_string()),
                hook_event: options.hook_event.clone(),
                hook_agent: options.hook_agent.clone(),
            },
        )?;

        save_execution_checkpoint(self.project_root, options, agent_scope, &result)?;

        Ok(result)
    }
}

fn generate_run_id() -> String {
    format!("{}-{}", Utc::now().timestamp_millis(), std::process::id())
}

#[derive(Debug, Clone)]
struct CommandOutcome {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
    timed_out: bool,
}

#[derive(Debug, Clone)]
struct StepOutputValue {
    path: PathBuf,
    json: Value,
}

#[derive(Debug, Clone)]
struct ControlStepOutcome {
    index: usize,
    result: StepResult,
    hard_fail: bool,
}

#[derive(Debug, Clone)]
struct ControlDagOutcome {
    success: bool,
    failed_steps: Vec<usize>,
    step_results: Vec<StepResult>,
}

fn inject_prompt_files(files: &[PromptInjectedFile]) -> Result<()> {
    for file in files {
        if !file.source.exists() {
            return Err(TuttiError::ConfigValidation(format!(
                "inject_files source does not exist: {}",
                file.source.display()
            )));
        }
        if let Some(parent) = file.destination.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if file.source != file.destination {
            std::fs::copy(&file.source, &file.destination)?;
        }
    }
    Ok(())
}

/// Expand tilde and variable placeholders in an artifact glob pattern.
fn expand_artifact_glob(pattern: &str, workspace_name: &str, agent_name: &str) -> Result<String> {
    let home = std::env::var("HOME").map_err(|_| {
        TuttiError::ConfigValidation("HOME environment variable is not set".to_string())
    })?;

    let mut expanded = pattern.to_string();

    // Expand ~ at the start of the pattern
    if expanded.starts_with("~/") {
        expanded = format!("{}/{}", home, &expanded[2..]);
    }

    // Expand {workspace} and {agent}
    expanded = expanded.replace("{workspace}", workspace_name);
    expanded = expanded.replace("{agent}", agent_name);

    // Expand {slug} by shelling out to gstack-slug
    if expanded.contains("{slug}") {
        let slug = resolve_gstack_slug()?;
        expanded = expanded.replace("{slug}", &slug);
    }

    Ok(expanded)
}

/// Validate that gstack-slug is available (for dry-run checks).
pub fn validate_gstack_slug_available() -> Result<()> {
    let home = std::env::var("HOME").unwrap_or_default();
    let slug_bin = PathBuf::from(&home).join(".claude/skills/gstack/bin/gstack-slug");
    if !slug_bin.exists() {
        return Err(TuttiError::ConfigValidation(format!(
            "gstack-slug not found at {} — install gstack or use a full path in artifact_glob instead of {{slug}}",
            slug_bin.display()
        )));
    }
    Ok(())
}

/// Shell out to gstack-slug to resolve the project slug.
fn resolve_gstack_slug() -> Result<String> {
    let home = std::env::var("HOME").unwrap_or_default();
    resolve_gstack_slug_with_home(&home)
}

fn resolve_gstack_slug_with_home(home: &str) -> Result<String> {
    let slug_bin = PathBuf::from(home).join(".claude/skills/gstack/bin/gstack-slug");

    if !slug_bin.exists() {
        return Err(TuttiError::ConfigValidation(format!(
            "gstack-slug not found at {} — install gstack or use a full path in artifact_glob instead of {{slug}}",
            slug_bin.display()
        )));
    }

    let output = Command::new(&slug_bin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| {
            TuttiError::ConfigValidation(format!(
                "failed to run gstack-slug at {}: {e}",
                slug_bin.display()
            ))
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TuttiError::ConfigValidation(format!(
            "gstack-slug exited with {}: {}",
            output.status,
            stderr.trim()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    for line in stdout.lines() {
        if let Some(slug) = line.strip_prefix("SLUG=") {
            return Ok(slug.to_string());
        }
    }

    Err(TuttiError::ConfigValidation(format!(
        "gstack-slug did not output SLUG=<value>; got: {}",
        stdout.trim()
    )))
}

/// Snapshot files matching a glob pattern (for pre-step artifact capture).
fn snapshot_artifact_glob(pattern: &str) -> Result<HashSet<PathBuf>> {
    let paths = glob::glob(pattern).map_err(|e| {
        TuttiError::ConfigValidation(format!("invalid artifact_glob pattern '{}': {e}", pattern))
    })?;
    Ok(paths.filter_map(|p| p.ok()).collect())
}

/// After a step completes, capture the newest artifact that appeared since the pre-step snapshot.
fn capture_artifact(
    pre_snapshot: &HashSet<PathBuf>,
    pattern: &str,
    artifact_name: &str,
) -> Result<PathBuf> {
    let post_files: Vec<PathBuf> = glob::glob(pattern)
        .map_err(|e| {
            TuttiError::ConfigValidation(format!(
                "invalid artifact_glob pattern '{}': {e}",
                pattern
            ))
        })?
        .filter_map(|p| p.ok())
        .filter(|p| !pre_snapshot.contains(p))
        .collect();

    if post_files.is_empty() {
        return Err(TuttiError::ConfigValidation(format!(
            "artifact not found: artifact_glob '{}' matched no new files after step completed for artifact '{}'",
            pattern, artifact_name
        )));
    }

    if post_files.len() > 1 {
        eprintln!(
            "warning: artifact_glob '{}' matched {} new files for '{}', using most recent",
            pattern,
            post_files.len(),
            artifact_name
        );
    }

    let newest = post_files
        .iter()
        .max_by_key(|p| {
            p.metadata()
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        })
        .unwrap(); // safe: post_files is non-empty

    Ok(newest.clone())
}

/// Store an artifact file as a step output value (copy to workflow-outputs and register).
fn store_artifact_output(
    project_root: &Path,
    run_id: &str,
    artifact_name: &str,
    artifact_path: &Path,
) -> Result<StepOutputValue> {
    let body = std::fs::read_to_string(artifact_path).map_err(|e| {
        TuttiError::ConfigValidation(format!(
            "failed reading artifact '{}' at {}: {e}",
            artifact_name,
            artifact_path.display()
        ))
    })?;

    // Store as JSON string value (artifact content may not be valid JSON)
    let json_value = Value::String(body);
    let canonical_path = save_workflow_output(project_root, run_id, artifact_name, &json_value)?;

    // Also copy the raw artifact file alongside the JSON
    let raw_path = canonical_path
        .parent()
        .unwrap_or(Path::new("."))
        .join(format!("{}.raw", artifact_name));
    std::fs::copy(artifact_path, &raw_path).map_err(|e| {
        TuttiError::ConfigValidation(format!(
            "failed copying artifact '{}' to {}: {e}",
            artifact_name,
            raw_path.display()
        ))
    })?;

    Ok(StepOutputValue {
        path: raw_path,
        json: json_value,
    })
}

fn prompt_agent_with_files(
    project_root: &Path,
    config: &TuttiConfig,
    agent: &str,
    text: &str,
    inject_files: &[PromptInjectedFile],
    output_path: Option<&Path>,
    wait_timeout: Duration,
) -> Result<bool> {
    let session_name = TmuxSession::session_name(&config.workspace.name, agent);
    let runtime = config
        .agents
        .iter()
        .find(|a| a.name == agent)
        .and_then(|a| a.resolved_runtime(&config.defaults))
        .unwrap_or_else(|| "unknown".to_string());

    if !TmuxSession::session_exists(&session_name) {
        start_and_wait_ready(project_root, config, agent, &session_name)?;
    }

    inject_prompt_files(inject_files)?;
    if let Some(path) = output_path {
        let _ = std::fs::remove_file(path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
    }

    let baseline_pane_hash = TmuxSession::capture_pane(&session_name, PROMPT_CAPTURE_LINES)
        .ok()
        .map(|output| hash_output(&output));

    TmuxSession::send_text(&session_name, text)?;
    if runtime == "codex" || runtime == "claude-code" {
        maybe_submit_buffered_prompt(&session_name, text)?;
    }

    if !wait_for_prompt_activity_or_output(
        &runtime,
        &session_name,
        text,
        baseline_pane_hash,
        output_path,
        Duration::from_secs(60),
    )? {
        return Ok(false);
    }

    if let Some(path) = output_path {
        wait_for_prompt_output_file(
            &session_name,
            &runtime,
            text,
            path,
            Duration::from_secs(120),
        )?;
        if !path.exists() {
            return Ok(false);
        }
    }

    let wait = health::wait_for_agent_idle(
        &runtime,
        &session_name,
        wait_timeout,
        Duration::from_secs(5),
        Duration::from_secs(10),
    )?;
    Ok(wait.is_completed() || output_path.is_some_and(Path::exists))
}

fn step_uses_validation_repair(step_id: Option<&str>) -> bool {
    matches!(
        step_id,
        Some("test_update_and_validate" | "validate" | "revalidate")
    )
}

fn validation_repair_paths(
    project_root: &Path,
    run_id: &str,
    step_id: &str,
    attempt: u32,
) -> (PathBuf, PathBuf) {
    let repair_dir = project_root
        .join(".tutti")
        .join("state")
        .join("auto")
        .join("validation-repair");
    let base = format!("{run_id}-{}-attempt-{attempt}", sanitize_step_key(step_id));
    (
        repair_dir.join(format!("{base}-failure.md")),
        repair_dir.join(format!("{base}-analysis.md")),
    )
}

fn write_validation_failure_report(
    project_root: &Path,
    run_id: &str,
    step_id: &str,
    command: &str,
    cwd: &Path,
    outcome: &CommandOutcome,
    attempt: u32,
) -> Result<(PathBuf, PathBuf)> {
    let (failure_path, analysis_path) =
        validation_repair_paths(project_root, run_id, step_id, attempt);
    if let Some(parent) = failure_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let body = format!(
        "# Validation Failure\n\n- Run ID: `{run_id}`\n- Step: `{step_id}`\n- Attempt: `{attempt}`\n- Command: `{command}`\n- CWD: `{}`\n- Exit code: `{}`\n- Timed out: `{}`\n\n## stdout\n\n```text\n{}\n```\n\n## stderr\n\n```text\n{}\n```\n",
        cwd.display(),
        outcome
            .exit_code
            .map_or_else(|| "none".to_string(), |code| code.to_string()),
        outcome.timed_out,
        outcome.stdout,
        outcome.stderr,
    );
    std::fs::write(&failure_path, body)?;
    let _ = std::fs::remove_file(&analysis_path);
    Ok((failure_path, analysis_path))
}

fn validation_repair_injected_files(
    project_root: &Path,
    agent: &str,
    files: &[&Path],
) -> Vec<PromptInjectedFile> {
    let destination_root = project_root.join(".tutti").join("worktrees").join(agent);
    files
        .iter()
        .map(|source| {
            let relative = source.strip_prefix(project_root).unwrap_or(source);
            PromptInjectedFile {
                source: (*source).to_path_buf(),
                destination: destination_root.join(relative),
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn attempt_validation_repair(
    config: &TuttiConfig,
    project_root: &Path,
    run_id: &str,
    step_id: &str,
    command: &str,
    cwd: &Path,
    timeout_secs: u64,
    retry_policy: Option<&RetryPolicy>,
    initial_outcome: &CommandOutcome,
) -> Result<Option<CommandRunOutcome>> {
    let mut current_outcome = initial_outcome.clone();

    // Validation repair requires "tester" and "implementer" agents to exist
    let has_tester = config.agents.iter().any(|a| a.name == "tester");
    let has_implementer = config.agents.iter().any(|a| a.name == "implementer");
    if !has_tester || !has_implementer {
        return Ok(None);
    }

    for attempt in 1..=VALIDATION_REPAIR_MAX_ATTEMPTS {
        let (failure_path, analysis_path) = write_validation_failure_report(
            project_root,
            run_id,
            step_id,
            command,
            cwd,
            &current_outcome,
            attempt,
        )?;

        let tester_prompt = format!(
            "Read .tutti/state/auto/validation-repair/{} in your worktree. Diagnose the single root cause of this validation failure and write a concise markdown repair plan to {}. Include: root cause, exact files to change, and the smallest viable fix. Do not modify code.",
            failure_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("validation failure report"),
            analysis_path.display()
        );
        let tester_files =
            validation_repair_injected_files(project_root, "tester", &[failure_path.as_path()]);
        if !prompt_agent_with_files(
            project_root,
            config,
            "tester",
            &tester_prompt,
            &tester_files,
            Some(&analysis_path),
            Duration::from_secs(900),
        )? {
            return Ok(None);
        }

        let branch_path = project_root
            .join(".tutti")
            .join("state")
            .join("auto")
            .join("branch.json");
        let implementer_prompt = format!(
            "Read .tutti/state/auto/validation-repair/{} and .tutti/state/auto/validation-repair/{} in your worktree. Apply only the smallest fix needed for this validation failure on the branch from .tutti/state/auto/branch.json. Then run `{}` in your worktree. If code changed, commit and push it. If your worktree is detached, push with git push origin HEAD:<target-branch>. Do not do broad repo exploration.",
            failure_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("validation failure report"),
            analysis_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("validation analysis"),
            command
        );
        let implementer_files = validation_repair_injected_files(
            project_root,
            "implementer",
            &[
                failure_path.as_path(),
                analysis_path.as_path(),
                branch_path.as_path(),
            ],
        );
        if !prompt_agent_with_files(
            project_root,
            config,
            "implementer",
            &implementer_prompt,
            &implementer_files,
            None,
            Duration::from_secs(1800),
        )? {
            return Ok(None);
        }

        if prompt_step_has_worktree_changes(project_root, "implementer")? {
            let _ = finalize_implementer_worktree_changes(project_root, "implementer")?;
        }

        let rerun = run_shell_command_with_retry(command, cwd, timeout_secs, retry_policy)?;
        let rerun_failed = rerun.outcome.timed_out || rerun.outcome.exit_code.unwrap_or(1) != 0;
        if !rerun_failed {
            return Ok(Some(rerun));
        }
        current_outcome = rerun.outcome;
    }

    Ok(None)
}

fn run_shell_command(command: &str, cwd: &Path, timeout_secs: u64) -> Result<CommandOutcome> {
    let mut child = Command::new("/bin/sh")
        .args(["-lc", command])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| TuttiError::Io(std::io::Error::other("failed to capture command stdout")))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| TuttiError::Io(std::io::Error::other("failed to capture command stderr")))?;

    let timeout = Duration::from_secs(timeout_secs.max(1));
    let (status, timed_out) = match child.wait_timeout(timeout)? {
        Some(status) => (status, false),
        None => {
            let _ = child.kill();
            (child.wait()?, true)
        }
    };

    let mut stdout_buf = String::new();
    let mut stderr_buf = String::new();
    let _ = stdout.read_to_string(&mut stdout_buf);
    let _ = stderr.read_to_string(&mut stderr_buf);

    Ok(CommandOutcome {
        exit_code: status.code(),
        stdout: stdout_buf,
        stderr: stderr_buf,
        timed_out,
    })
}

fn prompt_snippet(rendered: &str) -> Option<String> {
    rendered
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(96).collect::<String>())
}

fn maybe_submit_buffered_prompt(session_name: &str, rendered: &str) -> Result<()> {
    let Some(snippet) = prompt_snippet(rendered) else {
        return Ok(());
    };

    for _ in 0..5 {
        std::thread::sleep(Duration::from_secs(2));
        let pane = match TmuxSession::capture_pane(session_name, 120) {
            Ok(output) => output,
            Err(_) => return Ok(()),
        };

        if pane.contains("Running") || pane.contains("Thinking") || pane.contains("Generating") {
            return Ok(());
        }

        if pane.contains(&snippet) {
            TmuxSession::send_enter_presses(session_name, 2)?;
            continue;
        }

        return Ok(());
    }

    Ok(())
}

fn wait_for_prompt_activity_or_output(
    runtime_name: &str,
    session_name: &str,
    rendered: &str,
    baseline_pane_hash: Option<u64>,
    output_path: Option<&Path>,
    timeout: Duration,
) -> Result<bool> {
    let adapter = runtime::get_adapter(runtime_name, None);
    let snippet = prompt_snippet(rendered);
    let start = std::time::Instant::now();

    while start.elapsed() < timeout {
        if let Some(path) = output_path
            && path.exists()
        {
            return Ok(true);
        }

        let pane = TmuxSession::capture_pane(session_name, PROMPT_CAPTURE_LINES)?;
        let pane_hash = hash_output(&pane);
        let consumed = baseline_pane_hash.is_some_and(|baseline| pane_hash != baseline)
            && snippet.as_ref().is_none_or(|needle| !pane.contains(needle));

        if let Some(adapter) = &adapter
            && matches!(adapter.detect_status(&pane), AgentStatus::Working)
        {
            return Ok(true);
        }

        if consumed {
            return Ok(true);
        }

        if snippet.as_ref().is_some_and(|needle| pane.contains(needle))
            && (runtime_name == "codex" || runtime_name == "claude-code")
        {
            maybe_submit_buffered_prompt(session_name, rendered)?;
        }

        std::thread::sleep(Duration::from_secs(2));
    }

    Ok(false)
}

fn wait_for_prompt_output_file(
    session_name: &str,
    runtime_name: &str,
    rendered: &str,
    path: &Path,
    grace: Duration,
) -> Result<()> {
    if path.exists() {
        return Ok(());
    }

    let deadline = std::time::Instant::now() + grace;
    while std::time::Instant::now() < deadline {
        if (runtime_name == "codex" || runtime_name == "claude-code") && !path.exists() {
            maybe_submit_buffered_prompt(session_name, rendered)?;
        }
        if path.exists() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_secs(2));
    }

    Ok(())
}

fn hash_output(output: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    output.hash(&mut hasher);
    hasher.finish()
}

fn prompt_step_has_branch_progress(project_root: &Path, agent: &str) -> Result<bool> {
    let branch_file = project_root
        .join(".tutti")
        .join("state")
        .join("auto")
        .join("branch.json");
    if !branch_file.exists() {
        return Ok(true);
    }

    let branch_state: WorkflowBranchState =
        serde_json::from_slice(&std::fs::read(&branch_file)?)
            .map_err(|e| TuttiError::ConfigValidation(e.to_string()))?;

    let worktree_dir = project_root.join(".tutti").join("worktrees").join(agent);
    if !worktree_dir.exists() {
        return Ok(false);
    }

    let current_branch = git_output(&worktree_dir, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if current_branch != branch_state.branch {
        return Ok(false);
    }

    let base_sha = match branch_state.base_sha {
        Some(base_sha) => base_sha,
        None => git_output(
            project_root,
            &["rev-parse", &format!("origin/{}", branch_state.base_branch)],
        )?,
    };
    let head_sha = git_output(&worktree_dir, &["rev-parse", "HEAD"])?;
    Ok(head_sha != base_sha)
}

fn prompt_step_has_worktree_changes(project_root: &Path, agent: &str) -> Result<bool> {
    let worktree_dir = project_root.join(".tutti").join("worktrees").join(agent);
    if !worktree_dir.exists() {
        return Ok(false);
    }
    let status = git_output(&worktree_dir, &["status", "--porcelain"])?;
    Ok(!status.trim().is_empty())
}

fn finalize_implementer_worktree_changes(project_root: &Path, agent: &str) -> Result<bool> {
    let branch_file = project_root
        .join(".tutti")
        .join("state")
        .join("auto")
        .join("branch.json");
    let selected_issue_file = project_root
        .join(".tutti")
        .join("state")
        .join("auto")
        .join("selected_issue.json");

    if !branch_file.exists() || !selected_issue_file.exists() {
        return Ok(false);
    }

    let branch_state: WorkflowBranchState =
        serde_json::from_slice(&std::fs::read(&branch_file)?)
            .map_err(|e| TuttiError::ConfigValidation(e.to_string()))?;
    let selected_issue: SelectedIssueState =
        serde_json::from_slice(&std::fs::read(&selected_issue_file)?)
            .map_err(|e| TuttiError::ConfigValidation(e.to_string()))?;

    let worktree_dir = project_root.join(".tutti").join("worktrees").join(agent);
    if !worktree_dir.exists() {
        return Ok(false);
    }

    let current_branch = git_output(&worktree_dir, &["rev-parse", "--abbrev-ref", "HEAD"])?;
    if current_branch != branch_state.branch {
        return Ok(false);
    }

    if !prompt_step_has_worktree_changes(project_root, agent)? {
        return Ok(false);
    }

    let commit_message = format!(
        "feat: address issue #{} {}",
        selected_issue.issue_number, selected_issue.title
    );

    let add = Command::new("git")
        .args(["add", "-A"])
        .current_dir(&worktree_dir)
        .output()?;
    if !add.status.success() {
        return Err(TuttiError::ConfigValidation(format!(
            "git add failed while finalizing implementer diff: {}",
            String::from_utf8_lossy(&add.stderr).trim()
        )));
    }

    let commit = Command::new("git")
        .args([
            "-c",
            "user.name=Tutti Automation",
            "-c",
            "user.email=tutti-automation@users.noreply.github.com",
            "commit",
            "-m",
            &commit_message,
        ])
        .current_dir(&worktree_dir)
        .output()?;
    if !commit.status.success() {
        let stderr = String::from_utf8_lossy(&commit.stderr);
        if stderr.contains("nothing to commit") {
            return Ok(false);
        }
        return Err(TuttiError::ConfigValidation(format!(
            "git commit failed while finalizing implementer diff: {}",
            stderr.trim()
        )));
    }

    let push = Command::new("git")
        .args(["push", "origin", &format!("HEAD:{}", branch_state.branch)])
        .current_dir(&worktree_dir)
        .output()?;
    if !push.status.success() {
        return Err(TuttiError::ConfigValidation(format!(
            "git push failed while finalizing implementer diff: {}",
            String::from_utf8_lossy(&push.stderr).trim()
        )));
    }

    Ok(true)
}

/// Start an agent session and wait for it to reach idle/ready state.
fn start_and_wait_ready(
    project_root: &Path,
    config: &TuttiConfig,
    agent: &str,
    session_name: &str,
) -> Result<()> {
    with_project_root(project_root, || {
        crate::cli::up::run(Some(agent), None, false, false, None, None)
    })?;
    let agent_runtime = config
        .agents
        .iter()
        .find(|a| a.name == agent)
        .map(|a| a.runtime.as_deref().unwrap_or("claude-code"))
        .unwrap_or("claude-code");
    let wait_result = health::wait_for_agent_idle(
        agent_runtime,
        session_name,
        Duration::from_secs(30),
        Duration::from_secs(3),
        Duration::from_secs(5),
    )?;
    if !wait_result.is_completed() {
        eprintln!(
            "  warn: agent '{}' did not reach ready state within 30s, proceeding anyway",
            agent
        );
    }
    Ok(())
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git").args(args).current_dir(cwd).output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(TuttiError::ConfigValidation(format!(
            "git {} failed: {}",
            args.join(" "),
            stderr.trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[derive(Debug)]
struct CommandRunOutcome {
    outcome: CommandOutcome,
    attempts: u32,
}

fn run_shell_command_with_retry(
    command: &str,
    cwd: &Path,
    timeout_secs: u64,
    retry_policy: Option<&RetryPolicy>,
) -> Result<CommandRunOutcome> {
    let max_attempts = retry_policy.map_or(1, |p| p.max_attempts.max(1));
    let mut attempt = 1;
    loop {
        let outcome = run_shell_command(command, cwd, timeout_secs)?;
        let failed = outcome.timed_out || outcome.exit_code.unwrap_or(1) != 0;
        if !failed || attempt >= max_attempts {
            return Ok(CommandRunOutcome {
                outcome,
                attempts: attempt,
            });
        }

        if let Some(policy) = retry_policy {
            let backoff_ms = retry_backoff_ms(*policy, attempt);
            std::thread::sleep(Duration::from_millis(backoff_ms));
        }
        attempt += 1;
    }
}

fn retry_backoff_ms(policy: RetryPolicy, attempt: u32) -> u64 {
    let mut backoff = policy.initial_backoff_ms.max(1);
    let cap = policy.max_backoff_ms.max(backoff);
    for _ in 1..attempt {
        backoff = backoff.saturating_mul(2).min(cap);
    }
    backoff
}

struct WorkflowCommandPolicyContext<'a> {
    project_root: &'a Path,
    workspace: &'a str,
    workflow_name: &'a str,
    run_id: &'a str,
    step_index: usize,
    agent: Option<&'a str>,
    policy: Option<&'a PermissionsConfig>,
}

fn evaluate_workflow_command_policy(
    ctx: WorkflowCommandPolicyContext<'_>,
    command: &str,
) -> crate::permissions::CommandPolicyDecision {
    let decision = evaluate_command_policy(ctx.policy, command);

    let _ = append_policy_decision(
        ctx.project_root,
        &crate::state::PolicyDecisionRecord {
            timestamp: Utc::now(),
            workspace: ctx.workspace.to_string(),
            agent: ctx.agent.map(ToString::to_string),
            runtime: None,
            action: "workflow_command".to_string(),
            mode: "workflow".to_string(),
            policy: if decision.policy_configured {
                "configured".to_string()
            } else {
                "unset".to_string()
            },
            enforcement: if decision.policy_configured {
                "hard".to_string()
            } else {
                "none".to_string()
            },
            decision: if decision.allowed {
                "allow".to_string()
            } else {
                "block".to_string()
            },
            reason: decision.reason.clone(),
            data: Some(json!({
                "workflow": ctx.workflow_name,
                "run_id": ctx.run_id,
                "step_index": ctx.step_index,
                "command": decision.command,
                "matched_rule": decision.matched_rule,
                "suggested_rule": decision.suggested_rule,
            })),
        },
    );

    decision
}

fn load_and_store_output(
    project_root: &Path,
    run_id: &str,
    step_id: &str,
    output_path: &Path,
) -> Result<StepOutputValue> {
    let body = std::fs::read_to_string(output_path).map_err(|e| {
        TuttiError::ConfigValidation(format!(
            "failed reading output_json for step '{}': {} ({e})",
            step_id,
            output_path.display()
        ))
    })?;
    let parsed: Value = serde_json::from_str(&body).map_err(|e| {
        TuttiError::ConfigValidation(format!(
            "failed parsing output_json for step '{}': {} ({e})",
            step_id,
            output_path.display()
        ))
    })?;

    let canonical_path = save_workflow_output(project_root, run_id, step_id, &parsed)?;
    Ok(StepOutputValue {
        path: canonical_path,
        json: parsed,
    })
}

fn load_resume_outputs(
    output_files: &HashMap<String, String>,
) -> Result<HashMap<String, StepOutputValue>> {
    let mut outputs = HashMap::new();
    for (step_id, path_str) in output_files {
        let path = PathBuf::from(path_str);
        let body = std::fs::read_to_string(&path).map_err(|e| {
            TuttiError::ConfigValidation(format!(
                "failed reading resumed output_json for step '{}': {} ({e})",
                step_id,
                path.display()
            ))
        })?;
        let parsed: Value = serde_json::from_str(&body).map_err(|e| {
            TuttiError::ConfigValidation(format!(
                "failed parsing resumed output_json for step '{}': {} ({e})",
                step_id,
                path.display()
            ))
        })?;
        outputs.insert(step_id.clone(), StepOutputValue { path, json: parsed });
    }
    Ok(outputs)
}

fn step_intent_payload(step: &ResolvedStep) -> Value {
    match step {
        ResolvedStep::Prompt {
            agent,
            text,
            session_name,
            artifact_glob,
            artifact_name,
            ..
        } => json!({
            "agent": agent,
            "session_name": session_name,
            "text_chars": text.chars().count(),
            "artifact_glob": artifact_glob,
            "artifact_name": artifact_name,
        }),
        ResolvedStep::Command {
            run,
            cwd,
            agent,
            timeout_secs,
            ..
        } => json!({
            "run": run,
            "cwd": cwd.display().to_string(),
            "agent": agent,
            "timeout_secs": timeout_secs,
        }),
        ResolvedStep::EnsureRunning {
            agent,
            session_name,
            ..
        } => json!({
            "agent": agent,
            "session_name": session_name,
        }),
        ResolvedStep::Workflow {
            workflow,
            agent_override,
            strict,
            ..
        } => json!({
            "workflow": workflow,
            "agent_override": agent_override,
            "strict": strict,
        }),
        ResolvedStep::Land {
            agent, pr, force, ..
        } => json!({
            "agent": agent,
            "pr": pr,
            "force": force,
        }),
        ResolvedStep::Review {
            agent, reviewer, ..
        } => json!({
            "agent": agent,
            "reviewer": reviewer,
        }),
    }
}

fn record_step_intent(
    project_root: &Path,
    run_id: &str,
    workflow_name: &str,
    step_index: usize,
    step: &ResolvedStep,
) -> Result<()> {
    let step_id = step_file_key(step, step_index);
    let existing = load_workflow_intent(project_root, run_id, &step_id)?;
    let attempt = existing
        .as_ref()
        .map(|record| record.attempt.saturating_add(1))
        .unwrap_or(1);
    let planned_at = existing
        .as_ref()
        .map(|record| record.planned_at)
        .unwrap_or_else(Utc::now);
    let record = WorkflowStepIntentRecord {
        run_id: run_id.to_string(),
        workflow_name: workflow_name.to_string(),
        step_index,
        step_id: step_id.clone(),
        step_type: step_type_name(step).to_string(),
        planned_at,
        intent: step_intent_payload(step),
        attempt,
        outcome: None,
    };
    save_workflow_intent(project_root, run_id, &step_id, &record)?;
    Ok(())
}

fn step_side_effects(step: &ResolvedStep, result: &StepResult) -> Option<Value> {
    match step {
        ResolvedStep::Land {
            agent, pr, force, ..
        } => Some(json!({
            "agent": agent,
            "pr": pr,
            "force": force,
            "status": format!("{:?}", result.status).to_lowercase(),
        })),
        ResolvedStep::Review {
            agent, reviewer, ..
        } => Some(json!({
            "agent": agent,
            "reviewer": reviewer,
            "status": format!("{:?}", result.status).to_lowercase(),
        })),
        ResolvedStep::EnsureRunning { agent, .. } => Some(json!({
            "agent": agent,
            "status": format!("{:?}", result.status).to_lowercase(),
        })),
        _ => None,
    }
}

fn record_step_outcome(
    project_root: &Path,
    run_id: &str,
    workflow_name: &str,
    step_index: usize,
    step: &ResolvedStep,
    result: &StepResult,
) -> Result<()> {
    let step_id = step_file_key(step, step_index);
    let mut record =
        load_workflow_intent(project_root, run_id, &step_id)?.unwrap_or(WorkflowStepIntentRecord {
            run_id: run_id.to_string(),
            workflow_name: workflow_name.to_string(),
            step_index,
            step_id: step_id.clone(),
            step_type: step_type_name(step).to_string(),
            planned_at: Utc::now(),
            intent: step_intent_payload(step),
            attempt: 1,
            outcome: None,
        });
    record.workflow_name = workflow_name.to_string();
    record.step_index = step_index;
    record.step_type = step_type_name(step).to_string();
    record.outcome = Some(WorkflowStepOutcomeRecord {
        completed_at: Utc::now(),
        status: format!("{:?}", result.status).to_lowercase(),
        success: result.status == StepStatus::Success,
        exit_code: result.exit_code,
        timed_out: result.timed_out,
        message: result.message.clone(),
        side_effects: step_side_effects(step, result),
    });
    save_workflow_intent(project_root, run_id, &step_id, &record)?;
    Ok(())
}

fn load_unsuccessful_step_intent(
    project_root: &Path,
    run_id: &str,
    step: &ResolvedStep,
    step_index: usize,
) -> Result<Option<WorkflowStepIntentRecord>> {
    let step_id = step_file_key(step, step_index);
    let Some(record) = load_workflow_intent(project_root, run_id, &step_id)? else {
        return Ok(None);
    };
    if record.outcome.as_ref().is_some_and(|o| o.success) {
        return Ok(None);
    }
    Ok(Some(record))
}

fn review_packet_exists_since(
    project_root: &Path,
    agent: &str,
    since: chrono::DateTime<Utc>,
) -> Result<Option<PathBuf>> {
    let reviews_dir = project_root.join(".tutti").join("state").join("reviews");
    if !reviews_dir.exists() {
        return Ok(None);
    }

    let prefix = format!("{agent}-review-");
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in std::fs::read_dir(&reviews_dir)? {
        let entry = entry?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|f| f.to_str()) else {
            continue;
        };
        if !name.starts_with(&prefix) || !name.ends_with(".md") {
            continue;
        }
        let Ok(meta) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let modified_utc: chrono::DateTime<Utc> = modified.into();
        if modified_utc < since {
            continue;
        }
        if best
            .as_ref()
            .is_none_or(|(best_modified, _)| modified > *best_modified)
        {
            best = Some((modified, path));
        }
    }
    Ok(best.map(|(_, path)| path))
}

fn agent_branch(config: &TuttiConfig, agent: &str) -> Option<String> {
    config
        .agents
        .iter()
        .find(|a| a.name == agent)
        .map(|a| a.resolved_branch())
}

fn is_branch_merged(project_root: &Path, branch: &str) -> Result<bool> {
    let status = Command::new("git")
        .args(["merge-base", "--is-ancestor", branch, "HEAD"])
        .current_dir(project_root)
        .status()?;
    Ok(status.success())
}

fn worktree_has_changes(project_root: &Path, agent: &str) -> Result<bool> {
    let snapshot = crate::worktree::inspect_worktree(project_root, agent)?;
    Ok(snapshot.exists && snapshot.dirty)
}

fn should_skip_land_replay(
    config: &TuttiConfig,
    project_root: &Path,
    agent: &str,
    intent: &WorkflowStepIntentRecord,
) -> Option<String> {
    let branch = agent_branch(config, agent).unwrap_or_else(|| format!("tutti/{agent}"));
    let merged = is_branch_merged(project_root, &branch).unwrap_or(false);
    let dirty_worktree = worktree_has_changes(project_root, agent).unwrap_or(true);
    if merged && !dirty_worktree {
        Some(format!(
            "resume guard: '{}' already merged (branch '{}') with no pending worktree changes after prior attempt at {}; skipping replay",
            agent,
            branch,
            intent.planned_at.to_rfc3339()
        ))
    } else {
        None
    }
}

fn land_replay_diagnostics(
    config: &TuttiConfig,
    project_root: &Path,
    agent: &str,
) -> (String, bool, bool) {
    let branch = agent_branch(config, agent).unwrap_or_else(|| format!("tutti/{agent}"));
    let merged = is_branch_merged(project_root, &branch).unwrap_or(false);
    let dirty_worktree = worktree_has_changes(project_root, agent).unwrap_or(true);
    (branch, merged, dirty_worktree)
}

pub fn build_resume_compensator_plan(
    config: &TuttiConfig,
    project_root: &Path,
    workflow: &ResolvedWorkflow,
    resume: &ResumeContext,
) -> Result<Vec<String>> {
    let Some(step_index) = resume
        .failed_steps
        .iter()
        .copied()
        .find(|idx| !resume.completed_steps.contains(idx))
    else {
        return Ok(Vec::new());
    };
    let Some(step) = workflow.steps.get(step_index.saturating_sub(1)) else {
        return Ok(Vec::new());
    };
    let Some(intent) =
        load_unsuccessful_step_intent(project_root, &resume.run_id, step, step_index)?
    else {
        return Ok(Vec::new());
    };

    let mut plan = vec![format!(
        "Compensator preflight: step {step_index} ({}) has prior intent without a successful outcome.",
        step_type_name(step)
    )];

    match step {
        ResolvedStep::Land { agent, .. } => {
            let (branch, merged, dirty_worktree) =
                land_replay_diagnostics(config, project_root, agent);
            if merged && !dirty_worktree {
                plan.push(format!(
                    "Detected branch '{branch}' already merged into HEAD with no pending worktree changes; replay will be skipped by idempotency guard."
                ));
            } else {
                plan.push(format!(
                    "Branch '{branch}' is not safely idempotent yet (merged={merged}, worktree_dirty={dirty_worktree}); replay will re-attempt `land`."
                ));
            }
        }
        ResolvedStep::Review { agent, .. } => {
            if let Some(packet) =
                review_packet_exists_since(project_root, agent, intent.planned_at)?
            {
                plan.push(format!(
                    "Detected existing review packet after prior attempt: {}. Replay will be skipped by idempotency guard.",
                    packet.display()
                ));
            } else {
                plan.push(
                    "No new review packet found after prior attempt; replay will re-send review."
                        .to_string(),
                );
            }
        }
        ResolvedStep::EnsureRunning {
            agent,
            session_name,
            ..
        } => {
            if TmuxSession::session_exists(session_name) {
                plan.push(format!(
                    "Agent '{agent}' is already running; replay remains a no-op."
                ));
            } else {
                plan.push(format!(
                    "Agent '{agent}' is not running; replay will attempt to start it."
                ));
            }
        }
        ResolvedStep::Command { .. } => {
            plan.push(
                "Command steps are best-effort only; verify repository side effects before replay."
                    .to_string(),
            );
        }
        _ => {
            plan.push(
                "Replay will continue; review prior step output if side effects are possible."
                    .to_string(),
            );
        }
    }

    Ok(plan)
}

fn save_execution_checkpoint(
    project_root: &Path,
    options: &ExecuteOptions,
    agent_scope: Option<&str>,
    result: &ExecutionResult,
) -> Result<()> {
    let completed_steps: HashSet<usize> = result
        .step_results
        .iter()
        .filter(|s| s.status != StepStatus::Failed)
        .map(|s| s.index)
        .collect();
    let mut completed_steps: Vec<usize> = completed_steps.into_iter().collect();
    completed_steps.sort_unstable();

    let record = WorkflowCheckpointRecord {
        run_id: result.run_id.clone(),
        workflow_name: result.workflow_name.clone(),
        strict: result.strict,
        origin: options.origin.as_str().to_string(),
        agent_scope: agent_scope.map(|s| s.to_string()),
        started_at: result.started_at,
        finished_at: result.finished_at,
        success: result.success,
        failed_steps: result.failed_steps.clone(),
        step_results: result.step_results.clone(),
        output_files: result.output_files.clone(),
    };

    let mut value = serde_json::to_value(record)?;
    if let Some(obj) = value.as_object_mut() {
        obj.insert(
            "completed_steps".to_string(),
            serde_json::to_value(completed_steps)?,
        );
    }
    save_workflow_checkpoint(project_root, &result.run_id, &value)?;
    Ok(())
}

fn render_template(
    input: &str,
    outputs: &HashMap<String, StepOutputValue>,
    shell_escape_values: bool,
) -> Result<String> {
    let mut rendered = input.to_string();
    for (step_id, value) in outputs {
        let path_token = format!("{{{{output.{step_id}.path}}}}");
        let json_token = format!("{{{{output.{step_id}.json}}}}");
        let path_value = value.path.display().to_string();
        let json_value = value.json.to_string();
        if shell_escape_values {
            rendered = rendered.replace(&path_token, &shell_escape(&path_value));
            rendered = rendered.replace(&json_token, &shell_escape(&json_value));
        } else {
            rendered = rendered.replace(&path_token, &path_value);
            rendered = rendered.replace(&json_token, &json_value);
        }
    }

    if rendered.contains("{{output.") {
        return Err(TuttiError::ConfigValidation(format!(
            "unresolved workflow output template in step text: {}",
            rendered
        )));
    }

    Ok(rendered)
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn with_project_root<T, F>(project_root: &Path, operation: F) -> Result<T>
where
    F: FnOnce() -> Result<T>,
{
    let original = std::env::current_dir()?;
    std::env::set_current_dir(project_root)?;
    let result = operation();
    let restore_result = std::env::set_current_dir(&original);
    match (result, restore_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(e), Ok(())) => Err(e),
        (Ok(_), Err(e)) => Err(TuttiError::Io(e)),
        (Err(e), Err(_)) => Err(e),
    }
}

fn build_normalized_dependencies(steps: &[ResolvedStep]) -> Vec<Vec<usize>> {
    let mut out = Vec::with_capacity(steps.len());
    for (idx, step) in steps.iter().enumerate() {
        let deps = step_depends_on(step);
        if deps.is_empty() && idx > 0 {
            out.push(vec![idx]);
        } else {
            out.push(deps.to_vec());
        }
    }
    out
}

fn execute_control_dag(
    config: &TuttiConfig,
    project_root: &Path,
    run_id: &str,
    workflow_name: &str,
    steps: &[ResolvedStep],
    dependencies: &[Vec<usize>],
    completed_steps: &HashSet<usize>,
) -> Result<ControlDagOutcome> {
    let mut success = true;
    let mut failed_steps = Vec::new();
    let mut step_results = Vec::new();
    let mut completed = completed_steps.clone();
    let mut pending: HashSet<usize> = (1..=steps.len())
        .filter(|idx| !completed_steps.contains(idx))
        .collect();

    while !pending.is_empty() {
        let mut ready: Vec<usize> = pending
            .iter()
            .copied()
            .filter(|idx| {
                dependencies[*idx - 1]
                    .iter()
                    .all(|dep| completed.contains(dep))
            })
            .collect();
        ready.sort_unstable();

        if ready.is_empty() {
            return Err(TuttiError::ConfigValidation(
                "workflow depends_on graph is blocked (unmet dependencies)".to_string(),
            ));
        }

        // Emit step.started for each step in this DAG wave
        for idx in &ready {
            let step = &steps[*idx - 1];
            let _ = append_control_event(
                project_root,
                &ControlEvent {
                    event: "workflow.step.started".to_string(),
                    workspace: config.workspace.name.clone(),
                    agent: step_agent_name(step).map(|s| s.to_string()),
                    timestamp: Utc::now(),
                    correlation_id: run_id.to_string(),
                    data: Some(json!({
                        "workflow_name": workflow_name,
                        "step_index": idx,
                        "step_type": step_type_name(step),
                        "total_steps": steps.len()
                    })),
                },
            );
        }

        let wave_outcomes: Vec<ControlStepOutcome> = if ready.len() == 1 {
            vec![execute_control_step(
                config,
                project_root,
                run_id,
                workflow_name,
                &steps[ready[0] - 1],
                ready[0],
            )?]
        } else {
            let mut handles = Vec::with_capacity(ready.len());
            for idx in &ready {
                let cfg = config.clone();
                let root = project_root.to_path_buf();
                let run_id = run_id.to_string();
                let workflow_name = workflow_name.to_string();
                let step = steps[*idx - 1].clone();
                let step_idx = *idx;
                handles.push(std::thread::spawn(move || {
                    execute_control_step(&cfg, &root, &run_id, &workflow_name, &step, step_idx)
                }));
            }
            let mut out = Vec::with_capacity(ready.len());
            for handle in handles {
                let joined = handle
                    .join()
                    .map_err(|_| TuttiError::State("control step thread panicked".to_string()))?;
                out.push(joined?);
            }
            out.sort_by_key(|o| o.index);
            out
        };

        let mut hard_fail = false;
        for outcome in wave_outcomes {
            pending.remove(&outcome.index);
            if outcome.result.status == StepStatus::Failed {
                success = false;
                failed_steps.push(outcome.index);
                if outcome.hard_fail {
                    hard_fail = true;
                }
            } else {
                completed.insert(outcome.index);
            }
            step_results.push(outcome.result);
        }

        if hard_fail {
            break;
        }
    }

    Ok(ControlDagOutcome {
        success,
        failed_steps,
        step_results,
    })
}

fn execute_control_step(
    config: &TuttiConfig,
    project_root: &Path,
    run_id: &str,
    workflow_name: &str,
    step: &ResolvedStep,
    step_index: usize,
) -> Result<ControlStepOutcome> {
    let started = std::time::Instant::now();
    let prior_unsuccessful_intent =
        load_unsuccessful_step_intent(project_root, run_id, step, step_index)?;
    if let Err(err) = record_step_intent(project_root, run_id, workflow_name, step_index, step) {
        return Ok(ControlStepOutcome {
            index: step_index,
            result: StepResult {
                index: step_index,
                step_type: step_type_name(step).to_string(),
                status: StepStatus::Failed,
                duration_ms: started.elapsed().as_millis() as u64,
                exit_code: None,
                timed_out: false,
                message: Some(format!("failed to write step intent: {err}")),
                stdout: None,
                stderr: None,
            },
            hard_fail: true,
        });
    }
    match step {
        ResolvedStep::EnsureRunning {
            agent,
            session_name,
            fail_mode,
            ..
        } => {
            if TmuxSession::session_exists(session_name) {
                return Ok(ControlStepOutcome {
                    index: step_index,
                    result: StepResult {
                        index: step_index,
                        step_type: "ensure_running".to_string(),
                        status: StepStatus::Success,
                        duration_ms: started.elapsed().as_millis() as u64,
                        exit_code: Some(0),
                        timed_out: false,
                        message: Some(format!("agent '{}' already running", agent)),
                        stdout: None,
                        stderr: None,
                    },
                    hard_fail: false,
                });
            }

            let start_result = run_tt_subcommand(project_root, &["up".to_string(), agent.clone()]);
            match (start_result, *fail_mode) {
                (Ok(_), _) => Ok(ControlStepOutcome {
                    index: step_index,
                    result: StepResult {
                        index: step_index,
                        step_type: "ensure_running".to_string(),
                        status: StepStatus::Success,
                        duration_ms: started.elapsed().as_millis() as u64,
                        exit_code: Some(0),
                        timed_out: false,
                        message: Some(format!("started agent '{}'", agent)),
                        stdout: None,
                        stderr: None,
                    },
                    hard_fail: false,
                }),
                (Err(e), WorkflowFailMode::Open) => Ok(ControlStepOutcome {
                    index: step_index,
                    result: StepResult {
                        index: step_index,
                        step_type: "ensure_running".to_string(),
                        status: StepStatus::Warning,
                        duration_ms: started.elapsed().as_millis() as u64,
                        exit_code: None,
                        timed_out: false,
                        message: Some(format!("failed to start '{}': {e}", agent)),
                        stdout: None,
                        stderr: None,
                    },
                    hard_fail: false,
                }),
                (Err(e), WorkflowFailMode::Closed) => Ok(ControlStepOutcome {
                    index: step_index,
                    result: StepResult {
                        index: step_index,
                        step_type: "ensure_running".to_string(),
                        status: StepStatus::Failed,
                        duration_ms: started.elapsed().as_millis() as u64,
                        exit_code: None,
                        timed_out: false,
                        message: Some(format!("failed to start '{}': {e}", agent)),
                        stdout: None,
                        stderr: None,
                    },
                    hard_fail: true,
                }),
            }
        }
        ResolvedStep::Review {
            agent,
            reviewer,
            fail_mode,
            ..
        } => {
            if let Some(intent) = prior_unsuccessful_intent.as_ref()
                && let Some(packet) =
                    review_packet_exists_since(project_root, agent, intent.planned_at)?
            {
                return Ok(ControlStepOutcome {
                    index: step_index,
                    result: StepResult {
                        index: step_index,
                        step_type: "review".to_string(),
                        status: StepStatus::Success,
                        duration_ms: started.elapsed().as_millis() as u64,
                        exit_code: Some(0),
                        timed_out: false,
                        message: Some(format!(
                            "resume guard: existing review packet detected at {}; skipping replay",
                            packet.display()
                        )),
                        stdout: None,
                        stderr: None,
                    },
                    hard_fail: false,
                });
            }
            let reviewer_session = TmuxSession::session_name(&config.workspace.name, reviewer);
            if !TmuxSession::session_exists(&reviewer_session)
                && let Err(e) =
                    run_tt_subcommand(project_root, &["up".to_string(), reviewer.clone()])
            {
                return Ok(control_error_outcome(
                    step_index,
                    "review",
                    *fail_mode,
                    started,
                    format!("failed to auto-start reviewer '{}': {e}", reviewer),
                ));
            }

            let args = vec![
                "review".to_string(),
                agent.clone(),
                "--reviewer".to_string(),
                reviewer.clone(),
            ];
            let run_result = run_tt_subcommand(project_root, &args);
            match (run_result, *fail_mode) {
                (Ok(_), _) => Ok(ControlStepOutcome {
                    index: step_index,
                    result: StepResult {
                        index: step_index,
                        step_type: "review".to_string(),
                        status: StepStatus::Success,
                        duration_ms: started.elapsed().as_millis() as u64,
                        exit_code: Some(0),
                        timed_out: false,
                        message: Some(format!("sent '{}' for review", agent)),
                        stdout: None,
                        stderr: None,
                    },
                    hard_fail: false,
                }),
                (Err(e), mode) => Ok(control_error_outcome(
                    step_index,
                    "review",
                    mode,
                    started,
                    e.to_string(),
                )),
            }
        }
        ResolvedStep::Land {
            agent,
            pr,
            force,
            fail_mode,
            ..
        } => {
            if let Some(intent) = prior_unsuccessful_intent.as_ref()
                && let Some(message) = should_skip_land_replay(config, project_root, agent, intent)
            {
                return Ok(ControlStepOutcome {
                    index: step_index,
                    result: StepResult {
                        index: step_index,
                        step_type: "land".to_string(),
                        status: StepStatus::Success,
                        duration_ms: started.elapsed().as_millis() as u64,
                        exit_code: Some(0),
                        timed_out: false,
                        message: Some(message),
                        stdout: None,
                        stderr: None,
                    },
                    hard_fail: false,
                });
            }
            let agent_session = TmuxSession::session_name(&config.workspace.name, agent);
            if !TmuxSession::session_exists(&agent_session)
                && let Err(e) = run_tt_subcommand(project_root, &["up".to_string(), agent.clone()])
            {
                return Ok(control_error_outcome(
                    step_index,
                    "land",
                    *fail_mode,
                    started,
                    format!("failed to auto-start '{}' before land: {e}", agent),
                ));
            }

            let mut args = vec!["land".to_string(), agent.clone()];
            if *pr {
                args.push("--pr".to_string());
            }
            if *force {
                args.push("--force".to_string());
            }
            let run_result = run_tt_subcommand_with_env(
                project_root,
                &args,
                &[(crate::cli::land::ENFORCE_MERGE_GATE_ENV, "1")],
            );
            match (run_result, *fail_mode) {
                (Ok(_), _) => Ok(ControlStepOutcome {
                    index: step_index,
                    result: StepResult {
                        index: step_index,
                        step_type: "land".to_string(),
                        status: StepStatus::Success,
                        duration_ms: started.elapsed().as_millis() as u64,
                        exit_code: Some(0),
                        timed_out: false,
                        message: Some(format!("landed '{}'", agent)),
                        stdout: None,
                        stderr: None,
                    },
                    hard_fail: false,
                }),
                (Err(e), mode) => Ok(control_error_outcome(
                    step_index,
                    "land",
                    mode,
                    started,
                    e.to_string(),
                )),
            }
        }
        _ => Err(TuttiError::ConfigValidation(
            "internal: non-control step passed to control DAG".to_string(),
        )),
    }
}

fn control_error_outcome(
    index: usize,
    step_type: &str,
    mode: WorkflowFailMode,
    started: std::time::Instant,
    message: String,
) -> ControlStepOutcome {
    let (status, hard_fail, exit_code) = match mode {
        WorkflowFailMode::Open => (StepStatus::Warning, false, Some(1)),
        WorkflowFailMode::Closed => (StepStatus::Failed, true, Some(1)),
    };
    ControlStepOutcome {
        index,
        result: StepResult {
            index,
            step_type: step_type.to_string(),
            status,
            duration_ms: started.elapsed().as_millis() as u64,
            exit_code,
            timed_out: false,
            message: Some(message),
            stdout: None,
            stderr: None,
        },
        hard_fail,
    }
}

fn run_tt_subcommand(project_root: &Path, args: &[String]) -> Result<()> {
    run_tt_subcommand_with_env(project_root, args, &[])
}

fn run_tt_subcommand_with_env(
    project_root: &Path,
    args: &[String],
    env: &[(&str, &str)],
) -> Result<()> {
    let bin = std::env::current_exe()?;
    let mut cmd = Command::new(bin);
    cmd.args(args).current_dir(project_root);
    for (key, value) in env {
        cmd.env(key, value);
    }
    let output = cmd.output()?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if !stderr.is_empty() { stderr } else { stdout };
    Err(TuttiError::State(format!(
        "tt {} failed: {}",
        args.join(" "),
        detail
    )))
}

pub struct HookDispatcher;

impl HookDispatcher {
    pub fn dispatch_agent_stop(
        config: &TuttiConfig,
        payload: &HookEventPayload,
    ) -> Result<Vec<ExecutionResult>> {
        let mut results = Vec::new();

        for hook in config
            .hooks
            .iter()
            .filter(|h| h.event == HookEvent::AgentStop)
        {
            if !hook_matches_agent(hook, &payload.agent_name) {
                continue;
            }

            let run_result = dispatch_agent_stop_hook(config, hook, payload);
            match run_result {
                Ok(result) => results.push(result),
                Err(e) => {
                    if hook.fail_mode.unwrap_or(WorkflowFailMode::Open) == WorkflowFailMode::Closed
                    {
                        return Err(e);
                    }
                }
            }
        }

        Ok(results)
    }

    pub fn dispatch_workflow_complete(
        config: &TuttiConfig,
        payload: &WorkflowCompletePayload,
    ) -> Result<Vec<ExecutionResult>> {
        let mut results = Vec::new();
        for hook in config
            .hooks
            .iter()
            .filter(|h| h.event == HookEvent::WorkflowComplete)
        {
            if !hook_matches_workflow_complete(hook, payload) {
                continue;
            }

            let run_result = dispatch_workflow_complete_hook(config, hook, payload);
            match run_result {
                Ok(result) => results.push(result),
                Err(e) => {
                    if hook.fail_mode.unwrap_or(WorkflowFailMode::Open) == WorkflowFailMode::Closed
                    {
                        return Err(e);
                    }
                }
            }
        }
        Ok(results)
    }
}

fn hook_matches_agent(hook: &HookConfig, agent_name: &str) -> bool {
    hook.agent.as_deref().is_none_or(|a| a == agent_name)
}

fn dispatch_agent_stop_hook(
    config: &TuttiConfig,
    hook: &HookConfig,
    payload: &HookEventPayload,
) -> Result<ExecutionResult> {
    let resolver = WorkflowResolver::new(config, &payload.project_root);
    let options = ExecuteOptions {
        strict: false,
        force_open_commands: false,
        command_policy: load_command_policy(),
        retry_policy: load_retry_policy(),
        origin: ExecutionOrigin::HookAgentStop,
        hook_event: Some("agent_stop".to_string()),
        hook_agent: Some(payload.agent_name.clone()),
    };

    let mut result = if let Some(workflow_name) = hook.workflow.as_deref() {
        let resolved = resolver.resolve(workflow_name, Some(&payload.agent_name), &options)?;
        execute_workflow_with_hooks(
            config,
            &payload.project_root,
            &resolved,
            &options,
            Some(&payload.agent_name),
            None,
        )?
    } else if let Some(cmd) = hook.run.as_deref() {
        let resolved = ResolvedWorkflow {
            name: format!("hook:agent_stop:{}", payload.agent_name),
            description: Some("Generated hook command".to_string()),
            steps: vec![ResolvedStep::Command {
                step_id: None,
                depends_on: vec![],
                run: cmd.to_string(),
                cwd: payload.project_root.clone(),
                agent: Some(payload.agent_name.clone()),
                timeout_secs: hook.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
                fail_mode: hook.fail_mode.unwrap_or(WorkflowFailMode::Open),
                output_json: None,
            }],
        };
        execute_workflow_with_hooks(
            config,
            &payload.project_root,
            &resolved,
            &options,
            Some(&payload.agent_name),
            None,
        )?
    } else {
        return Err(TuttiError::ConfigValidation(
            "hook must specify workflow or run".to_string(),
        ));
    };

    if !result.success
        && hook.fail_mode.unwrap_or(WorkflowFailMode::Open) == WorkflowFailMode::Closed
    {
        return Err(TuttiError::ConfigValidation(format!(
            "hook failed for agent '{}': {}",
            payload.agent_name, result.workflow_name
        )));
    }

    if !result.success {
        for step in &mut result.step_results {
            if step.status == StepStatus::Failed {
                step.status = StepStatus::Warning;
            }
        }
        result.success = true;
        result.failed_steps.clear();
    }

    Ok(result)
}

fn dispatch_workflow_complete_hook(
    config: &TuttiConfig,
    hook: &HookConfig,
    payload: &WorkflowCompletePayload,
) -> Result<ExecutionResult> {
    let resolver = WorkflowResolver::new(config, &payload.project_root);
    let options = ExecuteOptions {
        strict: false,
        force_open_commands: false,
        command_policy: load_command_policy(),
        retry_policy: load_retry_policy(),
        origin: ExecutionOrigin::HookWorkflowComplete,
        hook_event: Some("workflow_complete".to_string()),
        hook_agent: payload.agent_scope.clone(),
    };

    let mut result = if let Some(workflow_name) = hook.workflow.as_deref() {
        let resolved = resolver.resolve(workflow_name, payload.agent_scope.as_deref(), &options)?;
        execute_workflow_with_hooks(
            config,
            &payload.project_root,
            &resolved,
            &options,
            payload.agent_scope.as_deref(),
            None,
        )?
    } else if let Some(cmd) = hook.run.as_deref() {
        let resolved = ResolvedWorkflow {
            name: format!("hook:workflow_complete:{}", payload.workflow_name),
            description: Some("Generated hook command".to_string()),
            steps: vec![ResolvedStep::Command {
                step_id: None,
                depends_on: vec![],
                run: cmd.to_string(),
                cwd: payload.project_root.clone(),
                agent: payload.agent_scope.clone(),
                timeout_secs: hook.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
                fail_mode: hook.fail_mode.unwrap_or(WorkflowFailMode::Open),
                output_json: None,
            }],
        };
        execute_workflow_with_hooks(
            config,
            &payload.project_root,
            &resolved,
            &options,
            payload.agent_scope.as_deref(),
            None,
        )?
    } else {
        return Err(TuttiError::ConfigValidation(
            "hook must specify workflow or run".to_string(),
        ));
    };

    if !result.success
        && hook.fail_mode.unwrap_or(WorkflowFailMode::Open) == WorkflowFailMode::Closed
    {
        return Err(TuttiError::ConfigValidation(format!(
            "workflow_complete hook failed for workflow '{}': {}",
            payload.workflow_name, result.workflow_name
        )));
    }

    if !result.success {
        for step in &mut result.step_results {
            if step.status == StepStatus::Failed {
                step.status = StepStatus::Warning;
            }
        }
        result.success = true;
        result.failed_steps.clear();
    }

    Ok(result)
}

fn hook_matches_workflow_complete(hook: &HookConfig, payload: &WorkflowCompletePayload) -> bool {
    if let Some(source) = hook.workflow_source {
        let expected = match source {
            HookWorkflowSource::Run => "run",
            HookWorkflowSource::Verify => "verify",
            HookWorkflowSource::HookAgentStop => "hook_agent_stop",
            HookWorkflowSource::ObserveCycle => "observe_cycle",
            HookWorkflowSource::HookWorkflowComplete => "hook_workflow_complete",
        };
        if payload.workflow_source != expected {
            return false;
        }
    }
    if let Some(name) = hook.workflow_name.as_deref()
        && payload.workflow_name != name
    {
        return false;
    }
    if let Some(agent_filter) = hook.agent.as_deref() {
        return payload
            .agent_scope
            .as_deref()
            .is_some_and(|a| a == agent_filter);
    }
    true
}

fn load_command_policy() -> Option<PermissionsConfig> {
    crate::config::GlobalConfig::load()
        .ok()
        .and_then(|global| global.permissions)
}

fn load_retry_policy() -> Option<RetryPolicy> {
    crate::config::GlobalConfig::load()
        .ok()
        .and_then(|global| retry_policy_from_resilience(global.resilience.as_ref()))
}

pub fn execute_workflow_with_hooks(
    config: &TuttiConfig,
    project_root: &Path,
    resolved: &ResolvedWorkflow,
    options: &ExecuteOptions,
    agent_scope: Option<&str>,
    resume: Option<&ResumeContext>,
) -> Result<ExecutionResult> {
    let running_before = running_sessions(config);
    let executor = WorkflowExecutor::new(config, project_root);
    let run_id = resume
        .map(|r| r.run_id.clone())
        .unwrap_or_else(generate_run_id);
    let _ = append_control_event(
        project_root,
        &ControlEvent {
            event: "workflow.started".to_string(),
            workspace: config.workspace.name.clone(),
            agent: agent_scope.map(|s| s.to_string()),
            timestamp: Utc::now(),
            correlation_id: run_id.clone(),
            data: Some(serde_json::json!({
                "workflow_name": resolved.name,
                "origin": options.origin.as_str(),
                "strict": options.strict,
                "resumed": resume.is_some()
            })),
        },
    );
    let result = executor.execute(resolved, options, agent_scope, Some(&run_id), resume)?;
    reclaim_non_persistent_sessions(config, project_root, &running_before)?;

    // Recursion guard: don't emit workflow_complete from workflow_complete hooks.
    if options.origin != ExecutionOrigin::HookWorkflowComplete {
        let payload = WorkflowCompletePayload {
            workspace_name: config.workspace.name.clone(),
            project_root: project_root.to_path_buf(),
            workflow_name: result.workflow_name.clone(),
            workflow_source: options.origin.as_str().to_string(),
            success: result.success,
            agent_scope: agent_scope.map(|s| s.to_string()),
        };
        HookDispatcher::dispatch_workflow_complete(config, &payload)?;
    }

    let event_name = if result.success {
        "workflow.completed"
    } else {
        "workflow.failed"
    };
    let _ = append_control_event(
        project_root,
        &ControlEvent {
            event: event_name.to_string(),
            workspace: config.workspace.name.clone(),
            agent: agent_scope.map(|s| s.to_string()),
            timestamp: Utc::now(),
            correlation_id: result.run_id.clone(),
            data: Some(serde_json::json!({
                "workflow_name": result.workflow_name.clone(),
                "origin": options.origin.as_str(),
                "success": result.success,
                "failed_steps": result.failed_steps.clone(),
                "strict": result.strict
            })),
        },
    );

    Ok(result)
}

fn ensure_agent_session_running(
    config: &TuttiConfig,
    project_root: &Path,
    agent: &str,
) -> Result<()> {
    let session = TmuxSession::session_name(&config.workspace.name, agent);
    if TmuxSession::session_exists(&session) {
        return Ok(());
    }
    start_and_wait_ready(project_root, config, agent, &session)
}

fn running_sessions(config: &TuttiConfig) -> std::collections::HashSet<String> {
    let mut out = std::collections::HashSet::new();
    for agent in &config.agents {
        let session = TmuxSession::session_name(&config.workspace.name, &agent.name);
        if TmuxSession::session_exists(&session) {
            out.insert(session);
        }
    }
    out
}

fn reclaim_non_persistent_sessions(
    config: &TuttiConfig,
    project_root: &Path,
    running_before: &std::collections::HashSet<String>,
) -> Result<()> {
    for agent in config.agents.iter().filter(|a| !a.persistent) {
        let session = TmuxSession::session_name(&config.workspace.name, &agent.name);
        if running_before.contains(&session) || !TmuxSession::session_exists(&session) {
            continue;
        }
        TmuxSession::kill_session(&session)?;
        let _ = crate::state::update_status_if_exists(project_root, &agent.name, "Stopped");
    }
    Ok(())
}

pub fn save_verify_summary(
    project_root: &Path,
    workflow_name: &str,
    strict: bool,
    agent_scope: Option<&str>,
    result: &ExecutionResult,
) -> Result<()> {
    let summary = VerifyLastSummary {
        workflow_name: workflow_name.to_string(),
        timestamp: Utc::now(),
        success: result.success,
        failed_steps: result.failed_steps.clone(),
        strict,
        agent_scope: agent_scope.map(|s| s.to_string()),
    };
    save_verify_last_summary(project_root, &summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentConfig, DefaultsConfig, PermissionsConfig, WorkspaceConfig};
    use std::collections::HashMap;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn sample_config(workflow: WorkflowConfig, hooks: Vec<HookConfig>) -> TuttiConfig {
        TuttiConfig {
            workspace: WorkspaceConfig {
                name: "ws".to_string(),
                description: None,
                env: None,
                auth: None,
            },
            defaults: DefaultsConfig {
                worktree: true,
                runtime: Some("claude-code".to_string()),
            },
            launch: None,
            agents: vec![AgentConfig {
                name: "backend".to_string(),
                runtime: Some("claude-code".to_string()),
                scope: None,
                prompt: None,
                depends_on: vec![],
                worktree: None,
                fresh_worktree: None,
                branch: None,
                persistent: false,
                memory: None,
                env: HashMap::new(),
            }],
            tool_packs: vec![],
            workflows: vec![workflow],
            hooks,
            handoff: None,
            observe: None,
            budget: None,
            webhooks: vec![],
        }
    }

    fn init_git_repo(dir: &Path) {
        git_run(dir, &["init"]);
        git_run(dir, &["config", "user.email", "tutti-tests@example.com"]);
        git_run(dir, &["config", "user.name", "Tutti Tests"]);
        std::fs::write(dir.join("README.md"), "ok\n").unwrap();
        git_run(dir, &["add", "README.md"]);
        git_run(dir, &["commit", "-m", "init"]);
    }

    fn git_run(dir: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn git_stdout(dir: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    #[test]
    fn command_fail_open_continues() {
        let workflow = WorkflowConfig {
            name: "verify".to_string(),
            description: None,
            schedule: None,
            steps: vec![
                WorkflowStepConfig::Command {
                    id: None,
                    depends_on: vec![],
                    run: "echo one".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    subdir: None,
                    agent: None,
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Open),
                    output_json: None,
                },
                WorkflowStepConfig::Command {
                    id: None,
                    depends_on: vec![],
                    run: "exit 7".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    subdir: None,
                    agent: None,
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Open),
                    output_json: None,
                },
                WorkflowStepConfig::Command {
                    id: None,
                    depends_on: vec![],
                    run: "echo three".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    subdir: None,
                    agent: None,
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Open),
                    output_json: None,
                },
            ],
        };

        let dir = std::env::temp_dir().join("tutti-test-automation-open");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = sample_config(workflow, vec![]);
        let resolver = WorkflowResolver::new(&config, &dir);
        let opts = ExecuteOptions {
            strict: false,
            force_open_commands: false,
            command_policy: None,
            retry_policy: None,
            origin: ExecutionOrigin::Run,
            hook_event: None,
            hook_agent: None,
        };
        let resolved = resolver.resolve("verify", None, &opts).unwrap();
        let executor = WorkflowExecutor::new(&config, &dir);
        let result = executor
            .execute(&resolved, &opts, None, None, None)
            .unwrap();

        assert!(result.success);
        assert_eq!(result.warning_count(), 1);
        assert_eq!(result.step_results.len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_fail_closed_aborts() {
        let workflow = WorkflowConfig {
            name: "verify".to_string(),
            description: None,
            schedule: None,
            steps: vec![
                WorkflowStepConfig::Command {
                    id: None,
                    depends_on: vec![],
                    run: "echo one".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    subdir: None,
                    agent: None,
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Closed),
                    output_json: None,
                },
                WorkflowStepConfig::Command {
                    id: None,
                    depends_on: vec![],
                    run: "exit 9".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    subdir: None,
                    agent: None,
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Closed),
                    output_json: None,
                },
                WorkflowStepConfig::Command {
                    id: None,
                    depends_on: vec![],
                    run: "echo never".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    subdir: None,
                    agent: None,
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Closed),
                    output_json: None,
                },
            ],
        };

        let dir = std::env::temp_dir().join("tutti-test-automation-closed");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = sample_config(workflow, vec![]);
        let resolver = WorkflowResolver::new(&config, &dir);
        let opts = ExecuteOptions {
            strict: false,
            force_open_commands: false,
            command_policy: None,
            retry_policy: None,
            origin: ExecutionOrigin::Run,
            hook_event: None,
            hook_agent: None,
        };
        let resolved = resolver.resolve("verify", None, &opts).unwrap();
        let executor = WorkflowExecutor::new(&config, &dir);
        let result = executor
            .execute(&resolved, &opts, None, None, None)
            .unwrap();

        assert!(!result.success);
        assert_eq!(result.failed_steps, vec![2]);
        assert_eq!(result.step_results.len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn strict_mode_forces_closed() {
        let workflow = WorkflowConfig {
            name: "verify".to_string(),
            description: None,
            schedule: None,
            steps: vec![WorkflowStepConfig::Command {
                id: None,
                depends_on: vec![],
                run: "exit 4".to_string(),
                cwd: Some(WorkflowCommandCwd::Workspace),
                subdir: None,
                agent: None,
                timeout_secs: Some(30),
                fail_mode: Some(WorkflowFailMode::Open),
                output_json: None,
            }],
        };

        let dir = std::env::temp_dir().join("tutti-test-automation-strict");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = sample_config(workflow, vec![]);
        let resolver = WorkflowResolver::new(&config, &dir);
        let opts = ExecuteOptions {
            strict: true,
            force_open_commands: false,
            command_policy: None,
            retry_policy: None,
            origin: ExecutionOrigin::Verify,
            hook_event: None,
            hook_agent: None,
        };
        let resolved = resolver.resolve("verify", None, &opts).unwrap();
        let executor = WorkflowExecutor::new(&config, &dir);
        let result = executor
            .execute(&resolved, &opts, None, None, None)
            .unwrap();

        assert!(!result.success);
        assert_eq!(result.failed_steps, vec![1]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_agent_worktree_cwd_resolves_to_agent_worktree_path() {
        let workflow = WorkflowConfig {
            name: "verify".to_string(),
            description: None,
            schedule: None,
            steps: vec![WorkflowStepConfig::Command {
                id: None,
                depends_on: vec![],
                run: "git status --short".to_string(),
                cwd: Some(WorkflowCommandCwd::AgentWorktree),
                subdir: None,
                agent: Some("backend".to_string()),
                timeout_secs: Some(30),
                fail_mode: Some(WorkflowFailMode::Closed),
                output_json: None,
            }],
        };

        let dir = std::env::temp_dir().join("tutti-test-agent-worktree-cwd");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".tutti/worktrees/backend")).unwrap();

        let config = sample_config(workflow, vec![]);
        let opts = ExecuteOptions {
            strict: false,
            force_open_commands: false,
            command_policy: None,
            retry_policy: None,
            origin: ExecutionOrigin::Run,
            hook_event: None,
            hook_agent: None,
        };

        let resolved = WorkflowResolver::new(&config, &dir)
            .resolve("verify", None, &opts)
            .unwrap();
        match &resolved.steps[0] {
            ResolvedStep::Command { cwd, .. } => {
                assert_eq!(*cwd, dir.join(".tutti/worktrees/backend"));
            }
            _ => panic!("expected command step"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_agent_worktree_cwd_requires_existing_worktree() {
        let workflow = WorkflowConfig {
            name: "verify".to_string(),
            description: None,
            schedule: None,
            steps: vec![WorkflowStepConfig::Command {
                id: None,
                depends_on: vec![],
                run: "git status --short".to_string(),
                cwd: Some(WorkflowCommandCwd::AgentWorktree),
                subdir: None,
                agent: Some("backend".to_string()),
                timeout_secs: Some(30),
                fail_mode: Some(WorkflowFailMode::Closed),
                output_json: None,
            }],
        };

        let dir = std::env::temp_dir().join("tutti-test-agent-worktree-missing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = sample_config(workflow, vec![]);
        let opts = ExecuteOptions {
            strict: false,
            force_open_commands: false,
            command_policy: None,
            retry_policy: None,
            origin: ExecutionOrigin::Run,
            hook_event: None,
            hook_agent: None,
        };

        let err = WorkflowResolver::new(&config, &dir)
            .resolve("verify", None, &opts)
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("agent worktree not found for 'backend'")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_policy_block_closed_fails_step() {
        let workflow = WorkflowConfig {
            name: "verify".to_string(),
            description: None,
            schedule: None,
            steps: vec![WorkflowStepConfig::Command {
                id: None,
                depends_on: vec![],
                run: "echo blocked".to_string(),
                cwd: Some(WorkflowCommandCwd::Workspace),
                subdir: None,
                agent: Some("backend".to_string()),
                timeout_secs: Some(30),
                fail_mode: Some(WorkflowFailMode::Closed),
                output_json: None,
            }],
        };

        let dir = std::env::temp_dir().join("tutti-test-command-policy-block-closed");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = sample_config(workflow, vec![]);
        let opts = ExecuteOptions {
            strict: false,
            force_open_commands: false,
            command_policy: Some(PermissionsConfig {
                allow: vec!["git status".to_string()],
            }),
            retry_policy: None,
            origin: ExecutionOrigin::Run,
            hook_event: None,
            hook_agent: None,
        };
        let resolved = WorkflowResolver::new(&config, &dir)
            .resolve("verify", None, &opts)
            .unwrap();
        let result = WorkflowExecutor::new(&config, &dir)
            .execute(&resolved, &opts, None, None, None)
            .unwrap();

        assert!(!result.success);
        assert_eq!(result.failed_steps, vec![1]);
        assert!(
            result.step_results[0]
                .message
                .as_deref()
                .is_some_and(|m| m.contains("blocked by permissions policy"))
        );
        assert!(
            result.step_results[0]
                .message
                .as_deref()
                .is_some_and(|m| m.contains("hint: add allow rule"))
        );

        let decisions = crate::state::load_policy_decisions(&dir).unwrap();
        assert!(decisions.iter().any(|d| {
            d.action == "workflow_command"
                && d.decision == "block"
                && d.agent.as_deref() == Some("backend")
        }));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_policy_block_open_warns_and_continues() {
        let workflow = WorkflowConfig {
            name: "verify".to_string(),
            description: None,
            schedule: None,
            steps: vec![
                WorkflowStepConfig::Command {
                    id: None,
                    depends_on: vec![],
                    run: "echo blocked".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    subdir: None,
                    agent: Some("backend".to_string()),
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Open),
                    output_json: None,
                },
                WorkflowStepConfig::Command {
                    id: None,
                    depends_on: vec![],
                    run: "echo ok".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    subdir: None,
                    agent: Some("backend".to_string()),
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Open),
                    output_json: None,
                },
            ],
        };

        let dir = std::env::temp_dir().join("tutti-test-command-policy-block-open");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = sample_config(workflow, vec![]);
        let opts = ExecuteOptions {
            strict: false,
            force_open_commands: false,
            command_policy: Some(PermissionsConfig {
                allow: vec!["git status".to_string()],
            }),
            retry_policy: None,
            origin: ExecutionOrigin::Run,
            hook_event: None,
            hook_agent: None,
        };
        let resolved = WorkflowResolver::new(&config, &dir)
            .resolve("verify", None, &opts)
            .unwrap();
        let result = WorkflowExecutor::new(&config, &dir)
            .execute(&resolved, &opts, None, None, None)
            .unwrap();

        assert!(result.success);
        assert_eq!(result.warning_count(), 2);
        assert_eq!(result.step_results.len(), 2);

        let decisions = crate::state::load_policy_decisions(&dir).unwrap();
        assert_eq!(
            decisions
                .iter()
                .filter(|d| d.action == "workflow_command" && d.decision == "block")
                .count(),
            2
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn execute_with_hooks_emits_started_and_completed_with_same_correlation() {
        let workflow = WorkflowConfig {
            name: "verify".to_string(),
            description: None,
            schedule: None,
            steps: vec![WorkflowStepConfig::Command {
                id: None,
                depends_on: vec![],
                run: "echo ok".to_string(),
                cwd: Some(WorkflowCommandCwd::Workspace),
                subdir: None,
                agent: None,
                timeout_secs: Some(30),
                fail_mode: Some(WorkflowFailMode::Closed),
                output_json: None,
            }],
        };

        let dir = std::env::temp_dir().join("tutti-test-workflow-start-event");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = sample_config(workflow, vec![]);
        let resolver = WorkflowResolver::new(&config, &dir);
        let opts = ExecuteOptions {
            strict: false,
            force_open_commands: false,
            command_policy: None,
            retry_policy: None,
            origin: ExecutionOrigin::Run,
            hook_event: None,
            hook_agent: None,
        };
        let resolved = resolver.resolve("verify", None, &opts).unwrap();
        let result =
            execute_workflow_with_hooks(&config, &dir, &resolved, &opts, None, None).unwrap();

        let events = crate::state::load_control_events(&dir).unwrap();
        let started = events
            .iter()
            .find(|e| e.event == "workflow.started")
            .unwrap();
        let completed = events
            .iter()
            .find(|e| e.event == "workflow.completed")
            .unwrap();
        assert_eq!(result.run_id, started.correlation_id);
        assert_eq!(result.run_id, completed.correlation_id);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hook_dispatch_filters_by_agent() {
        let workflow = WorkflowConfig {
            name: "verify".to_string(),
            description: None,
            schedule: None,
            steps: vec![WorkflowStepConfig::Command {
                id: None,
                depends_on: vec![],
                run: "echo hook".to_string(),
                cwd: Some(WorkflowCommandCwd::Workspace),
                subdir: None,
                agent: None,
                timeout_secs: Some(30),
                fail_mode: Some(WorkflowFailMode::Open),
                output_json: None,
            }],
        };
        let hooks = vec![HookConfig {
            event: HookEvent::AgentStop,
            agent: Some("backend".to_string()),
            workflow_source: None,
            workflow_name: None,
            workflow: Some("verify".to_string()),
            run: None,
            timeout_secs: None,
            fail_mode: Some(WorkflowFailMode::Open),
        }];

        let dir = std::env::temp_dir().join("tutti-test-hook-filter");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let config = sample_config(workflow, hooks);
        let payload = HookEventPayload {
            workspace_name: "ws".to_string(),
            project_root: dir.clone(),
            agent_name: "frontend".to_string(),
            runtime: "claude-code".to_string(),
            session_name: "tutti-ws-frontend".to_string(),
            reason: "manual".to_string(),
        };

        let results = HookDispatcher::dispatch_agent_stop(&config, &payload).unwrap();
        assert!(results.is_empty());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_template_replaces_known_outputs_and_rejects_unresolved() {
        let mut outputs = HashMap::new();
        outputs.insert(
            "scan".to_string(),
            StepOutputValue {
                path: PathBuf::from("/tmp/scan.json"),
                json: serde_json::json!({"issues": 2}),
            },
        );
        let rendered = render_template(
            "cat {{output.scan.path}} && echo {{output.scan.json}}",
            &outputs,
            false,
        )
        .unwrap();
        assert!(rendered.contains("/tmp/scan.json"));
        assert!(rendered.contains("{\"issues\":2}"));

        let err = render_template("{{output.missing.path}}", &outputs, false).unwrap_err();
        assert!(
            err.to_string()
                .contains("unresolved workflow output template")
        );
    }

    #[test]
    fn retry_policy_requires_more_than_one_attempt() {
        let resilience = ResilienceConfig {
            provider_down_strategy: None,
            save_state_on_failure: false,
            rate_limit_strategy: None,
            retry_max_attempts: Some(1),
            retry_initial_backoff_ms: Some(50),
            retry_max_backoff_ms: Some(200),
        };
        assert!(retry_policy_from_resilience(Some(&resilience)).is_none());
    }

    #[test]
    fn run_shell_command_with_retry_retries_until_success() {
        let dir = std::env::temp_dir().join("tutti-test-command-retry");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let policy = RetryPolicy {
            max_attempts: 3,
            initial_backoff_ms: 1,
            max_backoff_ms: 5,
        };
        let command = "n=$(cat .retry-count 2>/dev/null || echo 0); n=$((n+1)); echo \"$n\" > .retry-count; if [ \"$n\" -lt 2 ]; then exit 1; fi";
        let outcome = run_shell_command_with_retry(command, &dir, 30, Some(&policy)).unwrap();
        assert_eq!(outcome.attempts, 2);
        assert_eq!(outcome.outcome.exit_code, Some(0));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workflow_complete_hook_filters_by_source() {
        let hook = HookConfig {
            event: HookEvent::WorkflowComplete,
            agent: None,
            workflow_source: Some(HookWorkflowSource::ObserveCycle),
            workflow_name: Some("verify".to_string()),
            workflow: Some("verify".to_string()),
            run: None,
            timeout_secs: None,
            fail_mode: Some(WorkflowFailMode::Open),
        };
        let payload = WorkflowCompletePayload {
            workspace_name: "ws".to_string(),
            project_root: PathBuf::from("/tmp/ws"),
            workflow_name: "verify".to_string(),
            workflow_source: "observe_cycle".to_string(),
            success: true,
            agent_scope: None,
        };
        assert!(hook_matches_workflow_complete(&hook, &payload));
    }

    #[test]
    fn inject_prompt_files_copies_workspace_state() {
        let dir = std::env::temp_dir().join("tutti-test-inject-files");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let source = dir.join(".tutti/state/snapshot.json");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "{\"ok\":true}\n").unwrap();

        let destination = dir.join(".tutti/worktrees/conductor/.tutti/state/snapshot.json");
        let files = vec![PromptInjectedFile {
            source: source.clone(),
            destination: destination.clone(),
        }];
        inject_prompt_files(&files).unwrap();

        assert_eq!(
            std::fs::read_to_string(destination).unwrap(),
            "{\"ok\":true}\n"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_resume_context_filters_failed_steps() {
        let dir = std::env::temp_dir().join("tutti-test-resume-context");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::state::ensure_tutti_dir(&dir).unwrap();

        let checkpoint = serde_json::json!({
            "run_id": "run123",
            "workflow_name": "verify",
            "strict": false,
            "origin": "run",
            "agent_scope": "backend",
            "started_at": Utc::now(),
            "finished_at": Utc::now(),
            "success": false,
            "failed_steps": [2],
            "step_results": [
                {
                    "index": 1,
                    "step_type": "command",
                    "status": "success",
                    "duration_ms": 1,
                    "exit_code": 0,
                    "timed_out": false,
                    "message": null,
                    "stdout": "",
                    "stderr": ""
                },
                {
                    "index": 2,
                    "step_type": "command",
                    "status": "failed",
                    "duration_ms": 1,
                    "exit_code": 1,
                    "timed_out": false,
                    "message": "boom",
                    "stdout": "",
                    "stderr": ""
                }
            ],
            "output_files": {}
        });
        crate::state::save_workflow_checkpoint(&dir, "run123", &checkpoint).unwrap();

        let resumed = load_resume_context(&dir, "run123").unwrap().unwrap();
        assert_eq!(resumed.workflow_name, "verify");
        assert!(resumed.completed_steps.contains(&1));
        assert!(!resumed.completed_steps.contains(&2));
        assert_eq!(resumed.step_results.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_resume_context_rejects_completed_run() {
        let dir = std::env::temp_dir().join("tutti-test-resume-complete");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::state::ensure_tutti_dir(&dir).unwrap();

        let checkpoint = serde_json::json!({
            "run_id": "run-ok",
            "workflow_name": "verify",
            "strict": false,
            "origin": "run",
            "agent_scope": null,
            "started_at": Utc::now(),
            "finished_at": Utc::now(),
            "success": true,
            "failed_steps": [],
            "step_results": [],
            "output_files": {}
        });
        crate::state::save_workflow_checkpoint(&dir, "run-ok", &checkpoint).unwrap();

        let err = load_resume_context(&dir, "run-ok").unwrap_err();
        assert!(err.to_string().contains("already succeeded"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn execute_persists_step_intent_and_outcome() {
        let workflow = WorkflowConfig {
            name: "verify".to_string(),
            description: None,
            schedule: None,
            steps: vec![WorkflowStepConfig::Command {
                id: None,
                depends_on: vec![],
                run: "echo ok".to_string(),
                cwd: Some(WorkflowCommandCwd::Workspace),
                subdir: None,
                agent: None,
                timeout_secs: Some(30),
                fail_mode: Some(WorkflowFailMode::Closed),
                output_json: None,
            }],
        };

        let dir = std::env::temp_dir().join("tutti-test-step-intent-outcome");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::state::ensure_tutti_dir(&dir).unwrap();

        let config = sample_config(workflow, vec![]);
        let opts = ExecuteOptions {
            strict: false,
            force_open_commands: false,
            command_policy: None,
            retry_policy: None,
            origin: ExecutionOrigin::Run,
            hook_event: None,
            hook_agent: None,
        };
        let resolved = WorkflowResolver::new(&config, &dir)
            .resolve("verify", None, &opts)
            .unwrap();
        let result = WorkflowExecutor::new(&config, &dir)
            .execute(&resolved, &opts, None, None, None)
            .unwrap();

        let intent = crate::state::load_workflow_intent(&dir, &result.run_id, "001-command")
            .unwrap()
            .unwrap();
        assert_eq!(intent.step_type, "command");
        assert!(
            intent
                .outcome
                .as_ref()
                .is_some_and(|outcome| outcome.success)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn record_step_intent_preserves_original_planned_at() {
        let dir = std::env::temp_dir().join("tutti-test-intent-preserve-planned-at");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::state::ensure_tutti_dir(&dir).unwrap();

        let step = ResolvedStep::Command {
            step_id: None,
            depends_on: vec![],
            run: "echo ok".to_string(),
            cwd: dir.clone(),
            agent: None,
            timeout_secs: 30,
            fail_mode: WorkflowFailMode::Closed,
            output_json: None,
        };

        record_step_intent(&dir, "run1", "verify", 1, &step).unwrap();
        let first = crate::state::load_workflow_intent(&dir, "run1", "001-command")
            .unwrap()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        record_step_intent(&dir, "run1", "verify", 1, &step).unwrap();
        let second = crate::state::load_workflow_intent(&dir, "run1", "001-command")
            .unwrap()
            .unwrap();

        assert_eq!(second.planned_at, first.planned_at);
        assert_eq!(second.attempt, first.attempt + 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn execute_control_step_reports_intent_write_failure_as_failed_result() {
        let dir = std::env::temp_dir().join("tutti-test-dag-intent-failure");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Force intent persistence to fail (`.tutti` is a file, not a directory).
        std::fs::write(dir.join(".tutti"), "not-a-dir\n").unwrap();

        let workflow = WorkflowConfig {
            name: "autofix".to_string(),
            description: None,
            schedule: None,
            steps: vec![WorkflowStepConfig::EnsureRunning {
                depends_on: vec![],
                agent: "backend".to_string(),
                fail_mode: Some(WorkflowFailMode::Closed),
            }],
        };
        let config = sample_config(workflow, vec![]);
        let step = ResolvedStep::EnsureRunning {
            depends_on: vec![],
            agent: "backend".to_string(),
            session_name: "tutti-ws-backend".to_string(),
            fail_mode: WorkflowFailMode::Closed,
        };

        let outcome = execute_control_step(&config, &dir, "run-fail", "autofix", &step, 1).unwrap();
        assert_eq!(outcome.result.status, StepStatus::Failed);
        assert!(outcome.hard_fail);
        assert!(
            outcome
                .result
                .message
                .as_deref()
                .is_some_and(|m| m.contains("failed to write step intent"))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_resume_compensator_plan_warns_for_command_step() {
        let workflow = WorkflowConfig {
            name: "verify".to_string(),
            description: None,
            schedule: None,
            steps: vec![WorkflowStepConfig::Command {
                id: None,
                depends_on: vec![],
                run: "exit 1".to_string(),
                cwd: Some(WorkflowCommandCwd::Workspace),
                subdir: None,
                agent: None,
                timeout_secs: Some(30),
                fail_mode: Some(WorkflowFailMode::Closed),
                output_json: None,
            }],
        };
        let dir = std::env::temp_dir().join("tutti-test-resume-plan-command");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::state::ensure_tutti_dir(&dir).unwrap();

        let config = sample_config(workflow, vec![]);
        let checkpoint = serde_json::json!({
            "run_id": "run-command",
            "workflow_name": "verify",
            "strict": false,
            "origin": "run",
            "agent_scope": null,
            "started_at": Utc::now(),
            "finished_at": Utc::now(),
            "success": false,
            "failed_steps": [1],
            "step_results": [],
            "output_files": {}
        });
        crate::state::save_workflow_checkpoint(&dir, "run-command", &checkpoint).unwrap();

        let intent = WorkflowStepIntentRecord {
            run_id: "run-command".to_string(),
            workflow_name: "verify".to_string(),
            step_index: 1,
            step_id: "001-command".to_string(),
            step_type: "command".to_string(),
            planned_at: Utc::now(),
            intent: serde_json::json!({"run":"exit 1"}),
            attempt: 1,
            outcome: None,
        };
        crate::state::save_workflow_intent(&dir, "run-command", "001-command", &intent).unwrap();

        let resume = load_resume_context(&dir, "run-command").unwrap().unwrap();
        let opts = ExecuteOptions {
            strict: false,
            force_open_commands: false,
            command_policy: None,
            retry_policy: None,
            origin: ExecutionOrigin::Run,
            hook_event: None,
            hook_agent: None,
        };
        let resolved = WorkflowResolver::new(&config, &dir)
            .resolve("verify", None, &opts)
            .unwrap();
        let plan = build_resume_compensator_plan(&config, &dir, &resolved, &resume).unwrap();
        assert!(
            plan.iter()
                .any(|line| line.contains("best-effort") || line.contains("best effort"))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_resume_compensator_plan_detects_already_merged_land_branch() {
        let workflow = WorkflowConfig {
            name: "autoland".to_string(),
            description: None,
            schedule: None,
            steps: vec![WorkflowStepConfig::Land {
                depends_on: vec![],
                agent: "backend".to_string(),
                pr: Some(false),
                force: Some(false),
                fail_mode: Some(WorkflowFailMode::Closed),
            }],
        };
        let dir = std::env::temp_dir().join("tutti-test-resume-plan-land");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_git_repo(&dir);
        let default_branch = git_stdout(&dir, &["rev-parse", "--abbrev-ref", "HEAD"]);
        git_run(&dir, &["checkout", "-b", "tutti/backend"]);
        std::fs::write(dir.join("feature.txt"), "hello\n").unwrap();
        git_run(&dir, &["add", "feature.txt"]);
        git_run(&dir, &["commit", "-m", "feature"]);
        git_run(&dir, &["checkout", &default_branch]);
        git_run(
            &dir,
            &["merge", "--no-ff", "-m", "merge backend", "tutti/backend"],
        );
        crate::state::ensure_tutti_dir(&dir).unwrap();

        let config = sample_config(workflow, vec![]);
        let checkpoint = serde_json::json!({
            "run_id": "run-land",
            "workflow_name": "autoland",
            "strict": false,
            "origin": "run",
            "agent_scope": "backend",
            "started_at": Utc::now(),
            "finished_at": Utc::now(),
            "success": false,
            "failed_steps": [1],
            "step_results": [],
            "output_files": {}
        });
        crate::state::save_workflow_checkpoint(&dir, "run-land", &checkpoint).unwrap();

        let intent = WorkflowStepIntentRecord {
            run_id: "run-land".to_string(),
            workflow_name: "autoland".to_string(),
            step_index: 1,
            step_id: "001-land".to_string(),
            step_type: "land".to_string(),
            planned_at: Utc::now(),
            intent: serde_json::json!({"agent":"backend","pr":false,"force":false}),
            attempt: 1,
            outcome: None,
        };
        crate::state::save_workflow_intent(&dir, "run-land", "001-land", &intent).unwrap();

        let resume = load_resume_context(&dir, "run-land").unwrap().unwrap();
        let opts = ExecuteOptions {
            strict: false,
            force_open_commands: false,
            command_policy: None,
            retry_policy: None,
            origin: ExecutionOrigin::Run,
            hook_event: None,
            hook_agent: None,
        };
        let resolved = WorkflowResolver::new(&config, &dir)
            .resolve("autoland", None, &opts)
            .unwrap();
        let plan = build_resume_compensator_plan(&config, &dir, &resolved, &resume).unwrap();
        assert!(plan.iter().any(|line| line.contains("already merged")));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_resume_compensator_plan_land_fresh_branch_is_not_skipped() {
        let workflow = WorkflowConfig {
            name: "autoland".to_string(),
            description: None,
            schedule: None,
            steps: vec![WorkflowStepConfig::Land {
                depends_on: vec![],
                agent: "backend".to_string(),
                pr: Some(false),
                force: Some(false),
                fail_mode: Some(WorkflowFailMode::Closed),
            }],
        };
        let dir = std::env::temp_dir().join("tutti-test-resume-plan-land-fresh");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        init_git_repo(&dir);
        git_run(&dir, &["branch", "tutti/backend"]);
        crate::state::ensure_tutti_dir(&dir).unwrap();
        let worktree_path = dir.join(".tutti").join("worktrees").join("backend");
        git_run(
            &dir,
            &[
                "worktree",
                "add",
                worktree_path.to_str().unwrap(),
                "tutti/backend",
            ],
        );
        std::fs::write(worktree_path.join("dirty.txt"), "pending\n").unwrap();

        let config = sample_config(workflow, vec![]);
        let checkpoint = serde_json::json!({
            "run_id": "run-land-fresh",
            "workflow_name": "autoland",
            "strict": false,
            "origin": "run",
            "agent_scope": "backend",
            "started_at": Utc::now(),
            "finished_at": Utc::now(),
            "success": false,
            "failed_steps": [1],
            "step_results": [],
            "output_files": {}
        });
        crate::state::save_workflow_checkpoint(&dir, "run-land-fresh", &checkpoint).unwrap();

        let intent = WorkflowStepIntentRecord {
            run_id: "run-land-fresh".to_string(),
            workflow_name: "autoland".to_string(),
            step_index: 1,
            step_id: "001-land".to_string(),
            step_type: "land".to_string(),
            planned_at: Utc::now(),
            intent: serde_json::json!({"agent":"backend","pr":false,"force":false}),
            attempt: 1,
            outcome: None,
        };
        crate::state::save_workflow_intent(&dir, "run-land-fresh", "001-land", &intent).unwrap();

        let resume = load_resume_context(&dir, "run-land-fresh")
            .unwrap()
            .unwrap();
        let opts = ExecuteOptions {
            strict: false,
            force_open_commands: false,
            command_policy: None,
            retry_policy: None,
            origin: ExecutionOrigin::Run,
            hook_event: None,
            hook_agent: None,
        };
        let resolved = WorkflowResolver::new(&config, &dir)
            .resolve("autoland", None, &opts)
            .unwrap();
        let plan = build_resume_compensator_plan(&config, &dir, &resolved, &resume).unwrap();
        assert!(
            plan.iter()
                .any(|line| line.contains("not safely idempotent"))
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resolver_applies_command_subdir() {
        let dir = std::env::temp_dir().join("tutti-test-command-subdir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("backend")).unwrap();

        let workflow = WorkflowConfig {
            name: "verify".to_string(),
            description: None,
            schedule: None,
            steps: vec![WorkflowStepConfig::Command {
                id: None,
                depends_on: vec![],
                run: "echo ok".to_string(),
                cwd: Some(WorkflowCommandCwd::Workspace),
                subdir: Some("backend".to_string()),
                agent: None,
                timeout_secs: Some(30),
                fail_mode: Some(WorkflowFailMode::Closed),
                output_json: None,
            }],
        };
        let config = sample_config(workflow, vec![]);
        let opts = ExecuteOptions {
            strict: false,
            force_open_commands: false,
            command_policy: None,
            retry_policy: None,
            origin: ExecutionOrigin::Run,
            hook_event: None,
            hook_agent: None,
        };
        let resolved = WorkflowResolver::new(&config, &dir)
            .resolve("verify", None, &opts)
            .unwrap();

        match &resolved.steps[0] {
            ResolvedStep::Command { cwd, .. } => assert_eq!(cwd, &dir.join("backend")),
            _ => panic!("expected command step"),
        }

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_normalized_dependencies_defaults_to_previous_when_explicit_mode_enabled() {
        let steps = vec![
            ResolvedStep::EnsureRunning {
                depends_on: vec![],
                agent: "backend".to_string(),
                session_name: "tutti-ws-backend".to_string(),
                fail_mode: WorkflowFailMode::Closed,
            },
            ResolvedStep::Review {
                depends_on: vec![1],
                agent: "backend".to_string(),
                reviewer: "reviewer".to_string(),
                fail_mode: WorkflowFailMode::Open,
            },
            ResolvedStep::Land {
                depends_on: vec![],
                agent: "backend".to_string(),
                pr: false,
                force: true,
                fail_mode: WorkflowFailMode::Closed,
            },
        ];

        let deps = build_normalized_dependencies(&steps);
        assert_eq!(deps[0], Vec::<usize>::new());
        assert_eq!(deps[1], vec![1]);
        assert_eq!(deps[2], vec![2]);
    }

    #[test]
    fn resolver_supports_control_steps() {
        let config = TuttiConfig {
            workspace: WorkspaceConfig {
                name: "ws".to_string(),
                description: None,
                env: None,
                auth: None,
            },
            defaults: DefaultsConfig {
                worktree: true,
                runtime: Some("claude-code".to_string()),
            },
            launch: None,
            agents: vec![
                AgentConfig {
                    name: "backend".to_string(),
                    runtime: Some("claude-code".to_string()),
                    scope: None,
                    prompt: None,
                    depends_on: vec![],
                    worktree: None,
                    fresh_worktree: None,
                    branch: None,
                    persistent: false,
                    memory: None,
                    env: HashMap::new(),
                },
                AgentConfig {
                    name: "reviewer".to_string(),
                    runtime: Some("claude-code".to_string()),
                    scope: None,
                    prompt: None,
                    depends_on: vec![],
                    worktree: None,
                    fresh_worktree: None,
                    branch: None,
                    persistent: false,
                    memory: None,
                    env: HashMap::new(),
                },
            ],
            tool_packs: vec![],
            workflows: vec![
                WorkflowConfig {
                    name: "verify".to_string(),
                    description: None,
                    schedule: None,
                    steps: vec![WorkflowStepConfig::Command {
                        id: None,
                        depends_on: vec![],
                        run: "echo ok".to_string(),
                        cwd: Some(WorkflowCommandCwd::Workspace),
                        subdir: None,
                        agent: None,
                        timeout_secs: None,
                        fail_mode: None,
                        output_json: None,
                    }],
                },
                WorkflowConfig {
                    name: "autofix".to_string(),
                    description: None,
                    schedule: None,
                    steps: vec![
                        WorkflowStepConfig::EnsureRunning {
                            depends_on: vec![],
                            agent: "backend".to_string(),
                            fail_mode: None,
                        },
                        WorkflowStepConfig::Workflow {
                            depends_on: vec![],
                            workflow: "verify".to_string(),
                            agent: Some("backend".to_string()),
                            strict: Some(true),
                            fail_mode: Some(WorkflowFailMode::Closed),
                        },
                        WorkflowStepConfig::Review {
                            depends_on: vec![],
                            agent: "backend".to_string(),
                            reviewer: Some("reviewer".to_string()),
                            fail_mode: Some(WorkflowFailMode::Open),
                        },
                        WorkflowStepConfig::Land {
                            depends_on: vec![],
                            agent: "backend".to_string(),
                            pr: Some(false),
                            force: Some(true),
                            fail_mode: Some(WorkflowFailMode::Closed),
                        },
                    ],
                },
            ],
            hooks: vec![],
            handoff: None,
            observe: None,
            budget: None,
            webhooks: vec![],
        };

        let dir = std::env::temp_dir().join("tutti-test-resolver-control-steps");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let resolver = WorkflowResolver::new(&config, &dir);
        let options = ExecuteOptions {
            strict: false,
            force_open_commands: false,
            command_policy: None,
            retry_policy: None,
            origin: ExecutionOrigin::Run,
            hook_event: None,
            hook_agent: None,
        };

        let resolved = resolver.resolve("autofix", None, &options).unwrap();
        assert_eq!(resolved.steps.len(), 4);
        assert!(matches!(
            resolved.steps[0],
            ResolvedStep::EnsureRunning { .. }
        ));
        assert!(matches!(resolved.steps[1], ResolvedStep::Workflow { .. }));
        assert!(matches!(resolved.steps[2], ResolvedStep::Review { .. }));
        assert!(matches!(resolved.steps[3], ResolvedStep::Land { .. }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn expand_artifact_glob_replaces_workspace_and_agent() {
        let expanded =
            expand_artifact_glob("/tmp/{workspace}/{agent}/*.md", "my-project", "planner").unwrap();
        assert_eq!(expanded, "/tmp/my-project/planner/*.md");
    }

    #[test]
    fn expand_artifact_glob_expands_tilde() {
        let home = std::env::var("HOME").unwrap();
        let expanded = expand_artifact_glob("~/.gstack/test/*.md", "ws", "ag").unwrap();
        assert!(expanded.starts_with(&home));
        assert!(expanded.ends_with("/.gstack/test/*.md"));
    }

    #[test]
    fn snapshot_artifact_glob_captures_existing_files() {
        let dir = std::env::temp_dir().join("tutti-test-snapshot-glob");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let file1 = dir.join("design-001.md");
        let file2 = dir.join("design-002.md");
        std::fs::write(&file1, "doc1").unwrap();
        std::fs::write(&file2, "doc2").unwrap();

        let pattern = format!("{}/*.md", dir.display());
        let snapshot = snapshot_artifact_glob(&pattern).unwrap();
        assert_eq!(snapshot.len(), 2);
        assert!(snapshot.contains(&file1));
        assert!(snapshot.contains(&file2));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capture_artifact_picks_newest_new_file() {
        let dir = std::env::temp_dir().join("tutti-test-capture-artifact");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // Pre-existing file
        let old = dir.join("design-old.md");
        std::fs::write(&old, "old content").unwrap();

        let pattern = format!("{}/*.md", dir.display());
        let pre_snapshot = snapshot_artifact_glob(&pattern).unwrap();
        assert_eq!(pre_snapshot.len(), 1);

        // Simulate skill creating a new file
        std::thread::sleep(std::time::Duration::from_millis(50));
        let new_file = dir.join("design-new.md");
        std::fs::write(&new_file, "new artifact content").unwrap();

        let result = capture_artifact(&pre_snapshot, &pattern, "design_doc").unwrap();
        assert_eq!(result, new_file);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capture_artifact_zero_new_files_fails() {
        let dir = std::env::temp_dir().join("tutti-test-capture-zero");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let old = dir.join("design-old.md");
        std::fs::write(&old, "old").unwrap();

        let pattern = format!("{}/*.md", dir.display());
        let pre_snapshot = snapshot_artifact_glob(&pattern).unwrap();

        // No new files created
        let err = capture_artifact(&pre_snapshot, &pattern, "design_doc").unwrap_err();
        assert!(err.to_string().contains("matched no new files"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn capture_artifact_multiple_new_files_picks_newest() {
        let dir = std::env::temp_dir().join("tutti-test-capture-multi");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let pattern = format!("{}/*.md", dir.display());
        let pre_snapshot = snapshot_artifact_glob(&pattern).unwrap();

        // Create two new files with different mtimes
        let file1 = dir.join("design-a.md");
        std::fs::write(&file1, "first").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let file2 = dir.join("design-b.md");
        std::fs::write(&file2, "second (newest)").unwrap();

        let result = capture_artifact(&pre_snapshot, &pattern, "design_doc").unwrap();
        assert_eq!(result, file2);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn store_artifact_output_creates_output_value() {
        let dir = std::env::temp_dir().join("tutti-test-store-artifact");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".tutti/state/workflow-outputs")).unwrap();

        let artifact = dir.join("my-design.md");
        std::fs::write(&artifact, "# Design\nThis is the design doc.").unwrap();

        let result = store_artifact_output(&dir, "run-001", "design_doc", &artifact).unwrap();
        assert!(result.path.exists());
        assert!(matches!(result.json, Value::String(_)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn render_template_with_artifact_output() {
        let mut outputs = HashMap::new();
        outputs.insert(
            "design_doc".to_string(),
            StepOutputValue {
                path: PathBuf::from("/tmp/artifacts/design_doc.raw"),
                json: Value::String("design content".to_string()),
            },
        );

        let rendered = render_template("Read {{output.design_doc.path}}", &outputs, false).unwrap();
        assert_eq!(rendered, "Read /tmp/artifacts/design_doc.raw");
    }

    #[test]
    fn gstack_slug_missing_binary_returns_actionable_error() {
        // This test verifies the error message when gstack-slug is missing
        // Uses the injectable home dir parameter instead of mutating process env
        let result = resolve_gstack_slug_with_home("/nonexistent-path-for-test");
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("gstack-slug not found"));
    }
}
