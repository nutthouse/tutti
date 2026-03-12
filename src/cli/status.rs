use super::snapshot::gather_workspace_snapshots;
use crate::config::{GlobalConfig, TuttiConfig};
use crate::error::Result;
use colored::Colorize;
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};

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
                let snapshots = gather_workspace_snapshots(&config, project_root);
                if snapshots.is_empty() {
                    table.add_row(vec![
                        ws.name.clone(),
                        "(no agents defined)".dimmed().to_string(),
                        "".to_string(),
                        "".to_string(),
                        "".to_string(),
                    ]);
                    continue;
                }
                for (i, snap) in snapshots.iter().enumerate() {
                    let ws_col = if i == 0 {
                        snap.workspace_name.clone()
                    } else {
                        "".to_string()
                    };
                    table.add_row(vec![
                        ws_col,
                        snap.agent_name.clone(),
                        snap.runtime.clone(),
                        snap.status_display.clone(),
                        snap.session_name.clone(),
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
    let snapshots = gather_workspace_snapshots(config, project_root);

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["Agent", "Runtime", "Status", "Session"]);

    for snapshot in &snapshots {
        table.add_row(vec![
            &snapshot.agent_name,
            &snapshot.runtime,
            &snapshot.status_display,
            &snapshot.session_name,
        ]);
    }

    println!("{table}");
}
