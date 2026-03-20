use crate::cli::snapshot;
use crate::config::{GlobalConfig, ResilienceConfig, TuttiConfig};
use crate::error::{Result, TuttiError};
use crate::health;
use crate::scheduler;
use crate::session::TmuxSession;
use crate::state;
use crate::state::ControlEvent;
use crate::webhook;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response, Server, StatusCode};

#[derive(Clone)]
struct WorkspaceTarget {
    name: String,
    project_root: PathBuf,
    config: TuttiConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct IdempotencyEntry {
    action: String,
    response: Value,
    created_at: DateTime<Utc>,
}

/// Start the serve loop, binding an HTTP control API and polling workspace health
pub fn run(
    workspace: Option<&str>,
    all: bool,
    port: Option<u16>,
    probe_interval_secs: u64,
    remote: bool,
    bind_override: Option<&str>,
) -> Result<()> {
    let targets = resolve_targets(workspace, all)?;
    if targets.is_empty() {
        println!("No workspaces to serve.");
        return Ok(());
    }

    for target in &targets {
        state::ensure_tutti_dir(&target.project_root)?;
    }

    let selected_port = port.unwrap_or_else(resolve_default_port);

    // Resolve bind address: --bind flag > --remote (0.0.0.0) > global config > 127.0.0.1
    let global = GlobalConfig::load().ok();
    let host = if let Some(b) = bind_override {
        b.to_string()
    } else if remote {
        "0.0.0.0".to_string()
    } else {
        global
            .as_ref()
            .and_then(|g| g.serve.as_ref())
            .map(|s| s.bind.clone())
            .unwrap_or_else(|| "127.0.0.1".to_string())
    };

    // Determine if auth is required
    let auth_mode = if remote {
        crate::config::ServeAuthMode::Bearer
    } else {
        global
            .as_ref()
            .and_then(|g| g.serve.as_ref())
            .map(|s| s.auth.clone())
            .unwrap_or_default()
    };

    // Block non-localhost bind without auth to prevent accidental unauthenticated exposure
    if !is_localhost_addr(&host) && auth_mode == crate::config::ServeAuthMode::None {
        return Err(TuttiError::ConfigValidation(
            "refusing to bind to non-localhost address without authentication; \
             use --remote to enable bearer-token auth, or set [serve] auth = \"bearer\" in config"
                .to_string(),
        ));
    }

    // Generate/load bearer token when auth is enabled
    let auth_token: Option<Arc<String>> = if auth_mode == crate::config::ServeAuthMode::Bearer {
        let token = load_or_generate_serve_token()?;
        println!("serve: bearer token: {token}");
        Some(Arc::new(token))
    } else {
        None
    };

    let http_targets = Arc::new(targets.clone());
    start_control_http_server(http_targets, &host, selected_port, auth_token)?;
    let resilience = global.as_ref().and_then(|g| g.resilience.as_ref());
    let recovery_cooldown = Duration::from_secs(90);
    let mut last_recovery_attempt = HashMap::<String, Instant>::new();

    println!(
        "serve: running {} workspace(s), control API at http://{}:{}/v1",
        targets.len(),
        host,
        selected_port
    );
    println!("serve: press Ctrl+C to stop");

    let mut in_flight = HashSet::<String>::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let mut ticker =
            tokio::time::interval(std::time::Duration::from_secs(probe_interval_secs.max(1)));
        loop {
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {
                    break;
                }
                _ = ticker.tick() => {
                    for target in &targets {
                        match health::probe_workspace(&target.config, &target.project_root, 200) {
                            Ok(records) => {
                                match attempt_resilience_recovery_for_workspace(
                                    target,
                                    &records,
                                    resilience,
                                    recovery_cooldown,
                                    &mut last_recovery_attempt,
                                ) {
                                    Ok(events) => {
                                        for event in events {
                                            println!("{event}");
                                        }
                                    }
                                    Err(e) => {
                                        eprintln!("warn: auth recovery tick failed for {}: {e}", target.name);
                                    }
                                }
                            }
                            Err(e) => {
                                eprintln!("warn: health probe failed for {}: {e}", target.name);
                            }
                        }
                        match scheduler::run_due_workflows_for_workspace(
                            &target.config,
                            &target.project_root,
                            &mut in_flight,
                        ) {
                            Ok(events) => {
                                for event in events {
                                    println!("{event}");
                                }
                            }
                            Err(e) => {
                                eprintln!("warn: scheduler tick failed for {}: {e}", target.name);
                            }
                        }
                    }
                }
            }
        }
    });

    println!("serve: stopped");
    Ok(())
}

/// Attempt automatic recovery for unhealthy agents in a workspace
fn attempt_resilience_recovery_for_workspace(
    target: &WorkspaceTarget,
    records: &[state::AgentHealth],
    resilience: Option<&ResilienceConfig>,
    recovery_cooldown: Duration,
    last_recovery_attempt: &mut HashMap<String, Instant>,
) -> Result<Vec<String>> {
    let mut out = Vec::new();

    for record in records {
        let Some(trigger) = health::recovery_trigger(record) else {
            continue;
        };
        let Some(strategy) = rotation_strategy_for_trigger(resilience, trigger) else {
            continue;
        };
        let key = format!("{}/{}", target.name, record.agent);
        if !recovery_cooldown_elapsed(last_recovery_attempt.get(&key), recovery_cooldown) {
            continue;
        }
        last_recovery_attempt.insert(key, Instant::now());

        let reason = record
            .reason
            .as_deref()
            .unwrap_or("auth_failed")
            .to_string();
        let correlation_prefix = format!(
            "serve-{}-{}-{}",
            target.name,
            record.agent,
            Utc::now().timestamp_millis()
        );
        let _ = state::append_control_event(
            &target.project_root,
            &ControlEvent {
                event: "agent.recovery_attempted".to_string(),
                workspace: target.name.clone(),
                agent: Some(record.agent.clone()),
                timestamp: Utc::now(),
                correlation_id: format!("{correlation_prefix}-agent.recovery_attempted"),
                data: Some(json!({
                    "reason": reason,
                    "strategy": strategy,
                    "trigger": trigger.as_str()
                })),
            },
        );

        let recovery_result = with_project_root(&target.project_root, || {
            restart_agent_session(target, &record.agent)
        });

        match recovery_result {
            Ok(()) => {
                let _ = state::append_control_event(
                    &target.project_root,
                    &ControlEvent {
                        event: "agent.recovery_succeeded".to_string(),
                        workspace: target.name.clone(),
                        agent: Some(record.agent.clone()),
                        timestamp: Utc::now(),
                        correlation_id: format!("{correlation_prefix}-agent.recovery_succeeded"),
                        data: Some(json!({
                            "reason": reason,
                            "strategy": strategy,
                            "trigger": trigger.as_str()
                        })),
                    },
                );
                out.push(format!(
                    "recovered {} in {} via {}",
                    record.agent, target.name, strategy
                ));
            }
            Err(e) => {
                let _ = state::append_control_event(
                    &target.project_root,
                    &ControlEvent {
                        event: "agent.recovery_failed".to_string(),
                        workspace: target.name.clone(),
                        agent: Some(record.agent.clone()),
                        timestamp: Utc::now(),
                        correlation_id: format!("{correlation_prefix}-agent.recovery_failed"),
                        data: Some(json!({
                            "reason": reason,
                            "strategy": strategy,
                            "trigger": trigger.as_str(),
                            "error": e.to_string()
                        })),
                    },
                );
                out.push(format!(
                    "recovery failed for {} in {}: {}",
                    record.agent, target.name, e
                ));
            }
        }
    }

    Ok(out)
}

