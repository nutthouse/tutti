use crate::config::{GlobalConfig, TuttiConfig};
use crate::error::{Result, TuttiError};
use crate::health;
use crate::scheduler;
use crate::state;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use tiny_http::{Header, Method, Response, Server, StatusCode};

#[derive(Clone)]
struct WorkspaceTarget {
    name: String,
    project_root: PathBuf,
    config: TuttiConfig,
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
    start_health_http_server(http_targets, host, selected_port)?;

    println!(
        "serve: running {} workspace(s), health endpoint at http://{}:{}/v1/health",
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

fn start_health_http_server(
    targets: Arc<Vec<WorkspaceTarget>>,
    host: &str,
    port: u16,
) -> Result<()> {
    let server = Server::http((host, port)).map_err(|e| {
        TuttiError::ConfigValidation(format!("failed to bind health HTTP server: {e}"))
    })?;
    thread::spawn(move || {
        for request in server.incoming_requests() {
            if request.method() != &Method::Get {
                let _ = request.respond(
                    Response::from_string("method not allowed").with_status_code(StatusCode(405)),
                );
                continue;
            }

            let url = request.url().to_string();
            let body = match health_response_json(&targets, &url) {
                Ok(json) => (StatusCode(200), json),
                Err(e) => (StatusCode(404), format!(r#"{{"error":"{}"}}"#, e)),
            };

            let mut response = Response::from_string(body.1).with_status_code(body.0);
            if let Ok(h) = Header::from_bytes("Content-Type", "application/json") {
                response = response.with_header(h);
            }
            let _ = request.respond(response);
        }
    });
    Ok(())
}

fn health_response_json(targets: &[WorkspaceTarget], url: &str) -> Result<String> {
    if url == "/v1/health" {
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
        return Ok(serde_json::to_string_pretty(&all)?);
    }

    let parts: Vec<&str> = url.split('/').collect();
    if parts.len() == 5 && parts[1] == "v1" && parts[2] == "health" {
        let workspace = parts[3];
        let agent = parts[4];
        let target = targets
            .iter()
            .find(|t| t.name == workspace)
            .ok_or_else(|| TuttiError::AgentNotFound(format!("{workspace}/{agent}")))?;
        let record = state::load_agent_health(&target.project_root, agent)?
            .ok_or_else(|| TuttiError::AgentNotFound(format!("{workspace}/{agent}")))?;
        return Ok(serde_json::to_string_pretty(&record)?);
    }

    Err(TuttiError::ConfigValidation("not found".to_string()))
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
