use crate::config::TuttiConfig;
use crate::error::{Result, TuttiError};
use crate::session::TmuxSession;
use crate::state;
use crate::state::ControlEvent;
use chrono::Utc;
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

const DEFAULT_CAPTURE_LINES: u32 = 120;

#[derive(Debug, Clone, Serialize)]
pub struct HandoffGenerateOutput {
    pub agent: String,
    pub packet: String,
    pub reason: String,
    pub ctx_pct: Option<u8>,
}

pub fn run(command: super::HandoffSubcommand) -> Result<()> {
    match command {
        super::HandoffSubcommand::Generate {
            agent,
            reason,
            ctx,
            json,
        } => {
            let cwd = std::env::current_dir()?;
            let (config, config_path) = TuttiConfig::load(&cwd)?;
            config.validate()?;
            let project_root = config_path.parent().ok_or_else(|| {
                TuttiError::ConfigValidation("could not determine workspace root".to_string())
            })?;

            let reason = reason.unwrap_or_else(|| "manual".to_string());
            let generated = generate_packet_for_agent(&config, project_root, &agent, ctx, &reason)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&generated)?);
            } else {
                println!("Generated handoff packet:");
                println!("  agent: {}", generated.agent);
                println!("  packet: {}", generated.packet);
                println!("  reason: {}", generated.reason);
                if let Some(ctx) = generated.ctx_pct {
                    println!("  ctx: {}%", ctx);
                }
            }
            let _ = state::append_control_event(
                project_root,
                &ControlEvent {
                    event: "handoff.generated".to_string(),
                    workspace: config.workspace.name.clone(),
                    agent: Some(agent),
                    timestamp: Utc::now(),
                    correlation_id: format!("handoff-{}", Utc::now().timestamp_millis()),
                    data: Some(serde_json::json!({
                        "packet": generated.packet,
                        "reason": generated.reason,
                        "ctx_pct": generated.ctx_pct
                    })),
                },
            );
            Ok(())
        }
        super::HandoffSubcommand::Apply { agent, packet } => {
            let cwd = std::env::current_dir()?;
            let (config, config_path) = TuttiConfig::load(&cwd)?;
            config.validate()?;
            let project_root = config_path.parent().ok_or_else(|| {
                TuttiError::ConfigValidation("could not determine workspace root".to_string())
            })?;

            ensure_agent_exists(&config, &agent)?;
            let session = TmuxSession::session_name(&config.workspace.name, &agent);
            if !TmuxSession::session_exists(&session) {
                return Err(TuttiError::AgentNotRunning(agent));
            }

            let packet_path = if let Some(path) = packet {
                resolve_packet_path(project_root, &path)
            } else {
                latest_handoff_packet(project_root, &agent)?
            };
            let content = std::fs::read_to_string(&packet_path)?;
            let prompt = format!(
                "Apply this Tutti handoff packet and continue execution.\n\
                 Packet: {}\n\
                 \n\
                 --- BEGIN HANDOFF ---\n\
                 {}\n\
                 --- END HANDOFF ---\n\
                 \n\
                 First, summarize your execution plan from the packet. Then continue.",
                packet_path.display(),
                content
            );
            TmuxSession::send_text(&session, &prompt)?;
            println!(
                "Applied handoff packet to {agent}: {}",
                packet_path.display()
            );
            let _ = state::append_control_event(
                project_root,
                &ControlEvent {
                    event: "handoff.applied".to_string(),
                    workspace: config.workspace.name.clone(),
                    agent: Some(agent),
                    timestamp: Utc::now(),
                    correlation_id: format!("handoff-{}", Utc::now().timestamp_millis()),
                    data: Some(serde_json::json!({
                        "packet": packet_path.display().to_string(),
                        "session_name": session
                    })),
                },
            );
            Ok(())
        }
        super::HandoffSubcommand::List { agent, limit, json } => {
            let cwd = std::env::current_dir()?;
            let (_config, config_path) = TuttiConfig::load(&cwd)?;
            let project_root = config_path.parent().ok_or_else(|| {
                TuttiError::ConfigValidation("could not determine workspace root".to_string())
            })?;
            let packets = list_handoff_packets(project_root, agent.as_deref(), limit)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&packets)?);
                return Ok(());
            }
            if packets.is_empty() {
                println!("No handoff packets found.");
                return Ok(());
            }

            let mut table = Table::new();
            table.load_preset(UTF8_BORDERS_ONLY);
            table.set_header(vec!["Agent", "Packet", "Modified"]);
            for packet in &packets {
                table.add_row(vec![
                    packet.agent.clone(),
                    packet.path.clone(),
                    packet.modified.clone(),
                ]);
            }
            println!("{table}");
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct HandoffPacketListItem {
    pub agent: String,
    pub path: String,
    pub modified: String,
}

