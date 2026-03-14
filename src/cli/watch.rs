use crate::config::{ResilienceConfig, TuttiConfig};
use crate::error::Result;
use crate::session::TmuxSession;
use crate::state;
use chrono::Utc;
use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::{Frame, Terminal};
use std::collections::HashMap;
use std::io::{self, Read, Stdout, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use super::snapshot::AgentSnapshot;
use super::snapshot::gather_workspace_snapshots_with_selected_tail;

const WATCH_TABLE_HEADERS: [&str; 6] = ["", "Agent", "Runtime", "Status", "PLAN", "CTX"];

#[derive(Clone)]
struct AgentPlanCell {
    plan_display: String,
}

pub fn run(interval: u64, restart_persistent: bool) -> Result<()> {
    crate::session::tmux::check_tmux()?;

    let cwd = std::env::current_dir()?;
    let (config, config_path) = TuttiConfig::load(&cwd)?;
    let project_root = config_path.parent().unwrap();

    if config.agents.is_empty() {
        println!("No agents defined in tutti.toml");
        return Ok(());
    }

    let mut ui = WatchTerminal::new()?;

    let agent_names: Vec<String> = config.agents.iter().map(|a| a.name.clone()).collect();
    let mut selected: usize = 0;
    let peek_lines: u32 = 20;
    let log_capture_lines: u32 = 200;
    let refresh_interval = Duration::from_secs(interval.max(1));
    let restart_cooldown = Duration::from_secs(20);
    let auth_recovery_cooldown = Duration::from_secs(90);
    let plan_cache = build_plan_cache(&config);
    let global = crate::config::GlobalConfig::load().ok();
    let failure_policy = SessionFailurePolicy {
        restart_persistent,
        restart_cooldown,
        auth_recovery_cooldown,
        resilience: global.as_ref().and_then(|g| g.resilience.as_ref()),
        project_root,
    };
    let mut failure_state = SessionFailureState::default();
    let mut last_logged_snapshot = HashMap::<String, String>::new();
    let mut last_handoff_generated = HashMap::<String, Instant>::new();
    let mut last_event: Option<String> = None;
    let handoff_cooldown = Duration::from_secs(300);

    loop {
        let selected_name = &agent_names[selected];
        let snapshots = gather_workspace_snapshots_with_selected_tail(
            &config,
            project_root,
            Some(selected_name),
            peek_lines,
        );

        ui.draw(|frame| {
            render_watch(
                frame,
                &config.workspace.name,
                interval.max(1),
                &snapshots,
                selected,
                &plan_cache,
                last_event.as_deref(),
            )
        })?;

        // Update state files
        for snapshot in &snapshots {
            let _ = state::update_status_if_exists(
                project_root,
                &snapshot.agent_name,
                &snapshot.status_raw,
            );
        }
        if let Err(e) = capture_tick_logs(
            project_root,
            &snapshots,
            &mut last_logged_snapshot,
            log_capture_lines,
        ) {
            last_event = Some(format!("log capture warning: {e}"));
        }
        if let Some(event) = super::handoff::auto_handoff_watch_tick(
            &config,
            project_root,
            &snapshots,
            handoff_cooldown,
            &mut last_handoff_generated,
        )? {
            last_event = Some(event);
        }
        if let Some(event) = detect_and_handle_session_failures(
            &config,
            &snapshots,
            &failure_policy,
            &mut failure_state,
            &mut ui,
        )? {
            last_event = Some(event);
        }

        // Poll for key input during the sleep interval.
        let deadline = std::time::Instant::now() + refresh_interval;
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            let wait = remaining.min(Duration::from_millis(50));
            if event::poll(wait)?
                && let Event::Key(key) = event::read()?
            {
                if key.kind != KeyEventKind::Press {
                    continue;
                }

                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
                        selected = (selected + 1) % agent_names.len();
                        break; // refresh immediately
                    }
                    KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
                        selected = selected.checked_sub(1).unwrap_or(agent_names.len() - 1);
                        break; // refresh immediately
                    }
                    KeyCode::Enter | KeyCode::Char('a') | KeyCode::Char('A') => {
                        ui.suspend()?;
                        let attach_result =
                            attach_selected(&config.workspace.name, &agent_names, selected);
                        ui.resume()?;
                        attach_result?;
                        break;
                    }
                    KeyCode::Char('p') | KeyCode::Char('P') => {
                        ui.suspend()?;
                        let peek_result =
                            show_full_peek(&config.workspace.name, &agent_names, selected);
                        ui.resume()?;
                        peek_result?;
                        break;
                    }
                    _ => {}
                }
            }
        }
    }
}