/// Kill and restart an agent's tmux session
fn restart_agent_session(target: &WorkspaceTarget, agent_name: &str) -> Result<()> {
    let session = TmuxSession::session_name(&target.config.workspace.name, agent_name);
    if TmuxSession::session_exists(&session) {
        TmuxSession::kill_session(&session)?;
    }
    super::up::run(Some(agent_name), None, false, false, None, None)?;
    Ok(())
}

/// Return the configured rotation strategy for a given recovery trigger, if any
fn rotation_strategy_for_trigger(
    resilience: Option<&ResilienceConfig>,
    trigger: health::RecoveryTrigger,
) -> Option<&str> {
    let resilience = resilience?;
    match trigger {
        health::RecoveryTrigger::RateLimited => resilience
            .rate_limit_strategy
            .as_deref()
            .filter(|s| strategy_requests_rotation(Some(s))),
        health::RecoveryTrigger::ProviderDown => resilience
            .provider_down_strategy
            .as_deref()
            .filter(|s| strategy_requests_rotation(Some(s))),
        health::RecoveryTrigger::AuthFailed => resilience
            .rate_limit_strategy
            .as_deref()
            .filter(|s| strategy_requests_rotation(Some(s)))
            .or_else(|| {
                resilience
                    .provider_down_strategy
                    .as_deref()
                    .filter(|s| strategy_requests_rotation(Some(s)))
            }),
    }
}

/// Check whether a strategy string indicates provider rotation
fn strategy_requests_rotation(strategy: Option<&str>) -> bool {
    strategy.is_some_and(|s| {
        matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "rotate" | "rotate_profile" | "profile_rotate" | "failover" | "auto_rotate"
        )
    })
}

/// Return true if enough time has passed since the last recovery attempt
fn recovery_cooldown_elapsed(last_attempt: Option<&Instant>, recovery_cooldown: Duration) -> bool {
    last_attempt.is_none_or(|last| last.elapsed() >= recovery_cooldown)
}

/// Spawn an HTTP server thread for the control API
fn start_control_http_server(
    targets: Arc<Vec<WorkspaceTarget>>,
    host: &str,
    port: u16,
    auth_token: Option<Arc<String>>,
) -> Result<()> {
    let server = Server::http((host, port)).map_err(|e| {
        TuttiError::ConfigValidation(format!("failed to bind health HTTP server: {e}"))
    })?;
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let request_targets = targets.clone();
            let token = auth_token.clone();
            thread::spawn(move || {
                handle_http_request(
                    request,
                    &request_targets,
                    token.as_deref().map(|s| s.as_str()),
                );
            });
        }
    });
    Ok(())
}

/// Validate a bearer token from an Authorization header value.
/// Returns `true` if auth is not required (`expected` is `None`) or if the
/// header contains a valid `Bearer <token>` matching `expected`.
fn validate_bearer_auth(auth_header: Option<&str>, expected: Option<&str>) -> bool {
    let Some(token) = expected else {
        return true;
    };
    auth_header
        .and_then(|v| v.strip_prefix("Bearer "))
        .map(|t| t == token)
        .unwrap_or(false)
}

/// Check whether a bind address refers to the local machine.
fn is_localhost_addr(addr: &str) -> bool {
    addr == "127.0.0.1" || addr == "localhost" || addr == "::1"
}

/// Dispatch an incoming HTTP request to the appropriate handler
fn handle_http_request(
    request: Request,
    targets: &[WorkspaceTarget],
    expected_token: Option<&str>,
) {
    // Auth middleware: reject if bearer token is required but missing/invalid
    if expected_token.is_some() {
        let auth_header = request
            .headers()
            .iter()
            .find(|h| h.field.equiv("Authorization"))
            .map(|h| h.value.as_str().to_string());
        let valid = validate_bearer_auth(auth_header.as_deref(), expected_token);
        if !valid {
            let body = api_err(
                "auth",
                "unauthorized",
                "invalid or missing bearer token".to_string(),
            )
            .to_string();
            let mut response = Response::from_string(body).with_status_code(StatusCode(401));
            if let Ok(h) = Header::from_bytes("Content-Type", "application/json") {
                response = response.with_header(h);
            }
            let _ = request.respond(response);
            return;
        }
    }

    let is_stream = request.method() == &Method::Get
        && request.url().split('?').next() == Some("/v1/events/stream");
    if is_stream {
        handle_sse_request(request, targets);
        return;
    }

    let mut request = request;
    let (status, body) = route_request(&mut request, targets);
    let mut response = Response::from_string(body).with_status_code(status);
    if let Ok(h) = Header::from_bytes("Content-Type", "application/json") {
        response = response.with_header(h);
    }
    let _ = request.respond(response);
}

