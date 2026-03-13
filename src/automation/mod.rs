use crate::config::{
    HookConfig, HookEvent, HookWorkflowSource, TuttiConfig, WorkflowCommandCwd, WorkflowConfig,
    WorkflowFailMode, WorkflowStepConfig,
};
use crate::error::{Result, TuttiError};
use crate::health;
use crate::health::WaitFailureReason;
use crate::session::TmuxSession;
use crate::state::{
    AutomationRunRecord, VerifyLastSummary, append_automation_run, save_verify_last_summary,
    save_workflow_output,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;
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
    pub origin: ExecutionOrigin,
    pub hook_event: Option<String>,
    pub hook_agent: Option<String>,
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
                    run,
                    cwd,
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
                    let resolved_cwd = match cwd_mode {
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

                    let output_json = resolve_optional_path(&resolved_cwd, output_json.as_deref());
                    steps.push(ResolvedStep::Command {
                        step_id: id.clone(),
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
                WorkflowStepConfig::EnsureRunning { agent, fail_mode } => {
                    let effective_agent = agent_override.unwrap_or(agent.as_str());
                    self.ensure_agent_exists(effective_agent)?;
                    let session_name =
                        TmuxSession::session_name(&self.config.workspace.name, effective_agent);
                    steps.push(ResolvedStep::EnsureRunning {
                        agent: effective_agent.to_string(),
                        session_name,
                        fail_mode: effective_control_fail_mode(*fail_mode, options.strict),
                    });
                }
                WorkflowStepConfig::Workflow {
                    workflow,
                    agent,
                    strict,
                    fail_mode,
                } => {
                    if let Some(agent_name) = agent_override.or(agent.as_deref()) {
                        self.ensure_agent_exists(agent_name)?;
                    }
                    steps.push(ResolvedStep::Workflow {
                        workflow: workflow.clone(),
                        agent_override: agent_override.or(agent.as_deref()).map(|s| s.to_string()),
                        strict: strict.unwrap_or(options.strict),
                        fail_mode: effective_control_fail_mode(*fail_mode, options.strict),
                    });
                }
                WorkflowStepConfig::Land {
                    agent,
                    pr,
                    force,
                    fail_mode,
                } => {
                    let effective_agent = agent_override.unwrap_or(agent.as_str());
                    self.ensure_agent_exists(effective_agent)?;
                    steps.push(ResolvedStep::Land {
                        agent: effective_agent.to_string(),
                        pr: pr.unwrap_or(false),
                        force: force.unwrap_or(false),
                        fail_mode: effective_control_fail_mode(*fail_mode, options.strict),
                    });
                }
                WorkflowStepConfig::Review {
                    agent,
                    reviewer,
                    fail_mode,
                } => {
                    let effective_agent = agent_override.unwrap_or(agent.as_str());
                    self.ensure_agent_exists(effective_agent)?;
                    let resolved_reviewer = reviewer.clone().unwrap_or_else(|| "reviewer".into());
                    self.ensure_agent_exists(&resolved_reviewer)?;
                    steps.push(ResolvedStep::Review {
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
        run: String,
        cwd: PathBuf,
        agent: Option<String>,
        timeout_secs: u64,
        fail_mode: WorkflowFailMode,
        output_json: Option<PathBuf>,
    },
    EnsureRunning {
        agent: String,
        session_name: String,
        fail_mode: WorkflowFailMode,
    },
    Workflow {
        workflow: String,
        agent_override: Option<String>,
        strict: bool,
        fail_mode: WorkflowFailMode,
    },
    Land {
        agent: String,
        pr: bool,
        force: bool,
        fail_mode: WorkflowFailMode,
    },
    Review {
        agent: String,
        reviewer: String,
        fail_mode: WorkflowFailMode,
    },
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
    ) -> Result<ExecutionResult> {
        let started_at = Utc::now();
        let run_id = format!(
            "{}-{}",
            started_at.format("%Y%m%d%H%M%S"),
            std::process::id()
        );
        let mut success = true;
        let mut failed_steps = Vec::new();
        let mut step_results = Vec::with_capacity(workflow.steps.len());
        let mut outputs = HashMap::<String, StepOutputValue>::new();
        let mut output_files = HashMap::<String, String>::new();

        for (idx, step) in workflow.steps.iter().enumerate() {
            let step_index = idx + 1;
            match step {
                ResolvedStep::Prompt {
                    step_id,
                    text,
                    inject_files,
                    session_name,
                    ..
                } => {
                    let started = std::time::Instant::now();
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
                                        "wait_for_idle failed: target session exited".to_string(),
                                    ),
                                    None => (false, "wait_for_idle failed: unknown".to_string()),
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
                        if let (Some(id), Some(path)) = (step_id.as_deref(), output_json.as_ref()) {
                            match load_and_store_output(self.project_root, &run_id, id, path) {
                                Ok(saved) => {
                                    output_files
                                        .insert(id.to_string(), saved.path.display().to_string());
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

                    let cmd_result = run_shell_command(&rendered, cwd, *timeout_secs);

                    match cmd_result {
                        Ok(outcome) => {
                            let failed = outcome.timed_out || outcome.exit_code.unwrap_or(1) != 0;
                            if failed {
                                let message = if outcome.timed_out {
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
                        crate::cli::up::run(Some(agent), None, false, None, None)
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
                } => {
                    let started = std::time::Instant::now();
                    let nested_options = ExecuteOptions {
                        strict: *strict,
                        force_open_commands: options.force_open_commands,
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
                } => {
                    let started = std::time::Instant::now();
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
                } => {
                    let started = std::time::Instant::now();
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

        Ok(result)
    }
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
        )?
    } else if let Some(cmd) = hook.run.as_deref() {
        let resolved = ResolvedWorkflow {
            name: format!("hook:agent_stop:{}", payload.agent_name),
            description: Some("Generated hook command".to_string()),
            steps: vec![ResolvedStep::Command {
                step_id: None,
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
        )?
    } else if let Some(cmd) = hook.run.as_deref() {
        let resolved = ResolvedWorkflow {
            name: format!("hook:workflow_complete:{}", payload.workflow_name),
            description: Some("Generated hook command".to_string()),
            steps: vec![ResolvedStep::Command {
                step_id: None,
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

pub fn execute_workflow_with_hooks(
    config: &TuttiConfig,
    project_root: &Path,
    resolved: &ResolvedWorkflow,
    options: &ExecuteOptions,
    agent_scope: Option<&str>,
) -> Result<ExecutionResult> {
    let running_before = running_sessions(config);
    let executor = WorkflowExecutor::new(config, project_root);
    let result = executor.execute(resolved, options, agent_scope)?;
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

    Ok(result)
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
    use crate::config::{AgentConfig, DefaultsConfig, WorkspaceConfig};
    use std::collections::HashMap;
    use std::path::PathBuf;

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
                branch: None,
                persistent: false,
                env: HashMap::new(),
            }],
            tool_packs: vec![],
            workflows: vec![workflow],
            hooks,
            handoff: None,
            observe: None,
        }
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
                    run: "echo one".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    agent: None,
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Open),
                    output_json: None,
                },
                WorkflowStepConfig::Command {
                    id: None,
                    run: "exit 7".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    agent: None,
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Open),
                    output_json: None,
                },
                WorkflowStepConfig::Command {
                    id: None,
                    run: "echo three".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
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
            origin: ExecutionOrigin::Run,
            hook_event: None,
            hook_agent: None,
        };
        let resolved = resolver.resolve("verify", None, &opts).unwrap();
        let executor = WorkflowExecutor::new(&config, &dir);
        let result = executor.execute(&resolved, &opts, None).unwrap();

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
                    run: "echo one".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    agent: None,
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Closed),
                    output_json: None,
                },
                WorkflowStepConfig::Command {
                    id: None,
                    run: "exit 9".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    agent: None,
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Closed),
                    output_json: None,
                },
                WorkflowStepConfig::Command {
                    id: None,
                    run: "echo never".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
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
            origin: ExecutionOrigin::Run,
            hook_event: None,
            hook_agent: None,
        };
        let resolved = resolver.resolve("verify", None, &opts).unwrap();
        let executor = WorkflowExecutor::new(&config, &dir);
        let result = executor.execute(&resolved, &opts, None).unwrap();

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
                run: "exit 4".to_string(),
                cwd: Some(WorkflowCommandCwd::Workspace),
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
            origin: ExecutionOrigin::Verify,
            hook_event: None,
            hook_agent: None,
        };
        let resolved = resolver.resolve("verify", None, &opts).unwrap();
        let executor = WorkflowExecutor::new(&config, &dir);
        let result = executor.execute(&resolved, &opts, None).unwrap();

        assert!(!result.success);
        assert_eq!(result.failed_steps, vec![1]);
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
                run: "echo hook".to_string(),
                cwd: Some(WorkflowCommandCwd::Workspace),
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
                    branch: None,
                    persistent: false,
                    env: HashMap::new(),
                },
                AgentConfig {
                    name: "reviewer".to_string(),
                    runtime: Some("claude-code".to_string()),
                    scope: None,
                    prompt: None,
                    depends_on: vec![],
                    worktree: None,
                    branch: None,
                    persistent: false,
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
                        run: "echo ok".to_string(),
                        cwd: Some(WorkflowCommandCwd::Workspace),
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
                            agent: "backend".to_string(),
                            fail_mode: None,
                        },
                        WorkflowStepConfig::Workflow {
                            workflow: "verify".to_string(),
                            agent: Some("backend".to_string()),
                            strict: Some(true),
                            fail_mode: Some(WorkflowFailMode::Closed),
                        },
                        WorkflowStepConfig::Review {
                            agent: "backend".to_string(),
                            reviewer: Some("reviewer".to_string()),
                            fail_mode: Some(WorkflowFailMode::Open),
                        },
                        WorkflowStepConfig::Land {
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
        };

        let dir = std::env::temp_dir().join("tutti-test-resolver-control-steps");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let resolver = WorkflowResolver::new(&config, &dir);
        let options = ExecuteOptions {
            strict: false,
            force_open_commands: false,
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
