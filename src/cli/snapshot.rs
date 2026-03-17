use crate::config::TuttiConfig;
use crate::health::{self, HealthState};
use crate::runtime::{self, AgentStatus};
use crate::session::TmuxSession;
use crate::state;
use chrono::Utc;
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
    /// Operator-facing reason for unhealthy states.
    pub reason: Option<String>,
    /// Relative age since last output change.
    pub last_change_display: String,
    /// Display-ready session field (session name or "—" when stopped).
    pub session_name: String,
    pub running: bool,
    pub ctx_pct: Option<u8>,
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

        let (detected, ctx_pct) = if running {
            let (status, ctx_pct) =
                detect_status(&runtime_name, &session, project_root, &agent.name);
            (Some(status), ctx_pct)
        } else {
            (None, None)
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
        let persisted_health = state::load_agent_health(project_root, &agent.name)
            .ok()
            .flatten();

        snapshots.push(build_snapshot(SnapshotBuildArgs {
            workspace_name: &config.workspace.name,
            agent_name: &agent.name,
            runtime: runtime_name,
            session,
            running,
            detected_status: detected,
            persisted_health,
            ctx_pct,
            tail_lines: tail,
            tail_error,
        }));
    }

    snapshots
}

struct SnapshotBuildArgs<'a> {
    workspace_name: &'a str,
    agent_name: &'a str,
    runtime: String,
    session: String,
    running: bool,
    detected_status: Option<AgentStatus>,
    persisted_health: Option<state::AgentHealth>,
    ctx_pct: Option<u8>,
    tail_lines: Option<Vec<String>>,
    tail_error: Option<String>,
}

fn build_snapshot(args: SnapshotBuildArgs<'_>) -> AgentSnapshot {
    if !args.running {
        return AgentSnapshot {
            workspace_name: args.workspace_name.to_string(),
            agent_name: args.agent_name.to_string(),
            runtime: args.runtime,
            status_display: format_health_state(HealthState::Stopped),
            status_raw: HealthState::Stopped.as_str().to_string(),
            reason: None,
            last_change_display: "--".to_string(),
            session_name: "—".to_string(),
            running: false,
            ctx_pct: None,
            tail_lines: None,
            tail_error: None,
        };
    }

    let now = Utc::now();
    let persisted_health = args
        .persisted_health
        .filter(|health| health.running == args.running);
    let (status_raw, status_display, reason, last_change_display) =
        if let Some(health_record) = persisted_health.as_ref() {
            let summary = health::summarize(health_record, now);
            (
                summary.state.as_str().to_string(),
                format_health_state(summary.state),
                summary.reason,
                health::format_last_change(summary.last_change_at, now),
            )
        } else {
            let status = args.detected_status.unwrap_or(AgentStatus::Unknown);
            let (raw, reason) = fallback_status(&status);
            (
                raw.to_string(),
                format_health_label(raw),
                reason,
                "--".to_string(),
            )
        };

    AgentSnapshot {
        workspace_name: args.workspace_name.to_string(),
        agent_name: args.agent_name.to_string(),
        runtime: args.runtime,
        status_display,
        status_raw,
        reason,
        last_change_display,
        session_name: args.session,
        running: true,
        ctx_pct: args.ctx_pct,
        tail_lines: args.tail_lines,
        tail_error: args.tail_error,
    }
}

fn fallback_status(status: &AgentStatus) -> (&'static str, Option<String>) {
    match status {
        AgentStatus::Working => (HealthState::Working.as_str(), None),
        AgentStatus::Idle => (HealthState::Idle.as_str(), None),
        AgentStatus::AuthFailed(reason) => (HealthState::AuthFailed.as_str(), Some(reason.clone())),
        AgentStatus::Unknown => (HealthState::Unknown.as_str(), None),
    }
}

fn detect_status(
    runtime_name: &str,
    session: &str,
    project_root: &Path,
    agent_name: &str,
) -> (AgentStatus, Option<u8>) {
    if let Some(adapter) = runtime::get_adapter(runtime_name, None) {
        match TmuxSession::capture_pane(session, 50) {
            Ok(output) => {
                let status = adapter.detect_status(&output);
                let ctx_pct = extract_context_pct_for_runtime(runtime_name, &output);
                if let AgentStatus::AuthFailed(ref reason) = status {
                    let _ = state::save_emergency_state(project_root, agent_name, &output, reason);
                }
                (status, ctx_pct)
            }
            Err(_) => (AgentStatus::Unknown, None),
        }
    } else {
        (AgentStatus::Unknown, None)
    }
}

fn extract_context_pct_for_runtime(runtime_name: &str, output: &str) -> Option<u8> {
    let runtime = runtime_name.to_ascii_lowercase();
    if runtime.contains("claude") {
        return extract_context_pct_with_hints(
            output,
            &["context", "ctx", "window", "compact", "token", "tokens"],
        );
    }
    if runtime.contains("codex") {
        return extract_context_pct_with_hints(output, &["context", "ctx", "window", "tokens"]);
    }

    // Unknown runtimes: keep a generic fallback so watch remains useful.
    extract_context_pct_with_hints(output, &["context", "ctx", "window", "tokens"])
        .or_else(|| extract_any_percent(output))
}

fn extract_context_pct_with_hints(output: &str, hints: &[&str]) -> Option<u8> {
    for line in output.lines().rev().take(40) {
        let lower = line.to_lowercase();
        if !hints.iter().any(|hint| lower.contains(hint)) {
            continue;
        }
        if let Some(pct) = parse_percent_in_line(&lower) {
            return Some(pct);
        }
    }
    None
}