/// Route an HTTP request to the matching read or action endpoint
fn route_request(request: &mut Request, targets: &[WorkspaceTarget]) -> (StatusCode, String) {
    let raw_url = request.url().to_string();
    let (path, query) = match raw_url.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (raw_url, None),
    };
    let query_map = parse_query(query.as_deref());
    let method = request.method().clone();

    if method == Method::Get {
        match route_read(&path, &query_map, targets) {
            Ok(value) => (StatusCode(200), value.to_string()),
            Err(TuttiError::AgentNotFound(e)) => {
                (StatusCode(404), api_err("read", "not_found", e).to_string())
            }
            Err(e) => (
                StatusCode(400),
                api_err("read", "bad_request", e.to_string()).to_string(),
            ),
        }
    } else if method == Method::Post && path.starts_with("/v1/actions/") {
        match route_action(request, &path, targets) {
            Ok((status, value)) => (status, value.to_string()),
            Err(e) => (
                StatusCode(400),
                api_err("action", "action_failed", e.to_string()).to_string(),
            ),
        }
    } else if method == Method::Post && path == "/v1/webhooks/generic" {
        match route_webhook(request, targets) {
            Ok(value) => (StatusCode(200), value.to_string()),
            Err(e) => (
                StatusCode(400),
                api_err("webhook", "webhook_failed", e.to_string()).to_string(),
            ),
        }
    } else {
        (
            StatusCode(404),
            api_err(
                "route",
                "not_found",
                format!("unknown endpoint: {} {}", method, path),
            )
            .to_string(),
        )
    }
}

/// Handle GET requests by matching the path to a read endpoint
fn route_read(
    path: &str,
    query: &HashMap<String, String>,
    targets: &[WorkspaceTarget],
) -> Result<Value> {
    match path {
        "/v1/health" => {
            let mut all = Vec::new();
            for target in targets {
                let mut records = state::load_all_health(&target.project_root)?;
                all.append(&mut records);
            }
            all.sort_by(|a, b| {
                a.workspace
                    .cmp(&b.workspace)
                    .then_with(|| a.agent.cmp(&b.agent))
            });
            Ok(api_ok("health.list", json!(all)))
        }
        "/v1/status" | "/v1/voices" => Ok(api_ok("status.list", status_data(targets)?)),
        "/v1/workflows" => Ok(api_ok("workflows.list", workflows_data(targets))),
        "/v1/runs" => Ok(api_ok("runs.list", runs_data(targets)?)),
        "/v1/logs" => Ok(api_ok("logs.list", logs_data(targets)?)),
        "/v1/handoffs" => Ok(api_ok("handoffs.list", handoffs_data(targets)?)),
        "/v1/policy-decisions" => Ok(api_ok(
            "policy_decisions.list",
            policy_decisions_data(targets, query.get("workspace").map(|s| s.as_str()))?,
        )),
        "/v1/events" => Ok(api_ok(
            "events.list",
            events_data(
                targets,
                query.get("cursor").map(|s| s.as_str()),
                query.get("workspace").map(|s| s.as_str()),
            )?,
        )),
        _ if path.starts_with("/v1/health/") => {
            let parts: Vec<&str> = path.split('/').collect();
            if parts.len() == 5 {
                let workspace = parts[3];
                let agent = parts[4];
                let target = targets
                    .iter()
                    .find(|t| t.name == workspace)
                    .ok_or_else(|| TuttiError::AgentNotFound(format!("{workspace}/{agent}")))?;
                let record = state::load_agent_health(&target.project_root, agent)?
                    .ok_or_else(|| TuttiError::AgentNotFound(format!("{workspace}/{agent}")))?;
                Ok(api_ok("health.get", json!(record)))
            } else {
                Err(TuttiError::ConfigValidation(
                    "invalid health path".to_string(),
                ))
            }
        }
        _ => Err(TuttiError::ConfigValidation("not found".to_string())),
    }
}

/// Parse a URL query string into a key-value map
fn parse_query(query: Option<&str>) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(query) = query else {
        return out;
    };
    for pair in query.split('&') {
        let mut parts = pair.splitn(2, '=');
        let key = parts.next().unwrap_or_default().trim();
        if key.is_empty() {
            continue;
        }
        let value = parts.next().unwrap_or_default().trim();
        out.insert(key.to_string(), value.to_string());
    }
    out
}

/// Handle POST requests to /v1/actions/* with idempotency support
fn route_action(
    request: &mut Request,
    path: &str,
    targets: &[WorkspaceTarget],
) -> Result<(StatusCode, Value)> {
    let action = path.trim_start_matches("/v1/actions/").to_string();
    if action.is_empty() {
        return Ok((
            StatusCode(404),
            api_err("action", "not_found", "missing action name".to_string()),
        ));
    }

    let body = read_json_body(request)?;
    let idempotency_key = read_idempotency_key(request, &body);
    let target = resolve_action_workspace(targets, body.get("workspace").and_then(Value::as_str))?;

    if let Some(key) = idempotency_key.as_deref()
        && let Some(cached) = idempotency_lookup(target, key)?
    {
        if cached.action == action {
            return Ok((StatusCode(200), cached.response));
        }
        return Ok((
            StatusCode(409),
            api_err(
                action.as_str(),
                "idempotency_conflict",
                format!(
                    "idempotency key reused for different action: {}",
                    cached.action
                ),
            ),
        ));
    }

    let data = execute_action(&action, &body, target)?;
    let response = api_ok(action.as_str(), data);

    if let Some(key) = idempotency_key.as_deref() {
        idempotency_save(target, key, action.as_str(), response.clone())?;
    }

    Ok((StatusCode(200), response))
}