pub fn auto_handoff_trigger_pct(config: &TuttiConfig) -> Option<u8> {
    let handoff = config.handoff.as_ref()?;
    if !handoff.auto {
        return None;
    }
    let threshold = handoff.threshold.clamp(0.0, 1.0);
    let trigger = (100.0 - threshold * 100.0).round();
    Some(trigger.clamp(0.0, 100.0) as u8)
}

pub fn should_auto_generate(config: &TuttiConfig, ctx_pct: u8) -> bool {
    let Some(trigger) = auto_handoff_trigger_pct(config) else {
        return false;
    };
    ctx_pct >= trigger
}

pub fn generate_packet_for_agent(
    config: &TuttiConfig,
    project_root: &Path,
    agent: &str,
    ctx_pct: Option<u8>,
    reason: &str,
) -> Result<HandoffGenerateOutput> {
    ensure_agent_exists(config, agent)?;
    state::ensure_tutti_dir(project_root)?;

    let session = TmuxSession::session_name(&config.workspace.name, agent);
    let output = if TmuxSession::session_exists(&session) {
        TmuxSession::capture_pane(&session, DEFAULT_CAPTURE_LINES).unwrap_or_default()
    } else {
        String::new()
    };
    let recent_lines = extract_recent_non_empty_lines(&output, 40);
    let active_task = recent_lines
        .last()
        .cloned()
        .unwrap_or_else(|| "(no recent output)".to_string());

    let work_root = resolve_agent_work_root(project_root, agent)?;
    let changed_files = collect_changed_files(&work_root, 25);
    let verify_summary = state::load_verify_last_summary(project_root)?;

    let timestamp = Utc::now();
    let filename = format!("{}-handoff-{}.md", agent, timestamp.format("%Y%m%d-%H%M%S"));
    let packet_path = handoff_dir(project_root).join(filename);
    let content = render_packet(&PacketRenderInput {
        config,
        agent,
        reason,
        ctx_pct,
        active_task: &active_task,
        changed_files: &changed_files,
        verify_summary: verify_summary.as_ref(),
        recent_output: &recent_lines,
    });
    std::fs::write(&packet_path, content)?;

    Ok(HandoffGenerateOutput {
        agent: agent.to_string(),
        packet: packet_path.display().to_string(),
        reason: reason.to_string(),
        ctx_pct,
    })
}

pub fn generated_recently(project_root: &Path, agent: &str, within: Duration) -> Result<bool> {
    let Some(path) = latest_handoff_packet_optional(project_root, agent)? else {
        return Ok(false);
    };
    let metadata = std::fs::metadata(path)?;
    let modified = metadata.modified()?;
    let age = modified
        .elapsed()
        .unwrap_or_else(|_| Duration::from_secs(0));
    Ok(age <= within)
}

fn ensure_agent_exists(config: &TuttiConfig, agent: &str) -> Result<()> {
    if config.agents.iter().any(|a| a.name == agent) {
        Ok(())
    } else {
        Err(TuttiError::AgentNotFound(agent.to_string()))
    }
}

fn resolve_packet_path(project_root: &Path, packet: &Path) -> PathBuf {
    if packet.is_absolute() {
        packet.to_path_buf()
    } else {
        project_root.join(packet)
    }
}

fn latest_handoff_packet(project_root: &Path, agent: &str) -> Result<PathBuf> {
    latest_handoff_packet_optional(project_root, agent)?.ok_or_else(|| {
        TuttiError::ConfigValidation(format!("no handoff packet found for agent '{}'", agent))
    })
}

fn latest_handoff_packet_optional(project_root: &Path, agent: &str) -> Result<Option<PathBuf>> {
    let dir = handoff_dir(project_root);
    if !dir.exists() {
        return Ok(None);
    }
    let prefix = format!("{agent}-");
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .filter(|path| {
            path.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| name.starts_with(&prefix))
        })
        .collect();
    entries.sort();
    Ok(entries.pop())
}