fn extract_any_percent(output: &str) -> Option<u8> {
    let mut fallback: Option<u8> = None;
    for line in output.lines().rev().take(30) {
        let lower = line.to_lowercase();
        let pct = parse_percent_in_line(&lower);
        if pct.is_none() {
            continue;
        }
        let pct = pct.unwrap_or(0);
        let has_ctx_hint = lower.contains("context")
            || lower.contains("ctx")
            || lower.contains("window")
            || lower.contains("tokens");
        if has_ctx_hint {
            return Some(pct);
        }
        if fallback.is_none() {
            fallback = Some(pct);
        }
    }
    fallback
}

fn parse_percent_in_line(line: &str) -> Option<u8> {
    let bytes = line.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let mut start = i;
            while start > 0 && bytes[start - 1].is_ascii_digit() {
                start -= 1;
            }
            if start == i {
                i += 1;
                continue;
            }
            if let Ok(n) = line[start..i].parse::<u16>()
                && n <= 100
            {
                return Some(n as u8);
            }
        }
        i += 1;
    }
    None
}

fn format_health_state(state: HealthState) -> String {
    format_health_label(state.as_str())
}

fn format_health_label(label: &str) -> String {
    match label {
        "working" => label.green().to_string(),
        "idle" => label.yellow().to_string(),
        "stalled" | "rate_limited" => label.yellow().bold().to_string(),
        "auth_failed" | "provider_down" => label.red().bold().to_string(),
        "stopped" | "unknown" => label.dimmed().to_string(),
        _ => label.normal().to_string(),
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
        let snapshot = build_snapshot(SnapshotBuildArgs {
            workspace_name: "ws",
            agent_name: "backend",
            runtime: "claude-code".to_string(),
            session: "tutti-ws-backend".to_string(),
            running: true,
            detected_status: Some(AgentStatus::Working),
            persisted_health: None,
            ctx_pct: Some(67),
            tail_lines: Some(vec!["line".to_string()]),
            tail_error: None,
        });

        assert_eq!(snapshot.workspace_name, "ws");
        assert_eq!(snapshot.agent_name, "backend");
        assert_eq!(snapshot.status_raw, "working");
        assert!(snapshot.reason.is_none());
        assert_eq!(snapshot.last_change_display, "--");
        assert_eq!(snapshot.session_name, "tutti-ws-backend");
        assert!(snapshot.running);
        assert_eq!(snapshot.ctx_pct, Some(67));
        assert_eq!(snapshot.tail_lines.unwrap(), vec!["line".to_string()]);
    }

    #[test]
    fn build_snapshot_stopped_sets_stopped_defaults() {
        let snapshot = build_snapshot(SnapshotBuildArgs {
            workspace_name: "ws",
            agent_name: "frontend",
            runtime: "codex".to_string(),
            session: "tutti-ws-frontend".to_string(),
            running: false,
            detected_status: Some(AgentStatus::Working),
            persisted_health: None,
            ctx_pct: Some(52),
            tail_lines: Some(vec!["ignored".to_string()]),
            tail_error: Some("ignored".to_string()),
        });

        assert_eq!(snapshot.workspace_name, "ws");
        assert_eq!(snapshot.agent_name, "frontend");
        assert_eq!(snapshot.status_raw, "stopped");
        assert_eq!(snapshot.session_name, "—");
        assert!(!snapshot.running);
        assert!(snapshot.ctx_pct.is_none());
        assert!(snapshot.tail_lines.is_none());
        assert!(snapshot.tail_error.is_none());
    }

    #[test]
    fn build_snapshot_prefers_health_classification_details() {
        let now = Utc::now();
        let snapshot = build_snapshot(SnapshotBuildArgs {
            workspace_name: "ws",
            agent_name: "backend",
            runtime: "claude-code".to_string(),
            session: "tutti-ws-backend".to_string(),
            running: true,
            detected_status: Some(AgentStatus::Working),
            persisted_health: Some(state::AgentHealth {
                workspace: "ws".to_string(),
                agent: "backend".to_string(),
                runtime: "claude-code".to_string(),
                session_name: "tutti-ws-backend".to_string(),
                running: true,
                activity_state: state::ActivityState::Idle,
                auth_state: state::AuthState::Ok,
                last_output_change_at: Some(now - chrono::Duration::minutes(6)),
                last_probe_at: now,
                reason: None,
                pane_hash: Some(1),
            }),
            ctx_pct: None,
            tail_lines: None,
            tail_error: None,
        });

        assert_eq!(snapshot.status_raw, "stalled");
        assert_eq!(
            snapshot.reason.as_deref(),
            Some("no output change detected")
        );
        assert_eq!(snapshot.last_change_display, "6m ago");
    }

    #[test]
    fn parse_percent_in_line_extracts_valid_pct() {
        assert_eq!(parse_percent_in_line("ctx 82%"), Some(82));
        assert_eq!(parse_percent_in_line("context=101%"), None);
        assert_eq!(parse_percent_in_line("no percent"), None);
    }

    #[test]
    fn claude_ctx_prefers_context_hint() {
        let output = "build progress: 95%\ncontext window 71%\n";
        assert_eq!(
            extract_context_pct_for_runtime("claude-code", output),
            Some(71)
        );
    }

    #[test]
    fn codex_ctx_ignores_unrelated_percent_without_hint() {
        let output = "build progress: 95%\nall good\n";
        assert_eq!(extract_context_pct_for_runtime("codex", output), None);
    }

    #[test]
    fn unknown_runtime_falls_back_to_any_percent() {
        let output = "build progress: 95%\n";
        assert_eq!(extract_context_pct_for_runtime("custom", output), Some(95));
    }
}
