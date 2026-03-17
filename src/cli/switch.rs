use super::snapshot::gather_workspace_snapshots;
use crate::config::{GlobalConfig, TuttiConfig};
use crate::error::Result;
use crate::session::TmuxSession;
use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use fuzzy_matcher::FuzzyMatcher;
use fuzzy_matcher::skim::SkimMatcherV2;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::text::Span;
use ratatui::widgets::{Block, Borders, Cell, Paragraph, Row, Table};
use ratatui::{Frame, Terminal};
use std::collections::HashSet;
use std::io::{self, Stdout};

#[derive(Clone)]
struct SwitchEntry {
    workspace_name: String,
    agent_name: String,
    runtime: String,
    status_raw: String,
    session_name: String,
}

impl SwitchEntry {
    fn ref_name(&self) -> String {
        format!("{}/{}", self.workspace_name, self.agent_name)
    }

    fn searchable_text(&self) -> String {
        format!(
            "{} {} {}",
            self.ref_name(),
            self.runtime,
            self.status_raw.to_lowercase()
        )
    }
}

pub fn run() -> Result<()> {
    crate::session::tmux::check_tmux()?;

    let entries = gather_running_entries()?;
    if entries.is_empty() {
        println!("No running agents found. Use `tt up` first.");
        return Ok(());
    }

    let mut ui = SwitchTerminal::new()?;
    let matcher = SkimMatcherV2::default();
    let mut query = String::new();
    let mut selected = 0usize;

    loop {
        let filtered = filter_entries(&query, &entries, &matcher);
        if selected >= filtered.len() && !filtered.is_empty() {
            selected = filtered.len() - 1;
        } else if filtered.is_empty() {
            selected = 0;
        }

        ui.draw(|frame| render_switch(frame, &entries, &filtered, selected, &query))?;

        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Press {
                continue;
            }

            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => return Ok(()),
                KeyCode::Up | KeyCode::Char('k') | KeyCode::Char('K') => {
                    if !filtered.is_empty() {
                        selected = selected.checked_sub(1).unwrap_or(filtered.len() - 1);
                    }
                }
                KeyCode::Down | KeyCode::Char('j') | KeyCode::Char('J') => {
                    if !filtered.is_empty() {
                        selected = (selected + 1) % filtered.len();
                    }
                }
                KeyCode::Backspace => {
                    query.pop();
                }
                KeyCode::Enter => {
                    if let Some(entry) = filtered.get(selected).map(|idx| &entries[*idx]) {
                        ui.suspend()?;
                        let attach_result = attach_entry(entry);
                        ui.resume()?;
                        attach_result?;
                    }
                }
                KeyCode::Char(ch) => {
                    if !key.modifiers.contains(KeyModifiers::CONTROL) {
                        query.push(ch);
                    }
                }
                _ => {}
            }
        }
    }
}

fn gather_running_entries() -> Result<Vec<SwitchEntry>> {
    let mut entries = Vec::new();
    let mut seen_sessions = HashSet::new();

    let global = GlobalConfig::load()?;
    for ws in &global.registered_workspaces {
        if let Ok((config, config_path)) = TuttiConfig::load(&ws.path) {
            let project_root = config_path.parent().unwrap();
            for snap in gather_workspace_snapshots(&config, project_root)
                .into_iter()
                .filter(|s| s.running)
            {
                if seen_sessions.insert(snap.session_name.clone()) {
                    entries.push(SwitchEntry {
                        workspace_name: snap.workspace_name,
                        agent_name: snap.agent_name,
                        runtime: snap.runtime,
                        status_raw: snap.status_raw,
                        session_name: snap.session_name,
                    });
                }
            }
        }
    }

    if let Ok(cwd) = std::env::current_dir()
        && let Ok((config, config_path)) = TuttiConfig::load(&cwd)
    {
        let project_root = config_path.parent().unwrap();
        for snap in gather_workspace_snapshots(&config, project_root)
            .into_iter()
            .filter(|s| s.running)
        {
            if seen_sessions.insert(snap.session_name.clone()) {
                entries.push(SwitchEntry {
                    workspace_name: snap.workspace_name,
                    agent_name: snap.agent_name,
                    runtime: snap.runtime,
                    status_raw: snap.status_raw,
                    session_name: snap.session_name,
                });
            }
        }
    }

    Ok(entries)
}