fn render_watch(
    frame: &mut Frame<'_>,
    workspace_name: &str,
    refresh_secs: u64,
    snapshots: &[AgentSnapshot],
    selected: usize,
    plan_cache: &HashMap<String, AgentPlanCell>,
    last_event: Option<&str>,
) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(45), Constraint::Percentage(55)])
        .split(frame.area());

    let mut title = format!(
        " tutti: {} (refresh {}s)  arrows/jk move  Enter attach  p peek  q/Esc quit ",
        workspace_name, refresh_secs
    );
    if let Some(event) = last_event {
        title.push_str("  |  ");
        title.push_str(event);
    }

    let rows = snapshots.iter().enumerate().map(|(idx, snapshot)| {
        let marker = if idx == selected { "▸" } else { " " };
        let plan_display = plan_cache
            .get(&snapshot.agent_name)
            .map(|u| u.plan_display.clone())
            .unwrap_or_else(|| "--".to_string());
        let ctx_display = snapshot
            .ctx_pct
            .map(|pct| format!("{pct}%"))
            .unwrap_or_else(|| "--".to_string());
        let status_cell = Cell::from(Span::styled(
            snapshot.status_raw.clone(),
            status_style(&snapshot.status_raw),
        ));
        Row::new(vec![
            Cell::from(marker.to_string()),
            Cell::from(snapshot.agent_name.clone()),
            Cell::from(snapshot.runtime.clone()),
            status_cell,
            Cell::from(plan_display),
            Cell::from(ctx_display),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(18),
            Constraint::Length(14),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Min(6),
        ],
    )
    .header(Row::new(WATCH_TABLE_HEADERS).style(Style::default().add_modifier(Modifier::BOLD)))
    .block(Block::default().borders(Borders::ALL).title(title));
    frame.render_widget(table, vertical[0]);

    let selected_snapshot = snapshots.get(selected);
    let (tail_title, tail_lines) = if let Some(snapshot) = selected_snapshot {
        let title = format!(" {} ({}) ", snapshot.agent_name, snapshot.status_raw);
        let lines = if !snapshot.running {
            vec![Line::from("(not running)")]
        } else if let Some(err) = &snapshot.tail_error {
            vec![Line::from(err.clone())]
        } else if let Some(lines) = &snapshot.tail_lines {
            if lines.is_empty() {
                vec![Line::from("")]
            } else {
                lines.iter().map(|line| Line::from(line.as_str())).collect()
            }
        } else {
            vec![Line::from("")]
        };
        (title, lines)
    } else {
        (" output ".to_string(), vec![Line::from("(no selection)")])
    };

    let tail =
        Paragraph::new(tail_lines).block(Block::default().borders(Borders::ALL).title(tail_title));
    frame.render_widget(tail, vertical[1]);
}

fn build_plan_cache(config: &TuttiConfig) -> HashMap<String, AgentPlanCell> {
    let global = crate::config::GlobalConfig::load().ok();
    build_plan_cache_with_global(config, global.as_ref())
}

fn build_plan_cache_with_global(
    config: &TuttiConfig,
    global: Option<&crate::config::GlobalConfig>,
) -> HashMap<String, AgentPlanCell> {
    let plan_display = global
        .map(|g| resolve_workspace_plan_label(config, g))
        .unwrap_or_else(|| "--".to_string());

    let mut cache = HashMap::<String, AgentPlanCell>::new();
    for agent in &config.agents {
        cache.insert(
            agent.name.clone(),
            AgentPlanCell {
                plan_display: plan_display.clone(),
            },
        );
    }
    cache
}

fn resolve_workspace_plan_label(
    config: &TuttiConfig,
    global: &crate::config::GlobalConfig,
) -> String {
    let profile_name = config
        .workspace
        .auth
        .as_ref()
        .and_then(|a| a.default_profile.as_deref());

    if let Some(profile_name) = profile_name
        && let Some(profile) = global.profiles.iter().find(|p| p.name == profile_name)
    {
        return format_plan_label(profile.plan.as_deref());
    }

    "--".to_string()
}

fn format_plan_label(plan: Option<&str>) -> String {
    match plan.map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) => p.to_uppercase(),
        None => "--".to_string(),
    }
}

