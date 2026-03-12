use crate::config::{GlobalConfig, TuttiConfig};
use crate::error::Result;
use crate::session::TmuxSession;
use colored::Colorize;
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};

/// List all registered workspaces.
pub fn list() -> Result<()> {
    let global = GlobalConfig::load()?;

    if global.registered_workspaces.is_empty() {
        println!("No registered workspaces. Run `tt init` in your projects to register them.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["Workspace", "Path", "Agents"]);

    for ws in &global.registered_workspaces {
        let agent_count = match TuttiConfig::load(&ws.path) {
            Ok((config, _)) => format!("{}", config.agents.len()),
            Err(_) => "?".to_string(),
        };
        table.add_row(vec![
            ws.name.clone(),
            ws.path.display().to_string(),
            agent_count,
        ]);
    }

    println!("{table}");
    Ok(())
}

/// Show status overview of all workspaces.
pub fn status() -> Result<()> {
    let global = GlobalConfig::load()?;

    if global.registered_workspaces.is_empty() {
        println!("No registered workspaces.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["Workspace", "Agents", "Running"]);

    for ws in &global.registered_workspaces {
        match TuttiConfig::load(&ws.path) {
            Ok((config, _)) => {
                let total = config.agents.len();
                let running = config
                    .agents
                    .iter()
                    .filter(|a| {
                        let session = TmuxSession::session_name(&config.workspace.name, &a.name);
                        TmuxSession::session_exists(&session)
                    })
                    .count();

                let running_str = if running > 0 {
                    format!("{running}/{total}").green().to_string()
                } else {
                    format!("0/{total}").dimmed().to_string()
                };

                table.add_row(vec![ws.name.clone(), total.to_string(), running_str]);
            }
            Err(_) => {
                table.add_row(vec![
                    ws.name.clone(),
                    "?".to_string(),
                    "(config error)".red().to_string(),
                ]);
            }
        }
    }

    println!("{table}");
    Ok(())
}
