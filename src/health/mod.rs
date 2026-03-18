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
const RUNTIME_SIGNAL_FALLBACK_GRACE_MULTIPLIER: u32 = 2;
const RATE_LIMIT_REASON_PREFIX: &str = "rate_limit:";
const PROVIDER_DOWN_REASON_PREFIX: &str = "provider_down:";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoveryTrigger {
    AuthFailed,
    RateLimited,
    ProviderDown,
}

impl RecoveryTrigger {
    pub fn as_str(self) -> &'static str {
        match self {
            RecoveryTrigger::AuthFailed => "auth_failed",
            RecoveryTrigger::RateLimited => "rate_limited",
            RecoveryTrigger::ProviderDown => "provider_down",
        }
    }
}

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
        Self::completed_with_detail(source, None)
    }

    pub fn completed_with_detail(source: WaitCompletionSource, detail: Option<String>) -> Self {
        Self {
            completion_source: Some(source),
            failure_reason: None,
            detail,
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
                        } else if let Some(reason) = adapter.detect_rate_limit(&output) {
                            health.auth_state = AuthState::Ok;
                            health.reason = Some(format!("{RATE_LIMIT_REASON_PREFIX} {reason}"));
                        } else if let Some(reason) = adapter.detect_provider_down(&output) {
                            health.auth_state = AuthState::Ok;
                            health.reason = Some(format!("{PROVIDER_DOWN_REASON_PREFIX} {reason}"));
                        } else {
                            health.auth_state = AuthState::Ok;
                            health.reason = None;
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

pub fn recovery_trigger(health: &AgentHealth) -> Option<RecoveryTrigger> {
    if !health.running {
        return None;
    }
    if matches!(health.auth_state, AuthState::Failed) {
        return Some(RecoveryTrigger::AuthFailed);
    }
    let reason = health.reason.as_deref()?.trim().to_ascii_lowercase();
    if reason.starts_with(RATE_LIMIT_REASON_PREFIX) {
        return Some(RecoveryTrigger::RateLimited);
    }
    if reason.starts_with(PROVIDER_DOWN_REASON_PREFIX) {
        return Some(RecoveryTrigger::ProviderDown);
    }
    None
}

pub fn wait_for_agent_idle(
    runtime_name: &str,
    session_name: &str,
    timeout: Duration,
    idle_stability: Duration,
) -> Result<WaitForIdleResult> {
    let adapter = runtime::get_adapter(runtime_name, None);
    let runtime_prefers_signal = adapter
        .as_ref()
        .is_some_and(|adapter| adapter.supports_completion_signal());
    let start = Instant::now();
    let mut saw_activity = false;
    let mut last_hash: Option<u64> = None;
    let mut idle_since: Option<Instant> = None;
    let mut runtime_fallback_since: Option<Instant> = None;
    let runtime_fallback_grace = idle_stability
        .checked_mul(RUNTIME_SIGNAL_FALLBACK_GRACE_MULTIPLIER)
        .unwrap_or(idle_stability);

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
            runtime_fallback_since = None;
        } else if saw_activity {
            if let Some(since) = idle_since {
                if since.elapsed() >= idle_stability {
                    if runtime_prefers_signal {
                        if let Some(fallback_since) = runtime_fallback_since {
                            if fallback_since.elapsed() >= runtime_fallback_grace {
                                return Ok(WaitForIdleResult::completed_with_detail(
                                    WaitCompletionSource::HeuristicIdleStable,
                                    Some(
                                        "runtime_signal_not_observed_after_activity_fallback"
                                            .to_string(),
                                    ),
                                ));
                            }
                        } else {
                            runtime_fallback_since = Some(Instant::now());
                        }
                    } else {
                        return Ok(WaitForIdleResult::completed(
                            WaitCompletionSource::HeuristicIdleStable,
                        ));
                    }
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
    strip_status_bar_noise(output).hash(&mut hasher);
    hasher.finish()
}

fn strip_status_bar_noise(output: &str) -> String {
    output
        .lines()
        .filter(|line| !is_status_bar_noise(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn is_status_bar_noise(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return false;
    }

    let lower = trimmed.to_ascii_lowercase();
    let looks_like_runtime_footer = (lower.contains("esc to interrupt")
        || lower.contains("ctrl+c to stop")
        || lower.contains("shift+tab")
        || lower.contains("enter to submit"))
        && (lower.contains("tokens")
            || lower.contains("context")
            || lower.contains("model")
            || lower.contains('%'));
    if looks_like_runtime_footer {
        return true;
    }

    let chars = trimmed.chars().count();
    if chars == 0 {
        return false;
    }
    let box_chars = trimmed
        .chars()
        .filter(|ch| {
            matches!(
                ch,
                '│' | '─'
                    | '╭'
                    | '╮'
                    | '╰'
                    | '╯'
                    | '┌'
                    | '┐'
                    | '└'
                    | '┘'
                    | '├'
                    | '┤'
                    | '┬'
                    | '┴'
                    | '┼'
                    | '█'
                    | '▁'
                    | '▂'
                    | '▃'
                    | '▄'
                    | '▅'
                    | '▆'
                    | '▇'
                    | '▉'
                    | '▊'
                    | '▋'
                    | '▌'
                    | '▍'
                    | '▎'
                    | '▏'
            )
        })
        .count();
    chars >= 8 && box_chars * 2 >= chars
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

    let previous_trigger = recovery_trigger(previous);
    let current_trigger = recovery_trigger(current);
    let previous_non_auth = previous_trigger.and_then(|t| match t {
        RecoveryTrigger::RateLimited | RecoveryTrigger::ProviderDown => Some(t),
        RecoveryTrigger::AuthFailed => None,
    });
    let current_non_auth = current_trigger.and_then(|t| match t {
        RecoveryTrigger::RateLimited | RecoveryTrigger::ProviderDown => Some(t),
        RecoveryTrigger::AuthFailed => None,
    });

    if current.running && previous_non_auth != current_non_auth {
        if let Some(trigger) = current_non_auth {
            let event = match trigger {
                RecoveryTrigger::RateLimited => "agent.rate_limited",
                RecoveryTrigger::ProviderDown => "agent.provider_down",
                RecoveryTrigger::AuthFailed => unreachable!("filtered above"),
            };
            out.push(ControlEvent {
                event: event.to_string(),
                workspace: current.workspace.clone(),
                agent: Some(current.agent.clone()),
                timestamp: now,
                correlation_id: format!("{prefix}-{event}"),
                data: Some(serde_json::json!({
                    "source": "probe",
                    "reason": current.reason,
                    "runtime": current.runtime
                })),
            });
        }

        if let Some(previous_trigger) = previous_non_auth
            && current_non_auth.is_none()
        {
            out.push(ControlEvent {
                event: "agent.provider_recovered".to_string(),
                workspace: current.workspace.clone(),
                agent: Some(current.agent.clone()),
                timestamp: now,
                correlation_id: format!("{prefix}-agent.provider_recovered"),
                data: Some(serde_json::json!({
                    "source": "probe",
                    "from": previous_trigger.as_str(),
                    "runtime": current.runtime
                })),
            });
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

    #[test]
    fn recovery_trigger_detects_rate_limit_and_provider_down_reason_prefixes() {
        let rate = sample_health(
            true,
            ActivityState::Idle,
            AuthState::Ok,
            Some("rate_limit: too many requests"),
        );
        assert_eq!(recovery_trigger(&rate), Some(RecoveryTrigger::RateLimited));

        let provider = sample_health(
            true,
            ActivityState::Idle,
            AuthState::Ok,
            Some("provider_down: service unavailable"),
        );
        assert_eq!(
            recovery_trigger(&provider),
            Some(RecoveryTrigger::ProviderDown)
        );
    }

    #[test]
    fn transition_events_emits_rate_limited_and_provider_recovered() {
        let previous = sample_health(
            true,
            ActivityState::Idle,
            AuthState::Ok,
            Some("rate_limit: too many requests"),
        );
        let current = sample_health(true, ActivityState::Idle, AuthState::Ok, None);
        let events = transition_events(Some(&previous), &current, Utc::now());
        assert!(events.iter().any(|e| e.event == "agent.provider_recovered"));
    }

    #[test]
    fn hash_output_ignores_status_bar_redraw_noise() {
        let base = "Implementing first slice\nDone.\n";
        let footer_a = concat!(
            "╭────────────────────────────────────────────────────────────╮\n",
            "│ Model: GPT-5 | Context: 82% | 1200 tokens | Esc to interrupt │\n",
            "╰────────────────────────────────────────────────────────────╯"
        );
        let footer_b = concat!(
            "╭────────────────────────────────────────────────────────────╮\n",
            "│ Model: GPT-5 | Context: 79% | 1320 tokens | Esc to interrupt │\n",
            "╰────────────────────────────────────────────────────────────╯"
        );

        let output_a = format!("{base}{footer_a}");
        let output_b = format!("{base}{footer_b}");

        assert_eq!(strip_status_bar_noise(&output_a), base.trim_end());
        assert_eq!(strip_status_bar_noise(&output_b), base.trim_end());
        assert_eq!(hash_output(&output_a), hash_output(&output_b));
        assert_ne!(
            hash_output(&output_a),
            hash_output("Implementing first slice\nStill working.\n")
        );
    }

    #[test]
    fn is_status_bar_noise_detects_runtime_footer_variants() {
        assert!(is_status_bar_noise(
            "Model: GPT-5 | Context: 58% | 892 tokens | Ctrl+C to stop"
        ));
        assert!(is_status_bar_noise(
            "claude-opus | 73% context | Shift+Tab for history"
        ));
        assert!(is_status_bar_noise(
            "Model: codex | 1200 tokens | Enter to submit"
        ));
        assert!(is_status_bar_noise("▁▂▃▄▅▆▇█▇▆▅▄▃▂▁"));
    }

    #[test]
    fn is_status_bar_noise_does_not_match_regular_pane_content() {
        assert!(!is_status_bar_noise(
            "Model evaluation finished at 82% branch coverage"
        ));
        assert!(!is_status_bar_noise(
            "Esc to interrupt the deploy if the health check fails"
        ));
        assert!(!is_status_bar_noise(
            "enter to submit the form once you finish reviewing payload.rs"
        ));
        assert!(!is_status_bar_noise("Build status: [########] 8/8 tasks complete"));
    }

    #[test]
    fn strip_status_bar_noise_preserves_non_footer_lines() {
        let output = concat!(
            "Plan next change\n",
            "\n",
            "Esc to interrupt the deploy if the health check fails\n",
            "Build status: [########] 8/8 tasks complete\n"
        );

        assert_eq!(
            strip_status_bar_noise(output),
            output.trim_end_matches('\n')
        );
    }
}
