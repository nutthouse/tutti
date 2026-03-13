use crate::config::{
    HookConfig, HookEvent, TuttiConfig, WorkflowCommandCwd, WorkflowConfig, WorkflowFailMode,
    WorkflowStepConfig,
};
use crate::error::{Result, TuttiError};
use crate::session::TmuxSession;
use crate::state::{
    AutomationRunRecord, VerifyLastSummary, append_automation_run, save_verify_last_summary,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
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
}

impl ExecutionOrigin {
    fn as_str(self) -> &'static str {
        match self {
            ExecutionOrigin::Run => "run",
            ExecutionOrigin::Verify => "verify",
            ExecutionOrigin::HookAgentStop => "hook_agent_stop",
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
                WorkflowStepConfig::Prompt { agent, text } => {
                    let effective_agent = agent_override.unwrap_or(agent.as_str());
                    self.ensure_agent_exists(effective_agent)?;
                    let session_name =
                        TmuxSession::session_name(&self.config.workspace.name, effective_agent);
                    steps.push(ResolvedStep::Prompt {
                        agent: effective_agent.to_string(),
                        text: text.clone(),
                        session_name,
                    });
                }
                WorkflowStepConfig::Command {
                    run,
                    cwd,
                    agent,
                    timeout_secs,
                    fail_mode,
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

                    steps.push(ResolvedStep::Command {
                        run: run.clone(),
                        cwd: resolved_cwd,
                        agent: effective_agent.map(|s| s.to_string()),
                        timeout_secs: timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
                        fail_mode: effective_fail_mode(
                            *fail_mode,
                            options.strict,
                            options.force_open_commands,
                        ),
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

#[derive(Debug, Clone)]
pub struct ResolvedWorkflow {
    pub name: String,
    pub description: Option<String>,
    pub steps: Vec<ResolvedStep>,
}

#[derive(Debug, Clone)]
pub enum ResolvedStep {
    Prompt {
        agent: String,
        text: String,
        session_name: String,
    },
    Command {
        run: String,
        cwd: PathBuf,
        agent: Option<String>,
        timeout_secs: u64,
        fail_mode: WorkflowFailMode,
    },
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
    pub workflow_name: String,
    pub strict: bool,
    pub success: bool,
    pub started_at: chrono::DateTime<Utc>,
    pub finished_at: chrono::DateTime<Utc>,
    pub failed_steps: Vec<usize>,
    pub step_results: Vec<StepResult>,
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
    project_root: &'a Path,
}

impl<'a> WorkflowExecutor<'a> {
    pub fn new(project_root: &'a Path) -> Self {
        Self { project_root }
    }

    pub fn execute(
        &self,
        workflow: &ResolvedWorkflow,
        options: &ExecuteOptions,
        agent_scope: Option<&str>,
    ) -> Result<ExecutionResult> {
        let started_at = Utc::now();
        let mut success = true;
        let mut failed_steps = Vec::new();
        let mut step_results = Vec::with_capacity(workflow.steps.len());

        for (idx, step) in workflow.steps.iter().enumerate() {
            let step_index = idx + 1;
            match step {
                ResolvedStep::Prompt {
                    text, session_name, ..
                } => {
                    let started = std::time::Instant::now();
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

                    if let Err(e) = TmuxSession::send_text(session_name, text) {
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
                    run,
                    cwd,
                    timeout_secs,
                    fail_mode,
                    ..
                } => {
                    let started = std::time::Instant::now();
                    let cmd_result = run_shell_command(run, cwd, *timeout_secs);

                    match cmd_result {
                        Ok(outcome) => {
                            let failed = outcome.timed_out || outcome.exit_code.unwrap_or(1) != 0;
                            if failed {
                                let message = if outcome.timed_out {
                                    format!("command timed out after {}s: {}", timeout_secs, run)
                                } else {
                                    format!(
                                        "command failed (exit {}): {}",
                                        outcome.exit_code.unwrap_or(-1),
                                        run
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
            }
        }

        let result = ExecutionResult {
            workflow_name: workflow.name.clone(),
            strict: options.strict,
            success,
            started_at,
            finished_at: Utc::now(),
            failed_steps,
            step_results,
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

            let run_result = dispatch_single_hook(config, hook, payload);
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

fn dispatch_single_hook(
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
    let executor = WorkflowExecutor::new(&payload.project_root);

    let mut result = if let Some(workflow_name) = hook.workflow.as_deref() {
        let resolved = resolver.resolve(workflow_name, Some(&payload.agent_name), &options)?;
        executor.execute(&resolved, &options, Some(&payload.agent_name))?
    } else if let Some(cmd) = hook.run.as_deref() {
        let resolved = ResolvedWorkflow {
            name: format!("hook:agent_stop:{}", payload.agent_name),
            description: Some("Generated hook command".to_string()),
            steps: vec![ResolvedStep::Command {
                run: cmd.to_string(),
                cwd: payload.project_root.clone(),
                agent: Some(payload.agent_name.clone()),
                timeout_secs: hook.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS),
                fail_mode: hook.fail_mode.unwrap_or(WorkflowFailMode::Open),
            }],
        };
        executor.execute(&resolved, &options, Some(&payload.agent_name))?
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
            steps: vec![
                WorkflowStepConfig::Command {
                    run: "echo one".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    agent: None,
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Open),
                },
                WorkflowStepConfig::Command {
                    run: "exit 7".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    agent: None,
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Open),
                },
                WorkflowStepConfig::Command {
                    run: "echo three".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    agent: None,
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Open),
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
        let executor = WorkflowExecutor::new(&dir);
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
            steps: vec![
                WorkflowStepConfig::Command {
                    run: "echo one".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    agent: None,
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Closed),
                },
                WorkflowStepConfig::Command {
                    run: "exit 9".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    agent: None,
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Closed),
                },
                WorkflowStepConfig::Command {
                    run: "echo never".to_string(),
                    cwd: Some(WorkflowCommandCwd::Workspace),
                    agent: None,
                    timeout_secs: Some(30),
                    fail_mode: Some(WorkflowFailMode::Closed),
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
        let executor = WorkflowExecutor::new(&dir);
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
            steps: vec![WorkflowStepConfig::Command {
                run: "exit 4".to_string(),
                cwd: Some(WorkflowCommandCwd::Workspace),
                agent: None,
                timeout_secs: Some(30),
                fail_mode: Some(WorkflowFailMode::Open),
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
        let executor = WorkflowExecutor::new(&dir);
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
            steps: vec![WorkflowStepConfig::Command {
                run: "echo hook".to_string(),
                cwd: Some(WorkflowCommandCwd::Workspace),
                agent: None,
                timeout_secs: Some(30),
                fail_mode: Some(WorkflowFailMode::Open),
            }],
        };
        let hooks = vec![HookConfig {
            event: HookEvent::AgentStop,
            agent: Some("backend".to_string()),
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
}
