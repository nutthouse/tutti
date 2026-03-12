use crate::config::TuttiConfig;
use crate::error::Result;
use crate::state;
use colored::Colorize;
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};
use std::thread;
use std::time::Duration;

use super::status::gather_agent_statuses;

pub fn run(interval: u64) -> Result<()> {
    crate::session::tmux::check_tmux()?;

    let cwd = std::env::current_dir()?;
    let (config, config_path) = TuttiConfig::load(&cwd)?;
    let project_root = config_path.parent().unwrap();

    if config.agents.is_empty() {
        println!("No agents defined in tutti.toml");
        return Ok(());
    }

    loop {
        // Clear terminal
        print!("\x1B[2J\x1B[H");

        let rows = gather_agent_statuses(&config, project_root);

        println!(
            "{}\n",
            format!(
                "Workspace: {} (refreshing every {}s)",
                config.workspace.name, interval
            )
            .bold()
        );

        let mut table = Table::new();
        table.load_preset(UTF8_BORDERS_ONLY);
        table.set_header(vec!["Agent", "Runtime", "Status", "Session"]);

        for row in &rows {
            table.add_row(vec![&row.name, &row.runtime, &row.status, &row.session]);
        }

        println!("{table}");
        println!("\n{}", "Press Ctrl+C to exit".dimmed());

        // Update state files with plain (non-ANSI) status
        for row in &rows {
            let _ = state::update_status_if_exists(project_root, &row.name, &row.raw_status);
        }

        thread::sleep(Duration::from_secs(interval));
    }
}