fn list_handoff_packets(
    project_root: &Path,
    agent_filter: Option<&str>,
    limit: usize,
) -> Result<Vec<HandoffPacketListItem>> {
    let dir = handoff_dir(project_root);
    if !dir.exists() {
        return Ok(vec![]);
    }

    let mut entries = Vec::<(std::time::SystemTime, PathBuf)>::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_none_or(|ext| ext != "md") {
            continue;
        }
        if let Some(agent) = agent_filter {
            let prefix = format!("{agent}-");
            let matches = path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|name| name.starts_with(&prefix));
            if !matches {
                continue;
            }
        }
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        entries.push((modified, path));
    }

    entries.sort_by(|(a, _), (b, _)| b.cmp(a));
    let mut out = Vec::new();
    for (_, path) in entries.into_iter().take(limit.max(1)) {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();
        let modified = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .map(|ts| chrono::DateTime::<chrono::Utc>::from(ts).to_rfc3339())
            .unwrap_or_else(|_| "--".to_string());
        let parsed_agent = parse_agent_from_filename(&name).unwrap_or_else(|| "--".to_string());
        out.push(HandoffPacketListItem {
            agent: parsed_agent,
            path: path.display().to_string(),
            modified,
        });
    }
    Ok(out)
}

fn parse_agent_from_filename(name: &str) -> Option<String> {
    if let Some((agent, _)) = name.split_once("-handoff-") {
        return Some(agent.to_string());
    }
    if let Some((agent, _)) = name.split_once("-emergency-") {
        return Some(agent.to_string());
    }
    name.split_once('-').map(|(agent, _)| agent.to_string())
}

fn handoff_dir(project_root: &Path) -> PathBuf {
    project_root.join(".tutti").join("handoffs")
}

fn resolve_agent_work_root(project_root: &Path, agent: &str) -> Result<PathBuf> {
    if let Some(state) = state::load_agent_state(project_root, agent)?
        && let Some(path) = state.worktree_path
        && path.exists()
    {
        return Ok(path);
    }
    let candidate = project_root.join(".tutti").join("worktrees").join(agent);
    if candidate.exists() {
        return Ok(candidate);
    }
    Ok(project_root.to_path_buf())
}

fn collect_changed_files(cwd: &Path, limit: usize) -> Vec<String> {
    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(cwd)
        .output();

    let Ok(output) = output else {
        return vec![];
    };
    if !output.status.success() {
        return vec![];
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            if line.len() >= 4 {
                Some(line[3..].trim().to_string())
            } else {
                None
            }
        })
        .take(limit.max(1))
        .collect()
}

struct PacketRenderInput<'a> {
    config: &'a TuttiConfig,
    agent: &'a str,
    reason: &'a str,
    ctx_pct: Option<u8>,
    active_task: &'a str,
    changed_files: &'a [String],
    verify_summary: Option<&'a state::VerifyLastSummary>,
    recent_output: &'a [String],
}