fn filter_entries(query: &str, entries: &[SwitchEntry], matcher: &SkimMatcherV2) -> Vec<usize> {
    if query.trim().is_empty() {
        return (0..entries.len()).collect();
    }

    let mut scored: Vec<(i64, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(idx, entry)| {
            matcher
                .fuzzy_match(&entry.searchable_text(), query)
                .map(|score| (score, idx))
        })
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    scored.into_iter().map(|(_, idx)| idx).collect()
}

fn render_switch(
    frame: &mut Frame<'_>,
    entries: &[SwitchEntry],
    filtered: &[usize],
    selected: usize,
    query: &str,
) {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5)])
        .split(frame.area());

    let query_box = Paragraph::new(format!("filter: {query}")).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" tt switch  (type to fuzzy-filter, Enter attach, q/Esc quit) "),
    );
    frame.render_widget(query_box, vertical[0]);

    if filtered.is_empty() {
        let empty = Paragraph::new("No running agents match this filter.")
            .block(Block::default().borders(Borders::ALL).title(" results "));
        frame.render_widget(empty, vertical[1]);
        return;
    }

    let rows = filtered.iter().enumerate().map(|(display_idx, entry_idx)| {
        let entry = &entries[*entry_idx];
        let marker = if display_idx == selected { "▸" } else { " " };
        Row::new(vec![
            Cell::from(marker.to_string()),
            Cell::from(entry.ref_name()),
            Cell::from(entry.runtime.clone()),
            Cell::from(Span::styled(
                entry.status_raw.clone(),
                status_style(&entry.status_raw),
            )),
        ])
    });

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(30),
            Constraint::Length(14),
            Constraint::Min(12),
        ],
    )
    .header(
        Row::new(vec!["", "Agent", "Runtime", "Status"])
            .style(Style::default().add_modifier(Modifier::BOLD)),
    )
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" running agents "),
    );
    frame.render_widget(table, vertical[1]);
}

fn status_style(raw: &str) -> Style {
    match raw.trim().to_ascii_lowercase().as_str() {
        "working" => Style::default().fg(ratatui::style::Color::Green),
        "idle" => Style::default().fg(ratatui::style::Color::Yellow),
        "stalled" | "rate_limited" => Style::default()
            .fg(ratatui::style::Color::Yellow)
            .add_modifier(Modifier::BOLD),
        "errored" | "auth_failed" | "provider_down" => Style::default()
            .fg(ratatui::style::Color::Red)
            .add_modifier(Modifier::BOLD),
        "stopped" => Style::default().fg(ratatui::style::Color::DarkGray),
        _ => Style::default().fg(ratatui::style::Color::Gray),
    }
}

fn attach_entry(entry: &SwitchEntry) -> Result<()> {
    let session = &entry.session_name;
    if !TmuxSession::session_exists(session) {
        println!("Session is no longer running: {}", entry.ref_name());
        return Ok(());
    }

    let bar = format!(
        " tutti: {} ({}) ── Ctrl+b d to detach",
        entry.agent_name, entry.workspace_name
    );
    let _ = TmuxSession::set_status_bar(session, &bar);
    TmuxSession::attach_session(session)
}

struct SwitchTerminal {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    active: bool,
}

impl SwitchTerminal {
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

impl Drop for SwitchTerminal {
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

    fn entry(ws: &str, agent: &str, runtime: &str, status: &str) -> SwitchEntry {
        SwitchEntry {
            workspace_name: ws.to_string(),
            agent_name: agent.to_string(),
            runtime: runtime.to_string(),
            status_raw: status.to_string(),
            session_name: format!("tutti-{ws}-{agent}"),
        }
    }

    #[test]
    fn filter_entries_empty_query_returns_all_in_order() {
        let entries = vec![
            entry("repo", "backend", "claude-code", "Working"),
            entry("repo", "frontend", "codex", "Idle"),
        ];
        let matcher = SkimMatcherV2::default();
        let filtered = filter_entries("", &entries, &matcher);
        assert_eq!(filtered, vec![0, 1]);
    }

    #[test]
    fn filter_entries_matches_workspace_or_agent() {
        let entries = vec![
            entry("api", "backend", "claude-code", "Working"),
            entry("web", "frontend", "codex", "Idle"),
        ];
        let matcher = SkimMatcherV2::default();
        let filtered = filter_entries("web/front", &entries, &matcher);
        assert_eq!(filtered, vec![1]);
    }
}
