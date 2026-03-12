use crate::config::GlobalConfig;
use crate::error::{Result, TuttiError};
use crate::usage::{
    ProfileUsageSummary, WorkspaceUsage, estimate_total_hours, format_tokens, summarize_profile,
};
use colored::Colorize;
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};
use std::path::PathBuf;

pub fn run(profile_filter: Option<&str>, by_workspace: bool) -> Result<()> {
    let global = GlobalConfig::load()?;

    if global.profiles.is_empty() {
        println!("No profiles configured.");
        println!(
            "Add a [[profile]] section to {} to get started.",
            "~/.config/tutti/config.toml".cyan()
        );
        return Ok(());
    }

    // Match each workspace to its profile
    let workspace_profiles = build_workspace_profile_map(&global);

    let profiles: Vec<_> = if let Some(filter) = profile_filter {
        global
            .profiles
            .iter()
            .filter(|p| p.name == filter)
            .collect()
    } else {
        global.profiles.iter().collect()
    };

    if profiles.is_empty() {
        return Err(TuttiError::UsageData(format!(
            "profile '{}' not found",
            profile_filter.unwrap_or("")
        )));
    }

    for profile in &profiles {
        let workspaces: Vec<(String, PathBuf)> = workspace_profiles
            .iter()
            .filter(|(_, prof_name)| prof_name == &profile.name)
            .map(|(name, _)| {
                let path = global
                    .registered_workspaces
                    .iter()
                    .find(|w| w.name == *name)
                    .map(|w| w.path.clone())
                    .unwrap_or_default();
                (name.clone(), path)
            })
            .collect();

        // Also include workspaces without explicit profile assignment if this is the only/first profile
        let workspaces = if workspaces.is_empty() && profiles.len() == 1 {
            global
                .registered_workspaces
                .iter()
                .map(|w| (w.name.clone(), w.path.clone()))
                .collect()
        } else {
            workspaces
        };

        match summarize_profile(profile, &workspaces) {
            Ok(summary) => {
                print_profile_summary(&summary, by_workspace);
                println!();
            }
            Err(e) => {
                eprintln!(
                    "  {} failed to scan profile '{}': {e}",
                    "warn".yellow(),
                    profile.name
                );
            }
        }
    }

    Ok(())
}

/// Map workspace names to their default profile.
fn build_workspace_profile_map(global: &GlobalConfig) -> Vec<(String, String)> {
    let mut map = Vec::new();

    for ws in &global.registered_workspaces {
        // Try to load the workspace config to find its default_profile
        if let Ok((config, _)) = crate::config::TuttiConfig::load(&ws.path)
            && let Some(auth) = &config.workspace.auth
            && let Some(profile_name) = &auth.default_profile
        {
            map.push((ws.name.clone(), profile_name.clone()));
        }
    }

    map
}

fn print_profile_summary(summary: &ProfileUsageSummary, by_workspace: bool) {
    let plan_display = summary.plan.as_deref().unwrap_or("unknown").to_uppercase();

    println!(
        "Profile: {} ({})",
        summary.profile_name.bold(),
        plan_display.dimmed()
    );
    println!(
        "  Resets: {} → {}",
        summary.reset_start.format("%a %b %-d"),
        (summary.reset_end - chrono::Duration::days(1)).format("%a %b %-d")
    );
    println!();

    // Period summary table
    let today_hours = estimate_total_hours(&summary.today);
    let weekly_hours = estimate_total_hours(&summary.weekly);

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["Period", "Input", "Output", "~Hours"]);
    table.add_row(vec![
        "Today".to_string(),
        format_tokens(summary.today.total.total_input()),
        format_tokens(summary.today.total.output_tokens),
        format!("~{:.1}h", today_hours),
    ]);
    table.add_row(vec![
        "This week".to_string(),
        format_tokens(summary.weekly.total.total_input()),
        format_tokens(summary.weekly.total.output_tokens),
        format!("~{:.1}h", weekly_hours),
    ]);
    println!("  {}", table.to_string().replace('\n', "\n  "));

    // Capacity bar
    if let (Some(pct), Some(ceiling)) = (summary.capacity_pct, summary.weekly_hours) {
        println!();
        print_capacity_bar(pct, ceiling);
    }

    // Workspace breakdown
    if by_workspace && !summary.by_workspace.is_empty() {
        println!();
        print_workspace_breakdown(&summary.by_workspace);
    }
}

fn print_capacity_bar(pct: f64, ceiling: f64) {
    let pct_clamped = pct.clamp(0.0, 100.0);
    let bar_width = 20;
    let filled = ((pct_clamped / 100.0) * bar_width as f64).round() as usize;
    let empty = bar_width - filled;

    let filled_str = "█".repeat(filled);
    let empty_str = "░".repeat(empty);

    let bar = if pct < 60.0 {
        format!("{}{}", filled_str.green(), empty_str)
    } else if pct < 80.0 {
        format!("{}{}", filled_str.yellow(), empty_str)
    } else {
        format!("{}{}", filled_str.red(), empty_str)
    };

    println!(
        "  Capacity: {} {:.0}% of {:.1}h",
        bar,
        pct.min(999.0),
        ceiling
    );
}

fn print_workspace_breakdown(workspaces: &[WorkspaceUsage]) {
    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["Workspace", "Input", "Output", "~Hours"]);

    for ws in workspaces {
        let hours = estimate_total_hours(&ws.usage);
        table.add_row(vec![
            ws.workspace_name.clone(),
            format_tokens(ws.usage.total.total_input()),
            format_tokens(ws.usage.total.output_tokens),
            format!("~{:.1}h", hours),
        ]);

        // Agent sub-rows
        let mut agents: Vec<_> = ws.by_agent.iter().collect();
        agents.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (agent_name, agent_usage) in agents {
            let agent_hours = estimate_total_hours(agent_usage);
            table.add_row(vec![
                format!("  ↳ {agent_name}"),
                format_tokens(agent_usage.total.total_input()),
                format_tokens(agent_usage.total.output_tokens),
                format!("~{:.1}h", agent_hours),
            ]);
        }
    }

    println!("  {}", table.to_string().replace('\n', "\n  "));
}
