use crate::config::TuttiConfig;
use crate::error::{Result, TuttiError};
use crate::health;
use crate::health::{WaitCompletionSource, WaitFailureReason};
use crate::session::TmuxSession;
use crate::{budget, budget::BudgetGuardOutcome};
use serde::Serialize;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct SendExecutionResult {
    pub workspace: String,
    pub agent: String,
    pub waited: bool,
    pub completion_source: Option<String>,
    pub captured_output: Option<String>,
}

pub fn run(
    agent_ref: &str,
    prompt_parts: &[String],
    options: SendOptions,
) -> Result<SendExecutionResult> {
    crate::session::tmux::check_tmux()?;

    let prompt = assemble_prompt(prompt_parts)
        .ok_or_else(|| TuttiError::ConfigValidation("prompt cannot be empty".to_string()))?;
    let workspace_selector = agent_ref
        .split_once('/')
        .map(|(workspace, _)| workspace.to_string());
    let target = resolve_agent_ref(agent_ref)?;
    let workspace_name = target.workspace_name;
    let agent_name = target.agent_name;
    let runtime_name = target.runtime_name;
    let (workspace_config, _) = TuttiConfig::load(&target.project_root)?;
    let budget_outcome = budget::enforce_pre_exec(
        &workspace_config,
        &target.project_root,
        "send",
        Some(&agent_name),
    )?;
    print_budget_warnings(&budget_outcome);
    let session = TmuxSession::session_name(&workspace_name, &agent_name);

    if !TmuxSession::session_exists(&session) {
        if !options.auto_up {
            return Err(TuttiError::AgentNotRunning(agent_name.to_string()));
        }
        super::up::run(
            Some(agent_name.as_str()),
            workspace_selector.as_deref(),
            false,
            None,
            None,
        )?;
        if !TmuxSession::session_exists(&session) {
            return Err(TuttiError::AgentNotRunning(agent_name.to_string()));
        }
    }

    let capture_lines = options.output_lines.max(20);
    let before_output = if options.output {
        Some(TmuxSession::capture_pane(&session, capture_lines).unwrap_or_default())
    } else {
        None
    };

    TmuxSession::send_text(&session, &prompt)?;
    let mut completion_source = None;
    if options.wait {
        let outcome = health::wait_for_agent_idle(
            &runtime_name,
            &session,
            Duration::from_secs(options.timeout_secs.max(1)),
            Duration::from_secs(options.idle_stable_secs.max(1)),
        )?;
        if !outcome.is_completed() {
            return Err(match outcome.failure_reason {
                Some(WaitFailureReason::IdleTimeout) => TuttiError::ConfigValidation(format!(
                    "send_wait_failed: idle_timeout (agent='{}', timeout_secs={})",
                    agent_name,
                    options.timeout_secs.max(1)
                )),
                Some(WaitFailureReason::AuthFailed) => TuttiError::ConfigValidation(format!(
                    "send_wait_failed: auth_failed (agent='{}', detail='{}')",
                    agent_name,
                    outcome.detail.as_deref().unwrap_or("unknown")
                )),
                Some(WaitFailureReason::SessionExited) => {
                    TuttiError::AgentNotRunning(agent_name.to_string())
                }
                None => TuttiError::ConfigValidation("send_wait_failed: unknown".to_string()),
            });
        }
        if let Ok((config, _)) = TuttiConfig::load(&target.project_root) {
            let _ = health::probe_workspace(&config, &target.project_root, 200);
        }
        let source = match outcome.completion_source {
            Some(WaitCompletionSource::RuntimeSignal) => "runtime_signal",
            Some(WaitCompletionSource::HeuristicIdleStable) => "heuristic_idle_stable",
            None => "unknown",
        };
        completion_source = Some(source.to_string());
        println!("sent prompt to {agent_name} ({workspace_name}) and wait completed ({source})");
    } else {
        println!("sent prompt to {agent_name} ({workspace_name})");
    }

    let mut captured_output = None;
    if options.output {
        let after_output = TmuxSession::capture_pane(&session, capture_lines).unwrap_or_default();
        let delta = pane_delta(before_output.as_deref().unwrap_or(""), &after_output);
        if delta.trim().is_empty() {
            println!("\n(no new output captured)");
        } else {
            println!("\n{delta}");
        }
        captured_output = Some(delta);
    }

    Ok(SendExecutionResult {
        workspace: workspace_name,
        agent: agent_name,
        waited: options.wait,
        completion_source,
        captured_output,
    })
}

#[derive(Debug, Clone, Copy)]
pub struct SendOptions {
    pub auto_up: bool,
    pub wait: bool,
    pub timeout_secs: u64,
    pub idle_stable_secs: u64,
    pub output: bool,
    pub output_lines: u32,
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

fn pane_delta(before: &str, after: &str) -> String {
    let before_lines: Vec<&str> = before.lines().collect();
    let after_lines: Vec<&str> = after.lines().collect();

    if before_lines.is_empty() {
        return after.trim_end().to_string();
    }
    if after_lines.is_empty() {
        return String::new();
    }

    let max_overlap = before_lines.len().min(after_lines.len());
    let mut overlap = 0usize;
    for candidate in (0..=max_overlap).rev() {
        if before_lines[before_lines.len() - candidate..] == after_lines[..candidate] {
            overlap = candidate;
            break;
        }
    }

    after_lines[overlap..].join("\n").trim_end().to_string()
}

fn print_budget_warnings(outcome: &BudgetGuardOutcome) {
    for warning in &outcome.warnings {
        eprintln!("warn: {warning}");
    }
}

#[cfg(test)]
mod tests {
    use super::{assemble_prompt, pane_delta};

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

    #[test]
    fn pane_delta_returns_new_tail_after_overlap() {
        let before = "a\nb\nc";
        let after = "b\nc\nd\ne";
        assert_eq!(pane_delta(before, after), "d\ne");
    }

    #[test]
    fn pane_delta_returns_empty_when_no_change() {
        let before = "a\nb\nc";
        let after = "a\nb\nc";
        assert_eq!(pane_delta(before, after), "");
    }
}