#[derive(Default)]
struct SessionFailureState {
    previous_running: HashMap<String, bool>,
    last_restart_attempt: HashMap<String, Instant>,
    last_auth_recovery_attempt: HashMap<String, Instant>,
}

struct SessionFailurePolicy<'a> {
    restart_persistent: bool,
    restart_cooldown: Duration,
    auth_recovery_cooldown: Duration,
    resilience: Option<&'a ResilienceConfig>,
    project_root: &'a Path,
}

fn detect_and_handle_session_failures(
    config: &TuttiConfig,
    snapshots: &[AgentSnapshot],
    policy: &SessionFailurePolicy<'_>,
    state: &mut SessionFailureState,
    ui: &mut WatchTerminal,
) -> Result<Option<String>> {
    let mut latest_event = None;
    let auth_rotation_enabled = profile_rotation_enabled(policy.resilience);
    let strategy = rotation_strategy_label(policy.resilience).unwrap_or("rotate_profile");

    for snapshot in snapshots {
        let was_running = state
            .previous_running
            .get(&snapshot.agent_name)
            .copied()
            .unwrap_or(snapshot.running);
        let now_running = snapshot.running;

        if was_running && !now_running {
            if policy.restart_persistent && is_persistent_agent(config, &snapshot.agent_name) {
                let can_attempt = state
                    .last_restart_attempt
                    .get(&snapshot.agent_name)
                    .is_none_or(|last| last.elapsed() >= policy.restart_cooldown);

                if can_attempt {
                    state
                        .last_restart_attempt
                        .insert(snapshot.agent_name.clone(), Instant::now());
                    let _ = emit_recovery_event(
                        policy.project_root,
                        &config.workspace.name,
                        &snapshot.agent_name,
                        "agent.recovery_attempted",
                        "crash_restart",
                        None,
                    );
                    ui.suspend()?;
                    let restart_result = restart_agent(config, &snapshot.agent_name);
                    ui.resume()?;

                    latest_event = Some(match restart_result {
                        Ok(_) => {
                            let _ = emit_recovery_event(
                                policy.project_root,
                                &config.workspace.name,
                                &snapshot.agent_name,
                                "agent.recovery_succeeded",
                                "crash_restart",
                                None,
                            );
                            format!("restarted {}", snapshot.agent_name)
                        }
                        Err(e) => {
                            let _ = emit_recovery_event(
                                policy.project_root,
                                &config.workspace.name,
                                &snapshot.agent_name,
                                "agent.recovery_failed",
                                "crash_restart",
                                Some(&e.to_string()),
                            );
                            format!("restart failed for {}: {e}", snapshot.agent_name)
                        }
                    });
                } else {
                    latest_event = Some(format!(
                        "{} crashed (restart cooldown active)",
                        snapshot.agent_name
                    ));
                }
            } else {
                latest_event = Some(format!("{} crashed", snapshot.agent_name));
            }
        } else if now_running
            && auth_rotation_enabled
            && is_auth_failed_status(&snapshot.status_raw)
        {
            let can_attempt = state
                .last_auth_recovery_attempt
                .get(&snapshot.agent_name)
                .is_none_or(|last| last.elapsed() >= policy.auth_recovery_cooldown);
            if can_attempt {
                state
                    .last_auth_recovery_attempt
                    .insert(snapshot.agent_name.clone(), Instant::now());
                let reason = snapshot.status_raw.clone();
                let _ = emit_recovery_event(
                    policy.project_root,
                    &config.workspace.name,
                    &snapshot.agent_name,
                    "agent.recovery_attempted",
                    strategy,
                    Some(reason.as_str()),
                );
                ui.suspend()?;
                let recovery_result = restart_agent(config, &snapshot.agent_name);
                ui.resume()?;
                latest_event = Some(match recovery_result {
                    Ok(_) => {
                        let _ = emit_recovery_event(
                            policy.project_root,
                            &config.workspace.name,
                            &snapshot.agent_name,
                            "agent.recovery_succeeded",
                            strategy,
                            Some(reason.as_str()),
                        );
                        format!("auth-recovered {} via {}", snapshot.agent_name, strategy)
                    }
                    Err(e) => {
                        let _ = emit_recovery_event(
                            policy.project_root,
                            &config.workspace.name,
                            &snapshot.agent_name,
                            "agent.recovery_failed",
                            strategy,
                            Some(&e.to_string()),
                        );
                        format!("auth recovery failed for {}: {e}", snapshot.agent_name)
                    }
                });
            } else {
                latest_event = Some(format!(
                    "{} auth-failed (recovery cooldown active)",
                    snapshot.agent_name
                ));
            }
        }

        state
            .previous_running
            .insert(snapshot.agent_name.clone(), now_running);
    }

    Ok(latest_event)
}

