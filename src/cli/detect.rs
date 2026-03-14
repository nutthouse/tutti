use crate::error::{Result, TuttiError};
use crate::runtime;
use crate::session::TmuxSession;
use serde::Serialize;

#[derive(Debug, Serialize)]
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
        .resolved_runtime(&resolved.config.defaults)
        .unwrap_or_else(|| "unknown".to_string());
    let session = TmuxSession::session_name(&resolved.workspace_name, &resolved.agent_name);

    if !TmuxSession::session_exists(&session) {
        return Err(TuttiError::AgentNotRunning(resolved.agent_name.clone()));
    }

    let output = TmuxSession::capture_pane(&session, lines.max(20))?;
    let diagnostics = runtime::diagnose_output(&runtime_name, &output, None)
        .ok_or_else(|| TuttiError::RuntimeUnknown(runtime_name.clone()))?;

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

    println!(
        "{} / {} ({})",
        payload.workspace, payload.agent, payload.runtime
    );
    println!("session: {}", payload.session);
    println!("status: {} ({:.2})", payload.status, payload.confidence);
    println!(
        "signals: auth={} rate_limit={} provider_down={} completion={}",
        payload.auth_match.as_deref().unwrap_or("--"),
        payload.rate_limit_match.as_deref().unwrap_or("--"),
        payload.provider_down_match.as_deref().unwrap_or("--"),
        payload.completion_match.as_deref().unwrap_or("--"),
    );
    if payload.matched_patterns.is_empty() {
        println!("matches: --");
    } else {
        println!("matches:");
        for matched in payload.matched_patterns {
            println!("  - {matched}");
        }
    }

    Ok(())
}
