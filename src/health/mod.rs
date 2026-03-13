use crate::config::TuttiConfig;
use crate::error::Result;
use crate::runtime::{self, AgentStatus};
use crate::session::TmuxSession;
use crate::state::{self, ActivityState, AgentHealth, AuthState, ControlEvent};
use chrono::{DateTime, Utc};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::Path;
use std::time::{Duration, Instant};

const DEFAULT_CAPTURE_LINES: u32 = 200;

#[derive(Debug, Clone, Copy)]
pub enum WaitCompletionSource {
    RuntimeSignal,
    HeuristicIdleStable,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaitFailureReason {
    IdleTimeout,
    AuthFailed,
    SessionExited,
}

#[derive(Debug, Clone)]
pub struct WaitForIdleResult {
    pub completion_source: Option<WaitCompletionSource>,
    pub failure_reason: Option<WaitFailureReason>,
    pub detail: Option<String>,
}

impl WaitForIdleResult {
    pub fn completed(source: WaitCompletionSource) -> Self {
        Self {
            completion_source: Some(source),
            failure_reason: None,
            detail: None,
        }
    }

    pub fn failed(reason: WaitFailureReason, detail: Option<String>) -> Self {
        Self {
            completion_source: None,
            failure_reason: Some(reason),
            detail,
        }
    }

