use crate::config::{
    HookConfig, HookEvent, HookWorkflowSource, PermissionsConfig, ResilienceConfig, TuttiConfig,
    WorkflowCommandCwd, WorkflowConfig, WorkflowFailMode, WorkflowStepConfig,
};
use crate::error::{Result, TuttiError};
use crate::health;
use crate::health::WaitFailureReason;
use crate::permissions::evaluate_command_policy;
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
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

const DEFAULT_TIMEOUT_SECS: u64 = 900;

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
                        output_json: self
                            .resolve_prompt_output_path(effective_agent, output_json)?,
                        wait_for_idle: wait_for_idle.unwrap_or(false),
                        wait_timeout_secs: wait_timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
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
        output_json: Option<PathBuf>,
        wait_for_idle: bool,
        wait_timeout_secs: u64,
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
                        step_id,
                        text,
                        output_json,
                        inject_files,
                        session_name,
                        ..
                    } => {
                        let started = std::time::Instant::now();
                        let rendered = match render_prompt_text(
                            text,
                            &outputs,
                            output_json.as_deref(),
                        ) {
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

                        if let Err(e) = inject_prompt_files(inject_files) {
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

                        if !TmuxSession::session_exists(session_name) {
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
                                    "target session is not running: {}",
                                    session_name
                                )),
                                stdout: None,
                                stderr: None,
                            });
                            break;
                        }

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
                            ..
                        } = step
                        {
                            if *wait_for_idle {
                                let wait = health::wait_for_agent_idle(
                                    runtime,
                                    session_name,
                                    Duration::from_secs((*wait_timeout_secs).max(1)),
                                    Duration::from_secs(5),
                                )?;
                                if !wait.is_completed() {
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

                        let cmd_result = run_shell_command_with_retry(
                            &rendered,
                            cwd,
                            *timeout_secs,
                            options.retry_policy.as_ref(),
                        );

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

                        match with_project_root(self.project_root, || {
                            crate::cli::up::run(Some(agent), None, false, false, None, None)
                        }) {
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
                            crate::cli::land::run(agent, *pr, *force)
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

#[derive(Debug)]
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
            ..
        } => json!({
            "agent": agent,
            "session_name": session_name,
            "text_chars": text.chars().count(),
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

fn render_prompt_text(
    input: &str,
    outputs: &HashMap<String, StepOutputValue>,
    output_json: Option<&Path>,
) -> Result<String> {
    let mut rendered = render_template(input, outputs, false)?;
    if let Some(path) = output_json {
        rendered.push_str(
            "\n\nStructured output contract:\n\
- Write a single valid JSON object to this exact path: ",
        );
        rendered.push_str(&path.display().to_string());
        rendered.push_str(
            "\n\
- The file must contain JSON only. Do not wrap it in Markdown fences.\n\
- Overwrite the file if it already exists.\n",
        );
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
            let run_result = run_tt_subcommand(project_root, &args);
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
    let bin = std::env::current_exe()?;
    let output = Command::new(bin)
        .args(args)
        .current_dir(project_root)
        .output()?;
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
    with_project_root(project_root, || {
        crate::cli::up::run(Some(agent), None, false, false, None, None)
    })
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
    fn render_prompt_text_appends_structured_output_contract() {
        let mut outputs = HashMap::new();
        outputs.insert(
            "plan".to_string(),
            StepOutputValue {
                path: PathBuf::from("/tmp/plan.json"),
                json: serde_json::json!({"scope": "small"}),
            },
        );

        let rendered = render_prompt_text(
            "Use {{output.plan.path}} first.",
            &outputs,
            Some(Path::new("/tmp/result.json")),
        )
        .unwrap();

        assert!(rendered.contains("Use /tmp/plan.json first."));
        assert!(rendered.contains("Structured output contract"));
        assert!(rendered.contains("/tmp/result.json"));
        assert!(rendered.contains("JSON only"));
    }

    #[test]
    fn render_prompt_text_without_output_json_does_not_append_contract() {
        let outputs = HashMap::new();

        let rendered = render_prompt_text("Keep this prompt stable.", &outputs, None).unwrap();

        assert_eq!(rendered, "Keep this prompt stable.");
        assert!(!rendered.contains("Structured output contract"));
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
}