/// Execute a named action (up, down, send, run, verify, review, land) against a workspace
fn execute_action(action: &str, body: &Value, target: &WorkspaceTarget) -> Result<Value> {
    match action {
        "up" => {
            let agent = body.get("agent").and_then(Value::as_str);
            let fresh_worktree = body
                .get("fresh_worktree")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            with_project_root(&target.project_root, || {
                super::up::run(agent, None, false, fresh_worktree, None, None)
            })?;
            Ok(json!({"workspace": target.name, "agent": agent, "fresh_worktree": fresh_worktree}))
        }
        "down" => {
            let agent = body.get("agent").and_then(Value::as_str);
            let clean = body.get("clean").and_then(Value::as_bool).unwrap_or(false);
            with_project_root(&target.project_root, || {
                super::down::run(agent, None, false, clean)
            })?;
            Ok(json!({"workspace": target.name, "agent": agent, "clean": clean}))
        }
        "send" => {
            let agent = required_body_str(body, "agent")?;
            let prompt = required_body_str(body, "prompt")?;
            let options = super::send::SendOptions {
                auto_up: body
                    .get("auto_up")
                    .and_then(Value::as_bool)
                    .unwrap_or(false),
                wait: body.get("wait").and_then(Value::as_bool).unwrap_or(false),
                timeout_secs: body
                    .get("timeout_secs")
                    .and_then(Value::as_u64)
                    .unwrap_or(900),
                idle_stable_secs: body
                    .get("idle_stable_secs")
                    .and_then(Value::as_u64)
                    .unwrap_or(5),
                output: body.get("output").and_then(Value::as_bool).unwrap_or(false),
                output_lines: body
                    .get("output_lines")
                    .and_then(Value::as_u64)
                    .unwrap_or(200) as u32,
            };
            let send_result = with_project_root(&target.project_root, || {
                super::send::run(agent, &[prompt.to_string()], options)
            })?;
            Ok(json!({
                "workspace": target.name,
                "agent": agent,
                "send": send_result
            }))
        }
        "run" => {
            let workflow = required_body_str(body, "workflow")?;
            let agent = body.get("agent").and_then(Value::as_str);
            let strict = body.get("strict").and_then(Value::as_bool).unwrap_or(false);
            let dry_run = body
                .get("dry_run")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            with_project_root(&target.project_root, || {
                super::run::run(Some(workflow), None, false, agent, false, strict, dry_run)
            })?;
            Ok(json!({
                "workspace": target.name,
                "workflow": workflow,
                "agent": agent,
                "strict": strict,
                "dry_run": dry_run
            }))
        }
        "verify" => {
            let workflow = body.get("workflow").and_then(Value::as_str);
            let agent = body.get("agent").and_then(Value::as_str);
            let strict = body.get("strict").and_then(Value::as_bool).unwrap_or(false);
            with_project_root(&target.project_root, || {
                super::verify::run(false, false, workflow, agent, strict)
            })?;
            Ok(json!({
                "workspace": target.name,
                "workflow": workflow,
                "agent": agent,
                "strict": strict
            }))
        }
        "review" => {
            let agent = required_body_str(body, "agent")?;
            let reviewer = body
                .get("reviewer")
                .and_then(Value::as_str)
                .unwrap_or("reviewer");
            with_project_root(&target.project_root, || super::review::run(agent, reviewer))?;
            Ok(json!({"workspace": target.name, "agent": agent, "reviewer": reviewer}))
        }
        "land" => {
            let agent = required_body_str(body, "agent")?;
            let pr = body.get("pr").and_then(Value::as_bool).unwrap_or(false);
            let force = body.get("force").and_then(Value::as_bool).unwrap_or(false);
            with_project_root(&target.project_root, || super::land::run(agent, pr, force))?;
            Ok(json!({"workspace": target.name, "agent": agent, "pr": pr, "force": force}))
        }
        _ => Err(TuttiError::ConfigValidation(format!(
            "unknown action '{}'",
            action
        ))),
    }
}

/// Maximum webhook payload size (1 MB)
const WEBHOOK_MAX_BODY_BYTES: usize = 1_048_576;

/// Mutex to serialize webhook dispatches so that `with_project_root` cwd changes
/// do not race across concurrent request threads.
static WEBHOOK_DISPATCH_LOCK: std::sync::LazyLock<Mutex<()>> =
    std::sync::LazyLock::new(|| Mutex::new(()));

/// Handle POST /v1/webhooks/generic
fn route_webhook(request: &mut Request, targets: &[WorkspaceTarget]) -> Result<Value> {
    // Body size is enforced inside read_json_body via take()
    let body = read_json_body(request)?;

    let source = body
        .get("source")
        .and_then(Value::as_str)
        .unwrap_or("generic");
    let event_type = body
        .get("event")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let payload = body.get("payload").cloned().unwrap_or_else(|| body.clone());
    let workspace_hint = body.get("workspace").and_then(Value::as_str);

    let target = resolve_action_workspace(targets, workspace_hint)?;

    // Deduplication: derive an idempotency key from the delivery ID header,
    // Idempotency-Key header, or a hash of the payload to prevent duplicate
    // workflow runs from GitHub retries or concurrent identical deliveries.
    let dedup_key = webhook_dedup_key(request, &body);
    if let Some(cached) = idempotency_lookup(target, &dedup_key)? {
        return Ok(cached.response);
    }

    let matched = webhook::match_triggers(&target.config.webhooks, source, event_type);

    if matched.is_empty() {
        webhook::log_event(&target.project_root, source, event_type, None, "no_match");
        let response = api_ok(
            "webhook.received",
            json!({
                "source": source,
                "event": event_type,
                "matched": false,
                "triggers_fired": 0
            }),
        );
        idempotency_save(target, &dedup_key, "webhook.received", response.clone())?;
        return Ok(response);
    }

    // Serialize webhook dispatches behind a mutex so that concurrent requests
    // cannot race on the process-wide cwd changed by `with_project_root`.
    let _guard = WEBHOOK_DISPATCH_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());

    let mut dispatched = Vec::new();
    for wh in &matched {
        if let Some(workflow) = &wh.workflow {
            with_project_root(&target.project_root, || {
                super::run::run(
                    Some(workflow),
                    None,
                    false,
                    wh.agent.as_deref(),
                    false,
                    false,
                    false,
                )
            })?;
            webhook::log_event(
                &target.project_root,
                source,
                event_type,
                Some(workflow),
                "dispatched_workflow",
            );
            dispatched.push(json!({
                "type": "workflow",
                "workflow": workflow,
                "agent": wh.agent,
            }));
        } else if let Some(agent) = &wh.agent {
            let raw_prompt = wh
                .prompt
                .as_deref()
                .unwrap_or("Webhook event received — check recent events for details.");
            let prompt = webhook::expand_template(raw_prompt, &payload);
            let options = super::send::SendOptions {
                auto_up: false,
                wait: false,
                timeout_secs: 900,
                idle_stable_secs: 5,
                output: false,
                output_lines: 200,
            };
            with_project_root(&target.project_root, || {
                super::send::run(agent, std::slice::from_ref(&prompt), options).map(|_| ())
            })?;
            webhook::log_event(
                &target.project_root,
                source,
                event_type,
                Some(agent),
                "dispatched_send",
            );
            dispatched.push(json!({
                "type": "send",
                "agent": agent,
            }));
        }
    }

    let response = api_ok(
        "webhook.received",
        json!({
            "source": source,
            "event": event_type,
            "matched": true,
            "triggers_fired": dispatched.len(),
            "dispatched": dispatched
        }),
    );
    idempotency_save(target, &dedup_key, "webhook.received", response.clone())?;
    Ok(response)
}

/// Derive a deduplication key for a webhook request.
/// Prefers X-GitHub-Delivery or Idempotency-Key headers; falls back to a
/// hash of the payload body for generic webhook sources.
fn webhook_dedup_key(request: &Request, body: &Value) -> String {
    // Check X-GitHub-Delivery header (unique per GitHub webhook delivery)
    for header in request.headers() {
        if header.field.equiv("X-GitHub-Delivery") {
            let val = header.value.as_str().trim();
            if !val.is_empty() {
                return format!("webhook:{val}");
            }
        }
    }
    // Check Idempotency-Key header or body field
    if let Some(key) = read_idempotency_key(request, body) {
        return format!("webhook:{key}");
    }
    // Fall back to payload content hash
    use std::hash::{DefaultHasher, Hash, Hasher};
    let serialized = body.to_string();
    let mut hasher = DefaultHasher::new();
    serialized.hash(&mut hasher);
    format!("webhook:hash:{:x}", hasher.finish())
}