fn restart_agent(config: &TuttiConfig, agent_name: &str) -> Result<()> {
    let session = TmuxSession::session_name(&config.workspace.name, agent_name);
    if TmuxSession::session_exists(&session) {
        TmuxSession::kill_session(&session)?;
    }
    super::up::run(Some(agent_name), None, false, false, None, None)?;
    Ok(())
}

fn is_auth_failed_status(status_raw: &str) -> bool {
    status_raw
        .trim()
        .to_ascii_lowercase()
        .starts_with("auth failed")
}

fn profile_rotation_enabled(resilience: Option<&ResilienceConfig>) -> bool {
    let Some(resilience) = resilience else {
        return false;
    };
    strategy_requests_rotation(resilience.provider_down_strategy.as_deref())
        || strategy_requests_rotation(resilience.rate_limit_strategy.as_deref())
}

fn rotation_strategy_label(resilience: Option<&ResilienceConfig>) -> Option<&str> {
    let resilience = resilience?;
    if strategy_requests_rotation(resilience.rate_limit_strategy.as_deref()) {
        return resilience.rate_limit_strategy.as_deref();
    }
    if strategy_requests_rotation(resilience.provider_down_strategy.as_deref()) {
        return resilience.provider_down_strategy.as_deref();
    }
    None
}

fn strategy_requests_rotation(strategy: Option<&str>) -> bool {
    strategy.is_some_and(|s| {
        matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "rotate" | "rotate_profile" | "profile_rotate" | "failover" | "auto_rotate"
        )
    })
}

fn emit_recovery_event(
    project_root: &Path,
    workspace: &str,
    agent: &str,
    event: &str,
    strategy: &str,
    reason: Option<&str>,
) -> Result<()> {
    let data = match reason {
        Some(value) => serde_json::json!({
            "strategy": strategy,
            "reason": value
        }),
        None => serde_json::json!({
            "strategy": strategy
        }),
    };
    state::append_control_event(
        project_root,
        &state::ControlEvent {
            event: event.to_string(),
            workspace: workspace.to_string(),
            agent: Some(agent.to_string()),
            timestamp: Utc::now(),
            correlation_id: format!(
                "watch-{}-{}-{}",
                event,
                agent,
                Utc::now().timestamp_millis()
            ),
            data: Some(data),
        },
    )
}

fn is_persistent_agent(config: &TuttiConfig, agent_name: &str) -> bool {
    config
        .agents
        .iter()
        .find(|agent| agent.name == agent_name)
        .is_some_and(|agent| agent.persistent)
}

fn status_style(raw: &str) -> Style {
    match raw {
        "Working" => Style::default().fg(ratatui::style::Color::Green),
        "Idle" => Style::default().fg(ratatui::style::Color::Yellow),
        "Errored" => Style::default().fg(ratatui::style::Color::Red),
        "Stopped" => Style::default().fg(ratatui::style::Color::DarkGray),
        s if s.starts_with("Auth Failed") => Style::default()
            .fg(ratatui::style::Color::Red)
            .add_modifier(Modifier::BOLD),
        _ => Style::default().fg(ratatui::style::Color::Gray),
    }
}

