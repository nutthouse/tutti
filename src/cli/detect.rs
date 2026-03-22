use crate::error::{Result, TuttiError};
use crate::runtime;
use crate::session::TmuxSession;
use serde::Serialize;

#[derive(Debug, Serialize, PartialEq)]
struct DetectOutput {
    workspace: String,
    agent: String,
    runtime: String,
    session: String,
    status: String,
    confidence: f32,
    matched_patterns: Vec<String>,
    auth_match: Option<String>,
    rate_limit_match: Option<String>,
    provider_down_match: Option<String>,
    completion_match: Option<String>,
}

pub fn run(agent_ref: &str, lines: u32, json: bool) -> Result<()> {
    let resolved = super::agent_ref::resolve(agent_ref)?;
    let agent = resolved.agent_config()?;
    let runtime_name = agent
        .resolved_runtime(&resolved.config.defaults, &resolved.config.roles)
        .ok_or_else(|| {
            TuttiError::ConfigValidation(format!(
                "agent '{}' has no runtime — set 'runtime' on the agent, assign a 'role' \
                 with a [roles] mapping, or set 'defaults.runtime' in tutti.toml",
                resolved.agent_name
            ))
        })?;
    let session = TmuxSession::session_name(&resolved.workspace_name, &resolved.agent_name);

    ensure_session_running(&session, &resolved.agent_name)?;

    let output = TmuxSession::capture_pane(&session, lines.max(20))?;
    let diagnostics = runtime::diagnose_output(&runtime_name, &output, None)?;

    let payload = DetectOutput {
        workspace: resolved.workspace_name.clone(),
        agent: resolved.agent_name.clone(),
        runtime: runtime_name,
        session,
        status: diagnostics.status.to_string(),
        confidence: diagnostics.confidence,
        matched_patterns: diagnostics.matched_patterns.clone(),
        auth_match: diagnostics.auth_match.clone(),
        rate_limit_match: diagnostics.rate_limit_match.clone(),
        provider_down_match: diagnostics.provider_down_match.clone(),
        completion_match: diagnostics.completion_match.clone(),
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&payload)?);
        return Ok(());
    }

    println!("{}", render_human_output(&payload));

    Ok(())
}

fn ensure_session_running(session: &str, agent_name: &str) -> Result<()> {
    if !TmuxSession::session_exists(session) {
        return Err(TuttiError::AgentNotRunning(agent_name.to_string()));
    }
    Ok(())
}

fn render_human_output(payload: &DetectOutput) -> String {
    let mut lines = vec![
        format!(
            "{} / {} ({})",
            payload.workspace, payload.agent, payload.runtime
        ),
        format!("session: {}", payload.session),
        format!("status: {} ({:.2})", payload.status, payload.confidence),
        format!(
            "signals: auth={} rate_limit={} provider_down={} completion={}",
            payload.auth_match.as_deref().unwrap_or("--"),
            payload.rate_limit_match.as_deref().unwrap_or("--"),
            payload.provider_down_match.as_deref().unwrap_or("--"),
            payload.completion_match.as_deref().unwrap_or("--"),
        ),
    ];

    if payload.matched_patterns.is_empty() {
        lines.push("matches: --".to_string());
    } else {
        lines.push("matches:".to_string());
        lines.extend(
            payload
                .matched_patterns
                .iter()
                .map(|matched| format!("  - {matched}")),
        );
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{DetectOutput, ensure_session_running, render_human_output};
    use crate::error::TuttiError;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn detect_output_serializes_to_json() {
        let output = DetectOutput {
            workspace: "employee-portal".to_string(),
            agent: "frontend".to_string(),
            runtime: "claude-code".to_string(),
            session: "tutti-employee-portal-frontend".to_string(),
            status: "Idle".to_string(),
            confidence: 0.95,
            matched_patterns: vec!["idle:What would you like to do?".to_string()],
            auth_match: None,
            rate_limit_match: None,
            provider_down_match: None,
            completion_match: Some("What would you like to do?".to_string()),
        };

        let json = serde_json::to_string(&output).unwrap();
        assert!(json.contains("\"confidence\":0.95"));
        assert!(json.contains("\"agent\":\"frontend\""));
    }

    #[test]
    fn render_human_output_lists_signals_and_matches() {
        let output = DetectOutput {
            workspace: "employee-portal".to_string(),
            agent: "ops".to_string(),
            runtime: "codex".to_string(),
            session: "tutti-employee-portal-ops".to_string(),
            status: "Working".to_string(),
            confidence: 0.78,
            matched_patterns: vec![
                "working:Generating".to_string(),
                "working:Running".to_string(),
            ],
            auth_match: None,
            rate_limit_match: Some("rate_limit_exceeded".to_string()),
            provider_down_match: None,
            completion_match: None,
        };

        let rendered = render_human_output(&output);
        assert!(rendered.contains("employee-portal / ops (codex)"));
        assert!(rendered.contains("status: Working (0.78)"));
        assert!(rendered.contains("rate_limit=rate_limit_exceeded"));
        assert!(rendered.contains("  - working:Generating"));
    }

    #[test]
    fn ensure_session_running_returns_agent_not_running_error() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let missing_session = format!("tutti-test-nonexistent-{nanos}");
        let err = ensure_session_running(&missing_session, "etl").unwrap_err();
        assert!(matches!(err, TuttiError::AgentNotRunning(agent) if agent == "etl"));
    }
}
