use crate::config::{GlobalConfig, TuttiConfig};
use crate::error::{Result, TuttiError};
use crate::health;
use crate::state::{self, AgentHealth};
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};

pub fn run(agent: Option<&str>, workspace: Option<&str>, all: bool, json: bool) -> Result<()> {
    let targets = resolve_targets(workspace, all)?;
    let mut records = Vec::<AgentHealth>::new();

    for (config, project_root) in targets {
        state::ensure_tutti_dir(&project_root)?;
        let mut probed = health::probe_workspace(&config, &project_root, 200)?;
        records.append(&mut probed);
    }

    if let Some(agent_name) = agent {
        records.retain(|h| h.agent == agent_name);
        if records.is_empty() {
            return Err(TuttiError::AgentNotFound(agent_name.to_string()));
        }
    }

    records.sort_by(|a, b| {
        a.workspace
            .cmp(&b.workspace)
            .then_with(|| a.agent.cmp(&b.agent))
    });

    if json {
        println!("{}", serde_json::to_string_pretty(&records)?);
        return Ok(());
    }

    if records.is_empty() {
        println!("No health records found.");
        return Ok(());
    }

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec![
        "Workspace",
        "Agent",
        "Runtime",
        "Running",
        "Activity",
        "Auth",
        "Last Output Change",
        "Reason",
    ]);

    for h in records {
        table.add_row(vec![
            h.workspace,
            h.agent,
            h.runtime,
            h.running.to_string(),
            format!("{:?}", h.activity_state).to_lowercase(),
            format!("{:?}", h.auth_state).to_lowercase(),
            h.last_output_change_at
                .map(|ts| ts.to_rfc3339())
                .unwrap_or_else(|| "--".to_string()),
            h.reason.unwrap_or_else(|| "--".to_string()),
        ]);
    }

    println!("{table}");
    Ok(())
}

fn resolve_targets(
    workspace: Option<&str>,
    all: bool,
) -> Result<Vec<(TuttiConfig, std::path::PathBuf)>> {
    if all {
        let global = GlobalConfig::load()?;
        let mut targets = Vec::new();
        for ws in &global.registered_workspaces {
            if let Ok((config, config_path)) = TuttiConfig::load(&ws.path) {
                config.validate()?;
                if let Some(root) = config_path.parent() {
                    targets.push((config, root.to_path_buf()));
                }
            }
        }
        return Ok(targets);
    }

    if let Some(ws_name) = workspace {
        let (config, config_path) = super::up::load_workspace_by_name(ws_name)?;
        config.validate()?;
        let project_root = config_path.parent().ok_or_else(|| {
            TuttiError::ConfigValidation("could not determine workspace root".to_string())
        })?;
        return Ok(vec![(config, project_root.to_path_buf())]);
    }

    let cwd = std::env::current_dir()?;
    let (config, config_path) = TuttiConfig::load(&cwd)?;
    config.validate()?;
    let project_root = config_path.parent().ok_or_else(|| {
        TuttiError::ConfigValidation("could not determine workspace root".to_string())
    })?;
    Ok(vec![(config, project_root.to_path_buf())])
}
