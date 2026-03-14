use crate::cli::snapshot;
use crate::config::{GlobalConfig, TuttiConfig};
use crate::error::{Result, TuttiError};
use crate::health;
use crate::scheduler;
use crate::state;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
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

pub fn run(
    workspace: Option<&str>,
    all: bool,
    port: Option<u16>,
    probe_interval_secs: u64,
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
    let host = "127.0.0.1";
    let http_targets = Arc::new(targets.clone());
    start_control_http_server(http_targets, host, selected_port)?;

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
                        if let Err(e) = health::probe_workspace(&target.config, &target.project_root, 200) {
                            eprintln!("warn: health probe failed for {}: {e}", target.name);
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

fn start_control_http_server(
    targets: Arc<Vec<WorkspaceTarget>>,
    host: &str,
    port: u16,
) -> Result<()> {
    let server = Server::http((host, port)).map_err(|e| {
        TuttiError::ConfigValidation(format!("failed to bind health HTTP server: {e}"))
    })?;
    thread::spawn(move || {
        for request in server.incoming_requests() {
            let request_targets = targets.clone();
            thread::spawn(move || {
                handle_http_request(request, &request_targets);
            });
        }
    });
    Ok(())
}

fn handle_http_request(request: Request, targets: &[WorkspaceTarget]) {
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

fn read_json_body(request: &mut Request) -> Result<Value> {
    let mut body = String::new();
    request.as_reader().read_to_string(&mut body)?;
    if body.trim().is_empty() {
        return Ok(json!({}));
    }
    let parsed: Value = serde_json::from_str(&body)?;
    Ok(parsed)
}

fn required_body_str<'a>(body: &'a Value, key: &str) -> Result<&'a str> {
    body.get(key)
        .and_then(Value::as_str)
        .filter(|v| !v.trim().is_empty())
        .ok_or_else(|| TuttiError::ConfigValidation(format!("missing required field '{}'", key)))
}

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

fn idempotency_file(project_root: &Path) -> PathBuf {
    project_root
        .join(".tutti")
        .join("state")
        .join("api-idempotency.json")
}

fn idempotency_lookup(target: &WorkspaceTarget, key: &str) -> Result<Option<IdempotencyEntry>> {
    let file = idempotency_file(&target.project_root);
    if !file.exists() {
        return Ok(None);
    }
    let body = std::fs::read_to_string(file)?;
    let map: HashMap<String, IdempotencyEntry> = serde_json::from_str(&body)?;
    Ok(map.get(key).cloned())
}

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

fn events_data(
    targets: &[WorkspaceTarget],
    cursor: Option<&str>,
    workspace: Option<&str>,
) -> Result<Value> {
    let cursor_ts = parse_cursor_ts(cursor)?;
    let events = load_events_for_targets(targets, workspace, cursor_ts, false)?;
    Ok(json!(events))
}

fn handle_sse_request(request: Request, targets: &[WorkspaceTarget]) {
    let raw_url = request.url().to_string();
    let (_, query) = match raw_url.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (raw_url, None),
    };
    let query_map = parse_query(query.as_deref());
    let workspace = query_map.get("workspace").map(|v| v.to_string());
    let cursor_ts = match parse_cursor_ts(query_map.get("cursor").map(|v| v.as_str())) {
        Ok(value) => value,
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

fn parse_cursor_ts(cursor: Option<&str>) -> Result<Option<DateTime<Utc>>> {
    match cursor {
        Some(raw) if !raw.trim().is_empty() => Ok(Some(
            DateTime::parse_from_rfc3339(raw)
                .map_err(|e| {
                    TuttiError::ConfigValidation(format!(
                        "invalid cursor '{}': expected RFC3339 timestamp ({e})",
                        raw
                    ))
                })?
                .with_timezone(&Utc),
        )),
        _ => Ok(None),
    }
}

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

fn header(name: &str, value: &str) -> Option<Header> {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).ok()
}

struct ChannelReader {
    rx: mpsc::Receiver<Vec<u8>>,
    buf: Vec<u8>,
    offset: usize,
}

impl ChannelReader {
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

fn api_ok(action: &str, data: Value) -> Value {
    json!({
        "ok": true,
        "action": action,
        "error": null,
        "data": data
    })
}

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

fn resolve_default_port() -> u16 {
    match GlobalConfig::load() {
        Ok(global) => global.dashboard.map(|d| d.port).unwrap_or(4040),
        Err(_) => 4040,
    }
}

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

#[allow(dead_code)]
fn _assert_path(_: &Path) {}
