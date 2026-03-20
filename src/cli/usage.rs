use crate::config::GlobalConfig;
use crate::error::{Result, TuttiError};
use crate::usage::{
    ProfileUsageSummary, TokenUsage, WorkspaceUsage, format_tokens, summarize_profile,
};
use colored::Colorize;
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};
use std::collections::HashMap;
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

    // Match each workspace to its explicit profile assignment.
    let workspace_profiles = build_workspace_profile_map(&global);
    let single_profile_mode = global.profiles.len() == 1;

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
        if !usage_enabled_for_profile(profile) {
            print_non_api_notice(profile);
            println!();
            continue;
        }

        // Sensible default:
        // - Single-profile setups: include unassigned workspaces.
        // - Multi-profile setups: require explicit [workspace.auth].default_profile.
        let workspaces = resolve_workspaces_for_profile(
            &global,
            &workspace_profiles,
            &profile.name,
            single_profile_mode,
        );

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

fn usage_enabled_for_profile(profile: &crate::config::ProfileConfig) -> bool {
    profile
        .plan
        .as_deref()
        .is_some_and(|p| p.trim().eq_ignore_ascii_case("api"))
}

fn print_non_api_notice(profile: &crate::config::ProfileConfig) {
    let lines = format_non_api_notice_lines(profile);
    for line in lines {
        println!("{line}");
    }
}

fn format_non_api_notice_lines(profile: &crate::config::ProfileConfig) -> [String; 3] {
    let plan_display = profile.plan.as_deref().unwrap_or("unknown").to_uppercase();
    [
        format!(
            "Profile: {} ({})",
            profile.name.bold(),
            plan_display.dimmed()
        ),
        format!("  {} usage metrics are API-only for now.", "info".cyan()),
        format!(
            "  Set {} in {} to enable {} for this profile.",
            "`plan = \"api\"`".cyan(),
            "~/.config/tutti/config.toml".cyan(),
            "`tt usage`".cyan()
        ),
    ]
}

/// Map workspace names to their default profile.
fn build_workspace_profile_map(global: &GlobalConfig) -> HashMap<String, String> {
    let mut map = HashMap::new();

    for ws in &global.registered_workspaces {
        // Try to load the workspace config to find its default_profile
        if let Ok((config, _)) = crate::config::TuttiConfig::load(&ws.path)
            && let Some(auth) = &config.workspace.auth
            && let Some(profile_name) = &auth.default_profile
        {
            map.insert(ws.name.clone(), profile_name.clone());
        }
    }

    map
}

fn resolve_workspaces_for_profile(
    global: &GlobalConfig,
    workspace_profiles: &HashMap<String, String>,
    profile_name: &str,
    include_unassigned: bool,
) -> Vec<(String, PathBuf)> {
    global
        .registered_workspaces
        .iter()
        .filter(|ws| match workspace_profiles.get(&ws.name) {
            Some(mapped_profile) => mapped_profile == profile_name,
            None => include_unassigned,
        })
        .map(|ws| (ws.name.clone(), ws.path.clone()))
        .collect()
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

    // Period summary table (token-first, no hour estimate columns)
    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["Period", "Input", "Cached", "Output", "Total"]);
    table.add_row(vec![
        "Today".to_string(),
        format_tokens(summary.today.total.input_tokens),
        format_tokens(total_cached_tokens(&summary.today.total)),
        format_tokens(summary.today.total.output_tokens),
        format_tokens(total_tokens(&summary.today.total)),
    ]);
    table.add_row(vec![
        "This week".to_string(),
        format_tokens(summary.weekly.total.input_tokens),
        format_tokens(total_cached_tokens(&summary.weekly.total)),
        format_tokens(summary.weekly.total.output_tokens),
        format_tokens(total_tokens(&summary.weekly.total)),
    ]);
    println!("  {}", table.to_string().replace('\n', "\n  "));

    // Capacity bar (% of configured plan ceiling)
    if let Some(pct) = summary.capacity_pct {
        println!();
        print_capacity_bar(pct);
    } else {
        println!();
        println!(
            "  Capacity: {} (set {} on this profile to compute plan %)",
            "n/a".dimmed(),
            "`weekly_hours`".cyan()
        );
    }

    // Workspace breakdown
    if by_workspace && !summary.by_workspace.is_empty() {
        println!();
        print_workspace_breakdown(&summary.by_workspace);
    }
}

fn print_capacity_bar(pct: f64) {
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
        "  Capacity: {} {:.0}% of configured plan ceiling",
        bar,
        pct.min(999.0)
    );
}