    pub fn is_completed(&self) -> bool {
        self.completion_source.is_some()
    }
}

pub fn probe_workspace(
    config: &TuttiConfig,
    project_root: &Path,
    lines: u32,
) -> Result<Vec<AgentHealth>> {
    let mut out = Vec::with_capacity(config.agents.len());
    let now = Utc::now();
    let capture_lines = lines.max(10);

    for agent in &config.agents {
        let runtime_name = agent
            .resolved_runtime(&config.defaults)
            .unwrap_or_else(|| "unknown".to_string());
        let session_name = TmuxSession::session_name(&config.workspace.name, &agent.name);
        let running = TmuxSession::session_exists(&session_name);
        let previous = state::load_agent_health(project_root, &agent.name)?;

        let mut health = AgentHealth {
            workspace: config.workspace.name.clone(),
            agent: agent.name.clone(),
            runtime: runtime_name.clone(),
            session_name: session_name.clone(),
            running,
            activity_state: if running {
                ActivityState::Unknown
            } else {
                ActivityState::Stopped
            },
            auth_state: if running {
                AuthState::Unknown
            } else {
                AuthState::Ok
            },
            last_output_change_at: previous.as_ref().and_then(|h| h.last_output_change_at),
            last_probe_at: now,
            reason: None,
            pane_hash: previous.as_ref().and_then(|h| h.pane_hash),
        };

        if running {
            match TmuxSession::capture_pane(&session_name, capture_lines) {
                Ok(output) => {
                    let pane_hash = hash_output(&output);
                    let changed = previous
                        .as_ref()
                        .and_then(|h| h.pane_hash)
                        .is_none_or(|h| h != pane_hash);
                    if changed {
                        health.last_output_change_at = Some(now);
                        health.activity_state = ActivityState::Working;
                    } else {
                        health.activity_state = ActivityState::Idle;
                    }
                    health.pane_hash = Some(pane_hash);

                    if let Some(adapter) = runtime::get_adapter(&runtime_name, None) {
                        if let Some(reason) = adapter.detect_auth_failure(&output) {
                            health.auth_state = AuthState::Failed;
                            health.reason = Some(reason);
                        } else {
                            health.auth_state = AuthState::Ok;
                        }
                    }
                }
                Err(e) => {
                    health.activity_state = ActivityState::Unknown;
                    health.auth_state = AuthState::Unknown;
                    health.reason = Some(e.to_string());
                }
            }
        }

        for event in transition_events(previous.as_ref(), &health, now) {
            let _ = state::append_control_event(project_root, &event);
        }
        state::save_agent_health(project_root, &health)?;
        out.push(health);
    }

    Ok(out)
}

pub fn wait_for_agent_idle(
    runtime_name: &str,
    session_name: &str,
    timeout: Duration,
    idle_stability: Duration,
) -> Result<WaitForIdleResult> {
    let adapter = runtime::get_adapter(runtime_name, None);
    let start = Instant::now();
    let mut saw_activity = false;
    let mut last_hash: Option<u64> = None;
    let mut idle_since: Option<Instant> = None;

    while start.elapsed() < timeout {
        if !TmuxSession::session_exists(session_name) {
            return Ok(WaitForIdleResult::failed(
                WaitFailureReason::SessionExited,
                Some("session exited".to_string()),
            ));
        }

        let output = TmuxSession::capture_pane(session_name, DEFAULT_CAPTURE_LINES)?;
        let pane_hash = hash_output(&output);
        let changed = last_hash.is_none_or(|h| h != pane_hash);
        let runtime_status = adapter.as_ref().map(|a| a.detect_status(&output));

        if let Some(adapter) = &adapter
            && let Some(reason) = adapter.detect_auth_failure(&output)
        {
            return Ok(WaitForIdleResult::failed(
                WaitFailureReason::AuthFailed,
                Some(reason),
            ));
        }

        if let Some(adapter) = &adapter
            && adapter.detect_completion_signal(&output).is_some()
            && saw_activity
        {
            return Ok(WaitForIdleResult::completed(
                WaitCompletionSource::RuntimeSignal,
            ));
        }

        if changed
            || runtime_status
                .as_ref()
                .is_some_and(|s| matches!(s, AgentStatus::Working))
        {
            saw_activity = true;
            idle_since = None;
        } else if saw_activity {
            if let Some(since) = idle_since {
                if since.elapsed() >= idle_stability {
                    return Ok(WaitForIdleResult::completed(
                        WaitCompletionSource::HeuristicIdleStable,
                    ));
                }
            } else {
                idle_since = Some(Instant::now());
            }
        }

        last_hash = Some(pane_hash);
        std::thread::sleep(Duration::from_secs(1));
    }

    Ok(WaitForIdleResult::failed(
        WaitFailureReason::IdleTimeout,
        Some("idle wait timed out".to_string()),
    ))
}

fn hash_output(output: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    output.hash(&mut hasher);
    hasher.finish()
}

fn transition_events(
    previous: Option<&AgentHealth>,
    current: &AgentHealth,
    now: DateTime<Utc>,
) -> Vec<ControlEvent> {
    let Some(previous) = previous else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let prefix = format!("probe-{}-{}", now.timestamp_millis(), current.agent);

    if previous.running != current.running {
        let event = if current.running {
            "agent.started"
        } else {
            "agent.stopped"
        };
        out.push(ControlEvent {
            event: event.to_string(),
            workspace: current.workspace.clone(),
            agent: Some(current.agent.clone()),
            timestamp: now,
            correlation_id: format!("{prefix}-{event}"),
            data: Some(serde_json::json!({
                "source": "probe",
                "runtime": current.runtime,
                "session_name": current.session_name
            })),
        });
    }

    if current.running && previous.activity_state != current.activity_state {
        let event = match current.activity_state {
            ActivityState::Working => Some("agent.working"),
            ActivityState::Idle => Some("agent.idle"),
            _ => None,
        };
        if let Some(event) = event {
            out.push(ControlEvent {
                event: event.to_string(),
                workspace: current.workspace.clone(),
                agent: Some(current.agent.clone()),
                timestamp: now,
                correlation_id: format!("{prefix}-{event}"),
                data: Some(serde_json::json!({
                    "source": "probe",
                    "from": previous.activity_state,
                    "to": current.activity_state
                })),
            });
        }
    }

    if current.running && previous.auth_state != current.auth_state {
        match current.auth_state {
            AuthState::Failed => out.push(ControlEvent {
                event: "agent.auth_failed".to_string(),
                workspace: current.workspace.clone(),
                agent: Some(current.agent.clone()),
                timestamp: now,
                correlation_id: format!("{prefix}-agent.auth_failed"),
                data: Some(serde_json::json!({
                    "source": "probe",
                    "reason": current.reason,
                    "runtime": current.runtime
                })),
            }),
            AuthState::Ok if matches!(previous.auth_state, AuthState::Failed) => {
                out.push(ControlEvent {
                    event: "agent.auth_recovered".to_string(),
                    workspace: current.workspace.clone(),
                    agent: Some(current.agent.clone()),
                    timestamp: now,
                    correlation_id: format!("{prefix}-agent.auth_recovered"),
                    data: Some(serde_json::json!({
                        "source": "probe",
                        "runtime": current.runtime
                    })),
                })
            }
            _ => {}
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_health(
        running: bool,
        activity_state: ActivityState,
        auth_state: AuthState,
        reason: Option<&str>,
    ) -> AgentHealth {
        AgentHealth {
            workspace: "ws".to_string(),
            agent: "backend".to_string(),
            runtime: "claude-code".to_string(),
            session_name: "tutti-ws-backend".to_string(),
            running,
            activity_state,
            auth_state,
            last_output_change_at: None,
            last_probe_at: Utc::now(),
            reason: reason.map(ToString::to_string),
            pane_hash: Some(123),
        }
    }

    #[test]
    fn transition_events_emits_activity_and_auth_changes() {
        let previous = sample_health(true, ActivityState::Working, AuthState::Ok, None);
        let current = sample_health(
            true,
            ActivityState::Idle,
            AuthState::Failed,
            Some("auth expired"),
        );
        let events = transition_events(Some(&previous), &current, Utc::now());
        assert!(events.iter().any(|e| e.event == "agent.idle"));
        assert!(events.iter().any(|e| e.event == "agent.auth_failed"));
    }

    #[test]
    fn transition_events_emits_running_changes() {
        let previous = sample_health(true, ActivityState::Idle, AuthState::Ok, None);
        let current = sample_health(false, ActivityState::Stopped, AuthState::Ok, None);
        let events = transition_events(Some(&previous), &current, Utc::now());
        assert!(events.iter().any(|e| e.event == "agent.stopped"));
    }
}