fn attach_selected(workspace_name: &str, agent_names: &[String], selected: usize) -> Result<()> {
    let agent = &agent_names[selected];
    let session = TmuxSession::session_name(workspace_name, agent);

    if !TmuxSession::session_exists(&session) {
        return Ok(());
    }

    let others: Vec<&str> = agent_names
        .iter()
        .filter(|n| n.as_str() != agent.as_str())
        .map(|n| n.as_str())
        .collect();
    let switch_hint = if others.is_empty() {
        String::new()
    } else {
        format!(" ── tt attach {}", others[0])
    };
    let bar = format!(
        " tutti: {} ({}) ── Ctrl+b d to detach{}",
        agent, workspace_name, switch_hint
    );
    let _ = TmuxSession::set_status_bar(&session, &bar);
    let _ = TmuxSession::attach_session(&session);

    Ok(())
}

fn show_full_peek(workspace_name: &str, agent_names: &[String], selected: usize) -> Result<()> {
    print!("\x1B[2J\x1B[H");
    let agent = &agent_names[selected];
    let session = TmuxSession::session_name(workspace_name, agent);
    println!("─── {} (full peek) ───\n", agent);
    if TmuxSession::session_exists(&session) {
        match TmuxSession::capture_pane(&session, 100) {
            Ok(output) => println!("{output}"),
            Err(_) => println!("(could not read output)"),
        }
    } else {
        println!("(not running)");
    }
    println!("\nPress any key to return to watch...");
    let _ = io::stdin().read(&mut [0u8]);
    Ok(())
}

fn capture_tick_logs(
    project_root: &Path,
    snapshots: &[AgentSnapshot],
    last_logged_snapshot: &mut HashMap<String, String>,
    lines: u32,
) -> Result<()> {
    let log_dir = project_root.join(".tutti").join("logs");
    std::fs::create_dir_all(&log_dir)?;

    for snapshot in snapshots {
        if !snapshot.running {
            continue;
        }

        let pane_output = match TmuxSession::capture_pane(&snapshot.session_name, lines) {
            Ok(output) => output,
            Err(_) => continue,
        };

        if last_logged_snapshot
            .get(&snapshot.agent_name)
            .is_some_and(|prev| prev == &pane_output)
        {
            continue;
        }

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_dir.join(format!("{}.log", snapshot.agent_name)))?;

        writeln!(
            file,
            "\n--- {} [{}] ---",
            Utc::now().to_rfc3339(),
            snapshot.status_raw
        )?;
        write!(file, "{pane_output}")?;
        if !pane_output.ends_with('\n') {
            writeln!(file)?;
        }

        last_logged_snapshot.insert(snapshot.agent_name.clone(), pane_output);
    }

    Ok(())
}

struct WatchTerminal {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    active: bool,
}

impl WatchTerminal {
    fn new() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, cursor::Hide)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;
        Ok(Self {
            terminal,
            active: true,
        })
    }

    fn draw<F>(&mut self, render: F) -> Result<()>
    where
        F: FnOnce(&mut Frame<'_>),
    {
        self.terminal.draw(render)?;
        Ok(())
    }

    fn suspend(&mut self) -> Result<()> {
        if self.active {
            disable_raw_mode()?;
            execute!(
                self.terminal.backend_mut(),
                LeaveAlternateScreen,
                cursor::Show
            )?;
            self.active = false;
        }
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        if !self.active {
            enable_raw_mode()?;
            execute!(
                self.terminal.backend_mut(),
                EnterAlternateScreen,
                cursor::Hide
            )?;
            self.terminal.clear()?;
            self.active = true;
        }
        Ok(())
    }
}