/// Resolve the target workspace for an action, defaulting to the only one if singular
fn resolve_action_workspace<'a>(
    targets: &'a [WorkspaceTarget],
    workspace: Option<&str>,
) -> Result<&'a WorkspaceTarget> {
    if let Some(ws_name) = workspace {
        return targets.iter().find(|t| t.name == ws_name).ok_or_else(|| {
            TuttiError::ConfigValidation(format!("unknown workspace '{}'", ws_name))
        });
    }

    if targets.len() == 1 {
        return Ok(&targets[0]);
    }

    Err(TuttiError::ConfigValidation(
        "workspace is required when serving multiple workspaces".to_string(),
    ))
}

/// Read and parse the request body as JSON, defaulting to an empty object.
/// Enforces the body size cap while reading via `take()` to prevent memory exhaustion
/// from oversized payloads (a malicious client could send more bytes than Content-Length).
fn read_json_body(request: &mut Request) -> Result<Value> {
    let mut body = String::new();
    request
        .as_reader()
        .take((WEBHOOK_MAX_BODY_BYTES + 1) as u64)
        .read_to_string(&mut body)?;
    if body.len() > WEBHOOK_MAX_BODY_BYTES {
        return Err(TuttiError::ConfigValidation(format!(
            "webhook payload too large (>{WEBHOOK_MAX_BODY_BYTES} bytes)"
        )));
    }
    if body.trim().is_empty() {
        return Ok(json!({}));
    }
    let parsed: Value = serde_json::from_str(&body)?;
    Ok(parsed)
}

/// Extract a required non-empty string field from the JSON body
fn required_body_str<'a>(body: &'a Value, key: &str) -> Result<&'a str> {
    body.get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| TuttiError::ConfigValidation(format!("missing required field '{}'", key)))
}

/// Extract the idempotency key from the request header or body
fn read_idempotency_key(request: &Request, body: &Value) -> Option<String> {
    if let Some(header_value) = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("Idempotency-Key"))
        .map(|h| h.value.as_str().to_string())
        .filter(|v| !v.trim().is_empty())
    {
        return Some(header_value);
    }
    body.get("idempotency_key")
        .and_then(Value::as_str)
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

/// Return the path to the idempotency store file for a project
fn idempotency_file(project_root: &Path) -> PathBuf {
    project_root
        .join(".tutti")
        .join("state")
        .join("api-idempotency.json")
}

/// Look up a previously cached response by idempotency key
fn idempotency_lookup(target: &WorkspaceTarget, key: &str) -> Result<Option<IdempotencyEntry>> {
    let file = idempotency_file(&target.project_root);
    if !file.exists() {
        return Ok(None);
    }
    let body = std::fs::read_to_string(file)?;
    let map: HashMap<String, IdempotencyEntry> = serde_json::from_str(&body)?;
    Ok(map.get(key).cloned())
}

/// Persist a response under the given idempotency key
fn idempotency_save(
    target: &WorkspaceTarget,
    key: &str,
    action: &str,
    response: Value,
) -> Result<()> {
    let file = idempotency_file(&target.project_root);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut map = if file.exists() {
        let body = std::fs::read_to_string(&file)?;
        serde_json::from_str::<HashMap<String, IdempotencyEntry>>(&body).unwrap_or_default()
    } else {
        HashMap::new()
    };
    map.insert(
        key.to_string(),
        IdempotencyEntry {
            action: action.to_string(),
            response,
            created_at: Utc::now(),
        },
    );
    std::fs::write(file, serde_json::to_string_pretty(&map)?)?;
    Ok(())
}

/// Gather agent status snapshots across all served workspaces
fn status_data(targets: &[WorkspaceTarget]) -> Result<Value> {
    let mut rows = Vec::new();
    for target in targets {
        let snapshots = snapshot::gather_workspace_snapshots(&target.config, &target.project_root);
        for s in snapshots {
            rows.push(json!({
                "workspace": s.workspace_name,
                "agent": s.agent_name,
                "runtime": s.runtime,
                "status": s.status_raw,
                "running": s.running,
                "session": s.session_name,
                "ctx": s.ctx_pct
            }));
        }
    }
    Ok(Value::Array(rows))
}

/// Collect workflow definitions from all served workspaces
fn workflows_data(targets: &[WorkspaceTarget]) -> Value {
    let mut rows = Vec::new();
    for target in targets {
        for wf in &target.config.workflows {
            rows.push(json!({
                "workspace": target.name,
                "name": wf.name,
                "description": wf.description,
                "schedule": wf.schedule,
                "steps": wf.steps.len()
            }));
        }
    }
    Value::Array(rows)
}

/// Load automation run records from all served workspaces
fn runs_data(targets: &[WorkspaceTarget]) -> Result<Value> {
    let mut rows = Vec::new();
    for target in targets {
        let path = target
            .project_root
            .join(".tutti")
            .join("state")
            .join("automation-runs.jsonl");
        if !path.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&path)?;
        for line in content.lines().filter(|l| !l.trim().is_empty()) {
            if let Ok(mut value) = serde_json::from_str::<Value>(line) {
                if let Value::Object(ref mut map) = value {
                    map.insert("workspace".to_string(), Value::String(target.name.clone()));
                }
                rows.push(value);
            }
        }
    }
    Ok(Value::Array(rows))
}

/// List log files with metadata from all served workspaces
fn logs_data(targets: &[WorkspaceTarget]) -> Result<Value> {
    let mut rows = Vec::new();
    for target in targets {
        let dir = target.project_root.join(".tutti").join("logs");
        if !dir.exists() {
            continue;
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "log") {
                let metadata = std::fs::metadata(&path)?;
                rows.push(json!({
                    "workspace": target.name,
                    "file": path.file_name().and_then(|v| v.to_str()).unwrap_or_default(),
                    "path": path.display().to_string(),
                    "size_bytes": metadata.len(),
                    "modified_at": metadata.modified().ok().map(|t| DateTime::<Utc>::from(t).to_rfc3339()),
                }));
            }
        }
    }
    Ok(Value::Array(rows))
}

