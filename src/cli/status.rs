use crate::config::{GlobalConfig, TuttiConfig};
use crate::error::Result;
use crate::runtime::{self, AgentStatus};
use crate::session::TmuxSession;
use crate::state;
use colored::Colorize;
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};

/// A row of agent status data, used by both `status` and `watch`.
pub struct AgentStatusRow {
    pub name: String,
    pub runtime: String,
    /// ANSI-colored status for display.
    pub status: String,
    /// Plain status string (no ANSI) for persisting to state files.
    pub raw_status: String,
    pub session: String,
}

/// Gather status for all agents in a config.
pub fn gather_agent_statuses(
    config: &TuttiConfig,
    project_root: &std::path::Path,
) -> Vec<AgentStatusRow> {
    let mut rows = Vec::new();

    for agent in &config.agents {
        let runtime_name = agent
            .resolved_runtime(&config.defaults)
            .unwrap_or_else(|| "—".to_string());

        let session = TmuxSession::session_name(&config.workspace.name, &agent.name);
        let running = TmuxSession::session_exists(&session);

        let (status, raw_status) = if running {
            detect_status_pair(&runtime_name, &session, project_root, &agent.name)
        } else {
            ("Stopped".dimmed().to_string(), "Stopped".to_string())
        };

        let session_display = if running {
            session.clone()
        } else {
            "—".to_string()
        };

        rows.push(AgentStatusRow {
            name: agent.name.clone(),
            runtime: runtime_name,
            status,
            raw_status,
            session: session_display,
        });
    }

    rows
}

pub fn run(all: bool) -> Result<()> {
    crate::session::tmux::check_tmux()?;

    if all {
        return run_all();
    }

    let cwd = std::env::current_dir()?;
    let (config, config_path) = TuttiConfig::load(&cwd)?;
    let project_root = config_path.parent().unwrap();

    if config.agents.is_empty() {
        println!("No agents defined in tutti.toml");
        return Ok(());
    }

    println!("{}", format!("Workspace: {}", config.workspace.name).bold());
    print_agent_table(&config, project_root);

    Ok(())
}

fn run_all() -> Result<()> {
    let global = GlobalConfig::load()?;
    if global.registered_workspaces.is_empty() {
        println!("No registered workspaces. Run `tt init` in your projects first.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["Workspace", "Agent", "Runtime", "Status", "Session"]);

    for ws in &global.registered_workspaces {
        match TuttiConfig::load(&ws.path) {
            Ok((config, config_path)) => {
                let project_root = config_path.parent().unwrap();
                let rows = gather_agent_statuses(&config, project_root);
                if rows.is_empty() {
                    table.add_row(vec![
                        ws.name.clone(),
                        "(no agents defined)".dimmed().to_string(),
                        "".to_string(),
                        "".to_string(),
                        "".to_string(),
                    ]);
                    continue;
                }
                for (i, row) in rows.iter().enumerate() {
                    let ws_col = if i == 0 {
                        ws.name.clone()
                    } else {
                        "".to_string()
                    };
                    table.add_row(vec![
                        ws_col,
                        row.name.clone(),
                        row.runtime.clone(),
                        row.status.clone(),
                        row.session.clone(),
                    ]);
                }
            }
            Err(_) => {
                table.add_row(vec![
                    ws.name.clone(),
                    "(config error)".red().to_string(),
                    "".to_string(),
                    "".to_string(),
                    "".to_string(),
                ]);
            }
        }
    }

    println!("{table}");
    Ok(())
}

fn print_agent_table(config: &TuttiConfig, project_root: &std::path::Path) {
    let rows = gather_agent_statuses(config, project_root);

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["Agent", "Runtime", "Status", "Session"]);

    for row in &rows {
        table.add_row(vec![&row.name, &row.runtime, &row.status, &row.session]);
    }

    println!("{table}");
}

/// Returns (formatted_status, raw_status) pair.
fn detect_status_pair(
    runtime_name: &str,
    session: &str,
    project_root: &std::path::Path,
    agent_name: &str,
) -> (String, String) {
    if let Some(adapter) = runtime::get_adapter(runtime_name, None) {
        match TmuxSession::capture_pane(session, 50) {
            Ok(output) => {
                let s = adapter.detect_status(&output);
                if let AgentStatus::AuthFailed(ref reason) = s {
                    let _ = state::save_emergency_state(project_root, agent_name, &output, reason);
                }
                (format_status(&s), s.to_string())
            }
            Err(_) => ("Unknown".dimmed().to_string(), "Unknown".to_string()),
        }
    } else {
        ("Unknown".dimmed().to_string(), "Unknown".to_string())
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