impl Drop for WatchTerminal {
    fn drop(&mut self) {
        if self.active {
            let _ = disable_raw_mode();
            let _ = execute!(
                self.terminal.backend_mut(),
                LeaveAlternateScreen,
                cursor::Show
            );
            self.active = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AgentConfig, DefaultsConfig, GlobalConfig, ProfileConfig, TuttiConfig, WorkspaceAuth,
        WorkspaceConfig,
    };
    use std::collections::HashMap;

    fn sample_tutti_config(default_profile: Option<&str>, agents: &[&str]) -> TuttiConfig {
        TuttiConfig {
            workspace: WorkspaceConfig {
                name: "ws".to_string(),
                description: None,
                env: None,
                auth: Some(WorkspaceAuth {
                    default_profile: default_profile.map(|s| s.to_string()),
                }),
            },
            defaults: DefaultsConfig::default(),
            launch: None,
            agents: agents
                .iter()
                .map(|name| AgentConfig {
                    name: (*name).to_string(),
                    runtime: None,
                    scope: None,
                    prompt: None,
                    depends_on: vec![],
                    worktree: None,
                    fresh_worktree: None,
                    branch: None,
                    persistent: false,
                    env: HashMap::new(),
                })
                .collect(),
            tool_packs: vec![],
            workflows: vec![],
            hooks: vec![],
            handoff: None,
            observe: None,
            budget: None,
        }
    }

    fn sample_profile(name: &str, plan: Option<&str>) -> ProfileConfig {
        ProfileConfig {
            name: name.to_string(),
            provider: "openai".to_string(),
            command: "codex".to_string(),
            max_concurrent: None,
            monthly_budget: None,
            priority: None,
            plan: plan.map(|s| s.to_string()),
            reset_day: None,
            weekly_hours: None,
        }
    }

    #[test]
    fn resolve_workspace_plan_label_uses_default_profile_plan() {
        let config = sample_tutti_config(Some("api-main"), &[]);
        let global = GlobalConfig {
            user: None,
            profiles: vec![sample_profile("api-main", Some("api"))],
            registered_workspaces: vec![],
            dashboard: None,
            resilience: None,
            permissions: None,
        };
        assert_eq!(resolve_workspace_plan_label(&config, &global), "API");
    }

    #[test]
    fn resolve_workspace_plan_label_returns_dash_when_unresolved() {
        let config = sample_tutti_config(Some("missing"), &[]);
        let global = GlobalConfig::default();
        assert_eq!(resolve_workspace_plan_label(&config, &global), "--");
    }

    #[test]
    fn format_plan_label_uppercases_and_defaults() {
        assert_eq!(format_plan_label(Some("max")), "MAX");
        assert_eq!(format_plan_label(Some("  pro ")), "PRO");
        assert_eq!(format_plan_label(Some("  ")), "--");
        assert_eq!(format_plan_label(None), "--");
    }

    #[test]
    fn watch_header_columns_include_plan_and_ctx() {
        assert_eq!(WATCH_TABLE_HEADERS[4], "PLAN");
        assert_eq!(WATCH_TABLE_HEADERS[5], "CTX");
    }

    #[test]
    fn build_plan_cache_applies_workspace_plan_to_agents() {
        let config = sample_tutti_config(Some("api-main"), &["backend", "frontend"]);
        let global = GlobalConfig {
            user: None,
            profiles: vec![sample_profile("api-main", Some("api"))],
            registered_workspaces: vec![],
            dashboard: None,
            resilience: None,
            permissions: None,
        };

        let cache = build_plan_cache_with_global(&config, Some(&global));
        assert_eq!(
            cache.get("backend").map(|c| c.plan_display.as_str()),
            Some("API")
        );
        assert_eq!(
            cache.get("frontend").map(|c| c.plan_display.as_str()),
            Some("API")
        );
    }

    #[test]
    fn build_plan_cache_defaults_to_dash_when_global_missing() {
        let config = sample_tutti_config(Some("api-main"), &["backend"]);
        let cache = build_plan_cache_with_global(&config, None);
        assert_eq!(
            cache.get("backend").map(|c| c.plan_display.as_str()),
            Some("--")
        );
    }

    #[test]
    fn is_auth_failed_status_matches_prefix_case_insensitive() {
        assert!(is_auth_failed_status("Auth Failed: token expired"));
        assert!(is_auth_failed_status("auth failed (invalid key)"));
        assert!(!is_auth_failed_status("Working"));
    }

    #[test]
    fn strategy_requests_rotation_accepts_known_values() {
        assert!(strategy_requests_rotation(Some("rotate_profile")));
        assert!(strategy_requests_rotation(Some("failover")));
        assert!(strategy_requests_rotation(Some("AUTO_ROTATE")));
        assert!(!strategy_requests_rotation(Some("pause")));
        assert!(!strategy_requests_rotation(None));
    }

    #[test]
    fn profile_rotation_enabled_checks_both_strategy_fields() {
        let cfg = ResilienceConfig {
            provider_down_strategy: Some("pause".to_string()),
            save_state_on_failure: false,
            rate_limit_strategy: Some("rotate_profile".to_string()),
            retry_max_attempts: None,
            retry_initial_backoff_ms: None,
            retry_max_backoff_ms: None,
        };
        assert!(profile_rotation_enabled(Some(&cfg)));
    }
}