/// List handoff files with metadata from all served workspaces
fn handoffs_data(targets: &[WorkspaceTarget]) -> Result<Value> {
    let mut rows = Vec::new();
    for target in targets {
        let dir = target.project_root.join(".tutti").join("handoffs");
        if !dir.exists() {
            continue;
        }
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "md") {
                let metadata = std::fs::metadata(&path)?;
                let filename = path
                    .file_name()
                    .and_then(|v| v.to_str())
                    .unwrap_or_default()
                    .to_string();
                let agent = filename.split('-').next().unwrap_or_default().to_string();
                rows.push(json!({
                    "workspace": target.name,
                    "agent": agent,
                    "file": filename,
                    "path": path.display().to_string(),
                    "size_bytes": metadata.len(),
                    "modified_at": metadata.modified().ok().map(|t| DateTime::<Utc>::from(t).to_rfc3339()),
                }));
            }
        }
    }
    Ok(Value::Array(rows))
}

/// Load policy decision records, optionally filtered by workspace
fn policy_decisions_data(targets: &[WorkspaceTarget], workspace: Option<&str>) -> Result<Value> {
    let selected: Vec<&WorkspaceTarget> = if let Some(ws) = workspace {
        vec![
            targets
                .iter()
                .find(|t| t.name == ws)
                .ok_or_else(|| TuttiError::AgentNotFound(ws.to_string()))?,
        ]
    } else {
        targets.iter().collect()
    };

    let mut rows = Vec::new();
    for target in selected {
        let records = state::load_policy_decisions(&target.project_root)?;
        rows.extend(records);
    }
    rows.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
    Ok(json!(rows))
}

/// Load control events with cursor-based pagination
fn events_data(
    targets: &[WorkspaceTarget],
    cursor: Option<&str>,
    workspace: Option<&str>,
) -> Result<Value> {
    let parsed = parse_cursor(cursor)?;
    let cursor_ts = parsed.as_ref().map(|c| c.timestamp);
    let skip_count = parsed.as_ref().map(|c| c.skip_count).unwrap_or(0);
    let include_boundary = skip_count > 0;
    let mut events = load_events_for_targets(targets, workspace, cursor_ts, include_boundary)?;
    // Skip events at the cursor boundary that were already seen
    if skip_count > 0
        && let Some(ts) = cursor_ts
    {
        let boundary_count = events.iter().take_while(|e| e.timestamp == ts).count();
        if boundary_count > 0 {
            events = events
                .into_iter()
                .skip(skip_count.min(boundary_count))
                .collect();
        }
    }
    Ok(json!(events))
}

/// Handle a Server-Sent Events stream request for real-time event delivery
fn handle_sse_request(request: Request, targets: &[WorkspaceTarget]) {
    let raw_url = request.url().to_string();
    let (_, query) = match raw_url.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (raw_url, None),
    };
    let query_map = parse_query(query.as_deref());
    let workspace = query_map.get("workspace").map(|v| v.to_string());
    let cursor_ts = match parse_cursor(query_map.get("cursor").map(|v| v.as_str())) {
        Ok(Some(parsed)) if parsed.skip_count > 0 => {
            let body = api_err(
                "events.stream",
                "bad_request",
                "stream endpoint does not support skip_count cursors; use a bare timestamp"
                    .to_string(),
            )
            .to_string();
            let mut response = Response::from_string(body).with_status_code(StatusCode(400));
            if let Some(h) = header("Content-Type", "application/json") {
                response = response.with_header(h);
            }
            let _ = request.respond(response);
            return;
        }
        Ok(parsed) => parsed.map(|c| c.timestamp),
        Err(e) => {
            let body = api_err("events.stream", "bad_request", e.to_string()).to_string();
            let mut response = Response::from_string(body).with_status_code(StatusCode(400));
            if let Some(h) = header("Content-Type", "application/json") {
                response = response.with_header(h);
            }
            let _ = request.respond(response);
            return;
        }
    };
    let timeout_secs = query_map
        .get("timeout_secs")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(300)
        .clamp(1, 3600);
    let poll_millis = query_map
        .get("poll_millis")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(1000)
        .clamp(200, 10_000);

    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let stream_targets = targets.to_vec();
    thread::spawn(move || {
        let _ = tx.send(b": tutti events stream\n\n".to_vec());
        let mut cursor = cursor_ts;
        let mut seen = HashSet::<String>::new();
        let mut last_heartbeat = Instant::now();
        let started = Instant::now();

        while started.elapsed() < Duration::from_secs(timeout_secs) {
            let since =
                cursor.and_then(|ts| ts.checked_sub_signed(chrono::TimeDelta::nanoseconds(1)));
            match load_events_for_targets(&stream_targets, workspace.as_deref(), since, false) {
                Ok(events) => {
                    for event in events {
                        let key = event_key(&event);
                        if !seen.insert(key) {
                            continue;
                        }
                        cursor = Some(cursor.map_or(event.timestamp, |ts| ts.max(event.timestamp)));
                        let payload =
                            serde_json::to_string(&event).unwrap_or_else(|_| "{}".to_string());
                        let chunk = format!("event: {}\ndata: {}\n\n", event.event, payload);
                        if tx.send(chunk.into_bytes()).is_err() {
                            return;
                        }
                    }
                }
                Err(e) => {
                    let chunk = format!(
                        "event: error\ndata: {}\n\n",
                        json!({"message": e.to_string()})
                    );
                    let _ = tx.send(chunk.into_bytes());
                    return;
                }
            }

            if last_heartbeat.elapsed() >= Duration::from_secs(10) {
                if tx.send(b": keepalive\n\n".to_vec()).is_err() {
                    return;
                }
                last_heartbeat = Instant::now();
            }
            thread::sleep(Duration::from_millis(poll_millis));
        }

        let _ = tx.send(
            format!(
                "event: end\ndata: {}\n\n",
                json!({"reason":"timeout","timeout_secs": timeout_secs})
            )
            .into_bytes(),
        );
    });

    let mut headers = Vec::new();
    if let Some(h) = header("Content-Type", "text/event-stream") {
        headers.push(h);
    }
    if let Some(h) = header("Cache-Control", "no-cache") {
        headers.push(h);
    }
    if let Some(h) = header("Connection", "keep-alive") {
        headers.push(h);
    }
    let reader = ChannelReader::new(rx);
    let response =
        Response::new(StatusCode(200), headers, reader, None, None).with_chunked_threshold(0);
    let _ = request.respond(response);
}

struct ParsedCursor {
    timestamp: DateTime<Utc>,
    skip_count: usize,
}