fn print_workspace_breakdown(workspaces: &[WorkspaceUsage]) {
    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["Workspace", "Input", "Cached", "Output", "Total"]);

    for ws in workspaces {
        table.add_row(vec![
            ws.workspace_name.clone(),
            format_tokens(ws.usage.total.input_tokens),
            format_tokens(total_cached_tokens(&ws.usage.total)),
            format_tokens(ws.usage.total.output_tokens),
            format_tokens(total_tokens(&ws.usage.total)),
        ]);

        // Agent sub-rows
        let mut agents: Vec<_> = ws.by_agent.iter().collect();
        agents.sort_by(|(a, _), (b, _)| a.cmp(b));
        for (agent_name, agent_usage) in agents {
            table.add_row(vec![
                format!("  ↳ {agent_name}"),
                format_tokens(agent_usage.total.input_tokens),
                format_tokens(total_cached_tokens(&agent_usage.total)),
                format_tokens(agent_usage.total.output_tokens),
                format_tokens(total_tokens(&agent_usage.total)),
            ]);
        }
    }

    println!("  {}", table.to_string().replace('\n', "\n  "));
}

fn total_cached_tokens(usage: &TokenUsage) -> u64 {
    usage.cache_creation_input_tokens + usage.cache_read_input_tokens
}

fn total_tokens(usage: &TokenUsage) -> u64 {
    usage.total_input() + usage.output_tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{GlobalConfig, ProfileConfig, RegisteredWorkspace};

    fn profile(name: &str) -> ProfileConfig {
        ProfileConfig {
            name: name.to_string(),
            provider: "anthropic".to_string(),
            command: "claude".to_string(),
            max_concurrent: None,
            monthly_budget: None,
            priority: None,
            plan: None,
            reset_day: None,
            weekly_hours: None,
        }
    }

    #[test]
    fn usage_enabled_for_api_plan_is_case_insensitive() {
        let mut p = profile("api-profile");
        p.plan = Some("API".to_string());
        assert!(usage_enabled_for_profile(&p));
    }

    #[test]
    fn usage_enabled_for_profile_rejects_non_api_and_missing() {
        let p = profile("none");
        assert!(!usage_enabled_for_profile(&p));

        let mut p2 = profile("max");
        p2.plan = Some("max".to_string());
        assert!(!usage_enabled_for_profile(&p2));
    }

    #[test]
    fn format_non_api_notice_mentions_api_only_and_required_plan() {
        let mut p = profile("personal");
        p.plan = Some("max".to_string());
        let lines = format_non_api_notice_lines(&p);
        assert_eq!(lines.len(), 3);
        assert!(lines[0].contains("personal"));
        assert!(lines[1].contains("API-only"));
        assert!(lines[2].contains("plan = \"api\""));
    }

    #[test]
    fn total_helpers_include_cached_and_output() {
        let usage = TokenUsage {
            input_tokens: 100,
            output_tokens: 30,
            cache_creation_input_tokens: 20,
            cache_read_input_tokens: 50,
        };
        assert_eq!(total_cached_tokens(&usage), 70);
        assert_eq!(total_tokens(&usage), 200);
    }

    #[test]
    fn resolve_workspaces_single_profile_includes_unassigned() {
        let global = GlobalConfig {
            user: None,
            profiles: vec![profile("personal")],
            registered_workspaces: vec![
                RegisteredWorkspace {
                    name: "a".to_string(),
                    path: PathBuf::from("/tmp/a"),
                },
                RegisteredWorkspace {
                    name: "b".to_string(),
                    path: PathBuf::from("/tmp/b"),
                },
            ],
            dashboard: None,
            resilience: None,
            permissions: None,
            serve: None,
            remotes: vec![],
        };
        let map = HashMap::from([("a".to_string(), "personal".to_string())]);

        let resolved = resolve_workspaces_for_profile(&global, &map, "personal", true);
        let names: Vec<_> = resolved.into_iter().map(|(name, _)| name).collect();

        assert_eq!(names, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn resolve_workspaces_multi_profile_excludes_unassigned() {
        let global = GlobalConfig {
            user: None,
            profiles: vec![profile("personal"), profile("work")],
            registered_workspaces: vec![
                RegisteredWorkspace {
                    name: "a".to_string(),
                    path: PathBuf::from("/tmp/a"),
                },
                RegisteredWorkspace {
                    name: "b".to_string(),
                    path: PathBuf::from("/tmp/b"),
                },
            ],
            dashboard: None,
            resilience: None,
            permissions: None,
            serve: None,
            remotes: vec![],
        };
        let map = HashMap::from([("a".to_string(), "work".to_string())]);

        let resolved = resolve_workspaces_for_profile(&global, &map, "personal", false);
        assert!(resolved.is_empty());
    }
}
