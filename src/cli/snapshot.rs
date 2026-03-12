use crate::config::TuttiConfig;
use crate::runtime::{self, AgentStatus};
use crate::session::TmuxSession;
use crate::state;
use colored::Colorize;
use std::path::Path;

/// Shared runtime snapshot for an agent.
/// This is the machine-oriented model used by status/watch/switch-style UIs.
pub struct AgentSnapshot {
    pub workspace_name: String,
    pub agent_name: String,
    pub runtime: String,
    /// ANSI-colored status for display.
    pub status_display: String,
    /// Plain status string for persistence/logic.
    pub status_raw: String,
    /// Display-ready session field (session name or "—" when stopped).
    pub session_name: String,
    pub running: bool,
    /// Present only when tail was requested for this snapshot.
    pub tail_lines: Option<Vec<String>>,
    pub tail_error: Option<String>,
}

/// Gather snapshots for all agents in a workspace config.
pub fn gather_workspace_snapshots(config: &TuttiConfig, project_root: &Path) -> Vec<AgentSnapshot> {
    gather_workspace_snapshots_with_selected_tail(config, project_root, None, 0)
}

/// Gather snapshots for all agents, optionally including a recent tail for one selected agent.
pub fn gather_workspace_snapshots_with_selected_tail(
    config: &TuttiConfig,
    project_root: &Path,
    selected_agent: Option<&str>,
    tail_lines: u32,
) -> Vec<AgentSnapshot> {
    let mut snapshots = Vec::new();

    for agent in &config.agents {
        let runtime_name = agent
            .resolved_runtime(&config.defaults)
            .unwrap_or_else(|| "—".to_string());

        let session = TmuxSession::session_name(&config.workspace.name, &agent.name);
        let running = TmuxSession::session_exists(&session);

        let detected = if running {
            Some(detect_status(
                &runtime_name,
                &session,
                project_root,
                &agent.name,
            ))
        } else {
            None
        };

        let include_tail = selected_agent.is_some_and(|name| name == agent.name);
        let (tail, tail_error) = if include_tail && running && tail_lines > 0 {
            match TmuxSession::capture_pane(&session, tail_lines) {
                Ok(output) => (Some(extract_recent_lines(&output, tail_lines)), None),
                Err(_) => (
                    Some(Vec::new()),
                    Some("(could not read output)".to_string()),
                ),
            }
        } else {
            (None, None)
        };

        snapshots.push(build_snapshot(
            &config.workspace.name,
            &agent.name,
            runtime_name,
            session,
            running,
            detected,
            tail,
            tail_error,
        ));
    }

    snapshots
}

fn build_snapshot(
    workspace_name: &str,
    agent_name: &str,
    runtime: String,
    session: String,
    running: bool,
    detected_status: Option<AgentStatus>,
    tail_lines: Option<Vec<String>>,
    tail_error: Option<String>,
) -> AgentSnapshot {
    if !running {
        return AgentSnapshot {
            workspace_name: workspace_name.to_string(),
            agent_name: agent_name.to_string(),
            runtime,
            status_display: "Stopped".dimmed().to_string(),
            status_raw: "Stopped".to_string(),
            session_name: "—".to_string(),
            running: false,
            tail_lines: None,
            tail_error: None,
        };
    }

    let status = detected_status.unwrap_or(AgentStatus::Unknown);

    AgentSnapshot {
        workspace_name: workspace_name.to_string(),
        agent_name: agent_name.to_string(),
        runtime,
        status_display: format_status(&status),
        status_raw: status.to_string(),
        session_name: session,
        running: true,
        tail_lines,
        tail_error,
    }
}

fn detect_status(
    runtime_name: &str,
    session: &str,
    project_root: &Path,
    agent_name: &str,
) -> AgentStatus {
    if let Some(adapter) = runtime::get_adapter(runtime_name, None) {
        match TmuxSession::capture_pane(session, 50) {
            Ok(output) => {
                let status = adapter.detect_status(&output);
                if let AgentStatus::AuthFailed(ref reason) = status {
                    let _ = state::save_emergency_state(project_root, agent_name, &output, reason);
                }
                status
            }
            Err(_) => AgentStatus::Unknown,
        }
    } else {
        AgentStatus::Unknown
    }
}

fn format_status(status: &AgentStatus) -> String {
    match status {
        AgentStatus::Working => "Working".green().to_string(),
        AgentStatus::Idle => "Idle".yellow().to_string(),
        AgentStatus::Errored => "Errored".red().to_string(),
        AgentStatus::AuthFailed(msg) => format!("{} ({})", "Auth Failed".red().bold(), msg),
        AgentStatus::Unknown => "Unknown".dimmed().to_string(),
        AgentStatus::Stopped => "Stopped".dimmed().to_string(),
    }
}

fn extract_recent_lines(output: &str, lines: u32) -> Vec<String> {
    output
        .lines()
        .rev()
        .take(lines as usize)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .map(|s| s.to_string())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_snapshot_running_uses_detected_status_and_session() {
        let snapshot = build_snapshot(
            "ws",
            "backend",
            "claude-code".to_string(),
            "tutti-ws-backend".to_string(),
            true,
            Some(AgentStatus::Working),
            Some(vec!["line".to_string()]),
            None,
        );

        assert_eq!(snapshot.workspace_name, "ws");
        assert_eq!(snapshot.agent_name, "backend");
        assert_eq!(snapshot.status_raw, "Working");
        assert_eq!(snapshot.session_name, "tutti-ws-backend");
        assert!(snapshot.running);
        assert_eq!(snapshot.tail_lines.unwrap(), vec!["line".to_string()]);
    }

    #[test]
    fn build_snapshot_stopped_sets_stopped_defaults() {
        let snapshot = build_snapshot(
            "ws",
            "frontend",
            "codex".to_string(),
            "tutti-ws-frontend".to_string(),
            false,
            Some(AgentStatus::Working),
            Some(vec!["ignored".to_string()]),
            Some("ignored".to_string()),
        );

        assert_eq!(snapshot.workspace_name, "ws");
        assert_eq!(snapshot.agent_name, "frontend");
        assert_eq!(snapshot.status_raw, "Stopped");
        assert_eq!(snapshot.session_name, "—");
        assert!(!snapshot.running);
        assert!(snapshot.tail_lines.is_none());
        assert!(snapshot.tail_error.is_none());
    }
}