/// Parse an optional cursor string into a timestamp and skip count
fn parse_cursor(cursor: Option<&str>) -> Result<Option<ParsedCursor>> {
    let Some(raw) = cursor else {
        return Ok(None);
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }

    if let Some((ts_part, skip_part)) = raw.rsplit_once('|') {
        let timestamp = DateTime::parse_from_rfc3339(ts_part)
            .map_err(|e| {
                TuttiError::ConfigValidation(format!(
                    "invalid cursor '{}': expected RFC3339 timestamp ({e})",
                    raw
                ))
            })?
            .with_timezone(&Utc);
        let skip_count: usize = skip_part.parse().map_err(|_| {
            TuttiError::ConfigValidation(format!(
                "invalid cursor '{}': skip count '{}' is not a valid number",
                raw, skip_part
            ))
        })?;
        Ok(Some(ParsedCursor {
            timestamp,
            skip_count,
        }))
    } else {
        let timestamp = DateTime::parse_from_rfc3339(raw)
            .map_err(|e| {
                TuttiError::ConfigValidation(format!(
                    "invalid cursor '{}': expected RFC3339 timestamp ({e})",
                    raw
                ))
            })?
            .with_timezone(&Utc);
        Ok(Some(ParsedCursor {
            timestamp,
            skip_count: 0,
        }))
    }
}

/// Load and merge control events from selected workspaces, filtered by cursor
fn load_events_for_targets(
    targets: &[WorkspaceTarget],
    workspace: Option<&str>,
    cursor_ts: Option<DateTime<Utc>>,
    include_cursor: bool,
) -> Result<Vec<state::ControlEvent>> {
    let selected: Vec<&WorkspaceTarget> = if let Some(ws) = workspace {
        vec![
            targets
                .iter()
                .find(|t| t.name == ws)
                .ok_or_else(|| TuttiError::AgentNotFound(ws.to_string()))?,
        ]
    } else {
        targets.iter().collect()
    };

    let mut rows = Vec::new();
    for target in selected {
        let events = state::load_control_events(&target.project_root)?;
        for event in events {
            if let Some(ts) = cursor_ts
                && ((include_cursor && event.timestamp < ts)
                    || (!include_cursor && event.timestamp <= ts))
            {
                continue;
            }
            rows.push(event);
        }
    }
    rows.sort_by(|a, b| {
        a.timestamp
            .cmp(&b.timestamp)
            .then_with(|| a.correlation_id.cmp(&b.correlation_id))
    });
    Ok(rows)
}

/// Build a deduplication key for a control event
fn event_key(event: &state::ControlEvent) -> String {
    format!(
        "{}|{}|{}|{}|{}",
        event.timestamp.to_rfc3339(),
        event.correlation_id,
        event.workspace,
        event.agent.as_deref().unwrap_or(""),
        event.event
    )
}

/// Create an HTTP header, returning None if the bytes are invalid
fn header(name: &str, value: &str) -> Option<Header> {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).ok()
}

struct ChannelReader {
    rx: mpsc::Receiver<Vec<u8>>,
    buf: Vec<u8>,
    offset: usize,
}

impl ChannelReader {
    /// Create a new reader that pulls chunks from the given channel
    fn new(rx: mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            rx,
            buf: Vec::new(),
            offset: 0,
        }
    }
}

impl Read for ChannelReader {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if out.is_empty() {
            return Ok(0);
        }

        loop {
            if self.offset < self.buf.len() {
                let remaining = self.buf.len() - self.offset;
                let n = remaining.min(out.len());
                out[..n].copy_from_slice(&self.buf[self.offset..self.offset + n]);
                self.offset += n;
                if self.offset >= self.buf.len() {
                    self.buf.clear();
                    self.offset = 0;
                }
                return Ok(n);
            }

            match self.rx.recv() {
                Ok(chunk) => {
                    self.buf = chunk;
                    self.offset = 0;
                }
                Err(_) => return Ok(0),
            }
        }
    }
}

/// Run an operation with the working directory temporarily set to project_root
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

/// Build a success API response envelope
fn api_ok(action: &str, data: Value) -> Value {
    json!({
        "ok": true,
        "action": action,
        "error": null,
        "data": data
    })
}

/// Build an error API response envelope
fn api_err(action: &str, code: &str, message: String) -> Value {
    json!({
        "ok": false,
        "action": action,
        "error": {
            "code": code,
            "message": message
        },
        "data": Value::Null
    })
}

/// Load an existing serve token or generate and persist a new one.
/// Token is stored at ~/.config/tutti/serve-token.
fn load_or_generate_serve_token() -> Result<String> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."));
    let config_dir = home.join(".config").join("tutti");
    let token_path = config_dir.join("serve-token");

    // Try loading an existing token
    if token_path.exists() {
        let contents = std::fs::read_to_string(&token_path)?;
        let token = contents.trim().to_string();
        if token.len() >= 32 {
            return Ok(token);
        }
    }

    // Generate a 256-bit (32-byte) random hex token
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|e| {
        TuttiError::ConfigValidation(format!("failed to generate random token: {e}"))
    })?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();

    std::fs::create_dir_all(&config_dir)?;
    std::fs::write(&token_path, &token)?;
    // Restrict permissions to owner only
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600))?;
    }

    Ok(token)
}

fn resolve_default_port() -> u16 {
    match GlobalConfig::load() {
        Ok(global) => global.dashboard.map(|d| d.port).unwrap_or(4040),
        Err(_) => 4040,
    }
}

/// Resolve workspace targets from CLI flags, global config, or current directory
fn resolve_targets(workspace: Option<&str>, all: bool) -> Result<Vec<WorkspaceTarget>> {
    if all {
        let global = GlobalConfig::load()?;
        let mut targets = Vec::new();
        for ws in &global.registered_workspaces {
            let (config, config_path) = TuttiConfig::load(&ws.path)?;
            config.validate()?;
            let root = config_path.parent().ok_or_else(|| {
                TuttiError::ConfigValidation("could not determine workspace root".to_string())
            })?;
            targets.push(WorkspaceTarget {
                name: config.workspace.name.clone(),
                project_root: root.to_path_buf(),
                config,
            });
        }
        return Ok(targets);
    }

    if let Some(ws_name) = workspace {
        let (config, config_path) = super::up::load_workspace_by_name(ws_name)?;
        config.validate()?;
        let root = config_path.parent().ok_or_else(|| {
            TuttiError::ConfigValidation("could not determine workspace root".to_string())
        })?;
        return Ok(vec![WorkspaceTarget {
            name: config.workspace.name.clone(),
            project_root: root.to_path_buf(),
            config,
        }]);
    }

    let cwd = std::env::current_dir()?;
    let (config, config_path) = TuttiConfig::load(&cwd)?;
    config.validate()?;
    let root = config_path.parent().ok_or_else(|| {
        TuttiError::ConfigValidation("could not determine workspace root".to_string())
    })?;
    Ok(vec![WorkspaceTarget {
        name: config.workspace.name.clone(),
        project_root: root.to_path_buf(),
        config,
    }])
}