fn render_packet(input: &PacketRenderInput<'_>) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Tutti Handoff: {}\n\n", input.agent));
    out.push_str("## Metadata\n");
    out.push_str(&format!("- Workspace: {}\n", input.config.workspace.name));
    out.push_str(&format!("- Agent: {}\n", input.agent));
    out.push_str(&format!("- Generated: {}\n", Utc::now().to_rfc3339()));
    out.push_str(&format!("- Trigger: {}\n", input.reason));
    out.push_str(&format!(
        "- CTX: {}\n\n",
        input
            .ctx_pct
            .map(|v| format!("{v}%"))
            .unwrap_or_else(|| "--".to_string())
    ));

    out.push_str("## Active Task Guess\n");
    out.push_str(&format!("- {}\n\n", input.active_task));

    out.push_str("## Changed Files\n");
    if input.changed_files.is_empty() {
        out.push_str("- (none detected)\n\n");
    } else {
        for file in input.changed_files {
            out.push_str(&format!("- {}\n", file));
        }
        out.push('\n');
    }

    out.push_str("## Verify Status\n");
    if let Some(summary) = input.verify_summary {
        out.push_str(&format!("- Workflow: {}\n", summary.workflow_name));
        out.push_str(&format!("- Success: {}\n", summary.success));
        out.push_str(&format!("- Strict: {}\n", summary.strict));
        out.push_str(&format!(
            "- Failed Steps: {}\n",
            if summary.failed_steps.is_empty() {
                "--".to_string()
            } else {
                summary
                    .failed_steps
                    .iter()
                    .map(std::string::ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ));
        out.push_str(&format!(
            "- Timestamp: {}\n\n",
            summary.timestamp.to_rfc3339()
        ));
    } else {
        out.push_str("- (no verify summary found)\n\n");
    }

    out.push_str("## Recent Output\n\n```text\n");
    if input.recent_output.is_empty() {
        out.push_str("(no output captured)\n");
    } else {
        for line in input.recent_output {
            out.push_str(line);
            out.push('\n');
        }
    }
    out.push_str("```\n\n");

    out.push_str("## Next Prompt Template\n");
    out.push_str("Use this packet to resume work. First restate the plan, then continue from the latest task.\n");
    out
}

fn extract_recent_non_empty_lines(output: &str, max: usize) -> Vec<String> {
    let mut lines = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    if lines.len() > max {
        lines = lines.split_off(lines.len() - max);
    }
    lines
}

pub fn auto_handoff_watch_tick(
    config: &TuttiConfig,
    project_root: &Path,
    snapshots: &[super::snapshot::AgentSnapshot],
    cooldown: Duration,
    last_generated: &mut std::collections::HashMap<String, Instant>,
) -> Result<Option<String>> {
    let mut generated = Vec::<String>::new();
    for snapshot in snapshots {
        if !snapshot.running {
            continue;
        }
        let Some(ctx_pct) = snapshot.ctx_pct else {
            continue;
        };
        if !should_auto_generate(config, ctx_pct) {
            continue;
        }

        let recent_in_memory = last_generated
            .get(&snapshot.agent_name)
            .is_some_and(|last| last.elapsed() <= cooldown);
        if recent_in_memory || generated_recently(project_root, &snapshot.agent_name, cooldown)? {
            continue;
        }

        let generated_packet = generate_packet_for_agent(
            config,
            project_root,
            &snapshot.agent_name,
            Some(ctx_pct),
            "auto_ctx_threshold",
        )?;
        last_generated.insert(snapshot.agent_name.clone(), Instant::now());
        generated.push(format!(
            "{} ({})",
            snapshot.agent_name, generated_packet.packet
        ));
    }

    if generated.is_empty() {
        Ok(None)
    } else {
        Ok(Some(format!("handoff generated: {}", generated.join(", "))))
    }
}

pub fn auto_handoff_post_launch(
    config: &TuttiConfig,
    project_root: &Path,
) -> Result<Option<String>> {
    let snapshots = super::snapshot::gather_workspace_snapshots(config, project_root);
    let mut memo = std::collections::HashMap::new();
    auto_handoff_watch_tick(
        config,
        project_root,
        &snapshots,
        Duration::from_secs(300),
        &mut memo,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentConfig, DefaultsConfig, HandoffConfig, WorkspaceConfig};
    use std::collections::HashMap;

    fn sample_config(auto: bool, threshold: f64) -> TuttiConfig {
        TuttiConfig {
            workspace: WorkspaceConfig {
                name: "ws".to_string(),
                description: None,
                env: None,
                auth: None,
            },
            defaults: DefaultsConfig {
                worktree: true,
                runtime: Some("claude-code".to_string()),
            },
            launch: None,
            agents: vec![AgentConfig {
                name: "backend".to_string(),
                runtime: None,
                scope: None,
                prompt: None,
                depends_on: vec![],
                worktree: None,
                branch: None,
                persistent: false,
                env: HashMap::new(),
            }],
            tool_packs: vec![],
            workflows: vec![],
            hooks: vec![],
            handoff: Some(HandoffConfig {
                auto,
                threshold,
                include: vec![],
            }),
            observe: None,
        }
    }

    #[test]
    fn auto_handoff_trigger_pct_maps_threshold() {
        let config = sample_config(true, 0.2);
        assert_eq!(auto_handoff_trigger_pct(&config), Some(80));
        let config = sample_config(true, 0.5);
        assert_eq!(auto_handoff_trigger_pct(&config), Some(50));
    }

    #[test]
    fn should_auto_generate_respects_toggle_and_threshold() {
        let on = sample_config(true, 0.2);
        assert!(should_auto_generate(&on, 80));
        assert!(should_auto_generate(&on, 92));
        assert!(!should_auto_generate(&on, 70));

        let off = sample_config(false, 0.2);
        assert!(!should_auto_generate(&off, 99));
    }

    #[test]
    fn extract_recent_non_empty_lines_trims_and_limits() {
        let output = "\nline1\n\n line2 \nline3\n";
        let lines = extract_recent_non_empty_lines(output, 2);
        assert_eq!(lines, vec!["line2".to_string(), "line3".to_string()]);
    }
}