/// Compile-time assertion that a value is a Path reference
#[allow(dead_code)]
fn _assert_path(_: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct HomeGuard(Option<String>);

    impl Drop for HomeGuard {
        fn drop(&mut self) {
            if let Some(value) = self.0.take() {
                unsafe {
                    std::env::set_var("HOME", value);
                }
            } else {
                unsafe {
                    std::env::remove_var("HOME");
                }
            }
        }
    }

    #[test]
    fn strategy_requests_rotation_matches_supported_values() {
        assert!(strategy_requests_rotation(Some("rotate_profile")));
        assert!(strategy_requests_rotation(Some("failover")));
        assert!(strategy_requests_rotation(Some("AUTO_ROTATE")));
        assert!(!strategy_requests_rotation(Some("pause")));
        assert!(!strategy_requests_rotation(None));
    }

    #[test]
    fn rotation_strategy_for_trigger_picks_expected_strategy() {
        let rate_limit = ResilienceConfig {
            provider_down_strategy: Some("pause".to_string()),
            save_state_on_failure: false,
            rate_limit_strategy: Some("rotate_profile".to_string()),
            retry_max_attempts: None,
            retry_initial_backoff_ms: None,
            retry_max_backoff_ms: None,
        };
        assert_eq!(
            rotation_strategy_for_trigger(Some(&rate_limit), health::RecoveryTrigger::RateLimited),
            Some("rotate_profile")
        );

        let provider_down = ResilienceConfig {
            provider_down_strategy: Some("failover".to_string()),
            save_state_on_failure: false,
            rate_limit_strategy: Some("pause".to_string()),
            retry_max_attempts: None,
            retry_initial_backoff_ms: None,
            retry_max_backoff_ms: None,
        };
        assert_eq!(
            rotation_strategy_for_trigger(
                Some(&provider_down),
                health::RecoveryTrigger::ProviderDown
            ),
            Some("failover")
        );

        let disabled = ResilienceConfig {
            provider_down_strategy: Some("pause".to_string()),
            save_state_on_failure: false,
            rate_limit_strategy: Some("pause".to_string()),
            retry_max_attempts: None,
            retry_initial_backoff_ms: None,
            retry_max_backoff_ms: None,
        };
        assert_eq!(
            rotation_strategy_for_trigger(Some(&disabled), health::RecoveryTrigger::AuthFailed),
            None
        );
    }

    #[test]
    fn recovery_cooldown_elapsed_throttles_recent_attempts() {
        let now = Instant::now();
        assert!(recovery_cooldown_elapsed(None, Duration::from_secs(30)));
        assert!(!recovery_cooldown_elapsed(
            Some(&now),
            Duration::from_secs(30)
        ));
        assert!(recovery_cooldown_elapsed(Some(&now), Duration::ZERO));
    }

    // ── Remote-serve auth tests ──

    #[test]
    fn validate_bearer_auth_accepts_valid_token() {
        assert!(validate_bearer_auth(Some("Bearer abc123"), Some("abc123")));
    }

    #[test]
    fn validate_bearer_auth_rejects_missing_header() {
        assert!(!validate_bearer_auth(None, Some("abc123")));
    }

    #[test]
    fn validate_bearer_auth_rejects_wrong_token() {
        assert!(!validate_bearer_auth(Some("Bearer wrong"), Some("abc123")));
    }

    #[test]
    fn validate_bearer_auth_rejects_malformed_header() {
        // Missing "Bearer " prefix
        assert!(!validate_bearer_auth(Some("abc123"), Some("abc123")));
        // Basic auth instead of bearer
        assert!(!validate_bearer_auth(
            Some("Basic dXNlcjpwYXNz"),
            Some("abc123")
        ));
    }

    #[test]
    fn validate_bearer_auth_skips_when_not_required() {
        assert!(validate_bearer_auth(None, None));
        assert!(validate_bearer_auth(Some("Bearer anything"), None));
    }

    #[test]
    fn is_localhost_addr_identifies_local_addresses() {
        assert!(is_localhost_addr("127.0.0.1"));
        assert!(is_localhost_addr("localhost"));
        assert!(is_localhost_addr("::1"));
        assert!(!is_localhost_addr("0.0.0.0"));
        assert!(!is_localhost_addr("192.168.1.1"));
        assert!(!is_localhost_addr("10.0.0.1"));
    }

    #[test]
    #[serial]
    fn token_generation_produces_valid_hex_and_reloads() {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let temp =
            std::env::temp_dir().join(format!("tutti-test-token-{}-{nanos}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(temp.join(".config").join("tutti")).unwrap();

        // Temporarily override HOME so token writes to temp.
        let _home_guard = HomeGuard(std::env::var("HOME").ok());
        unsafe { std::env::set_var("HOME", &temp) };

        let token1 = load_or_generate_serve_token().expect("first generation should succeed");
        assert_eq!(token1.len(), 64, "256-bit token should be 64 hex chars");
        assert!(
            token1.chars().all(|c| c.is_ascii_hexdigit()),
            "token should be valid hex"
        );

        // Second call should reload the same token
        let token2 = load_or_generate_serve_token().expect("reload should succeed");
        assert_eq!(token1, token2, "reloaded token should match original");

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    fn serve_config_toml_round_trip() {
        let toml_str = r#"
[serve]
bind = "0.0.0.0"
auth = "bearer"
"#;
        #[derive(Debug, Deserialize)]
        struct Wrapper {
            serve: crate::config::ServeConfig,
        }
        let parsed: Wrapper = toml::from_str(toml_str).expect("should parse");
        assert_eq!(parsed.serve.bind, "0.0.0.0");
        assert_eq!(parsed.serve.auth, crate::config::ServeAuthMode::Bearer);

        // Default values
        let toml_default = "[serve]\n";
        let parsed_default: Wrapper = toml::from_str(toml_default).expect("should parse defaults");
        assert_eq!(parsed_default.serve.bind, "127.0.0.1");
        assert_eq!(
            parsed_default.serve.auth,
            crate::config::ServeAuthMode::None
        );
    }
}
