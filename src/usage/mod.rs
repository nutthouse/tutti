use crate::config::ProfileConfig;
use crate::error::Result;
use chrono::{DateTime, Datelike, NaiveTime, TimeZone, Utc, Weekday};
use serde::Deserialize;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Component, Path, PathBuf};

// ── JSONL deserialization types ──

#[derive(Debug, Clone, Default, Deserialize)]
pub struct TokenUsage {
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_input_tokens: u64,
    #[serde(default)]
    pub cache_read_input_tokens: u64,
}

impl TokenUsage {
    pub fn merge(&mut self, other: &TokenUsage) {
        self.input_tokens += other.input_tokens;
        self.output_tokens += other.output_tokens;
        self.cache_creation_input_tokens += other.cache_creation_input_tokens;
        self.cache_read_input_tokens += other.cache_read_input_tokens;
    }

    pub fn total_input(&self) -> u64 {
        self.input_tokens + self.cache_creation_input_tokens + self.cache_read_input_tokens
    }
}

#[derive(Debug, Deserialize)]
struct JsonlEvent {
    #[serde(default, rename = "type")]
    event_type: String,
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default)]
    message: Option<JsonlMessage>,
}

#[derive(Debug, Deserialize)]
struct JsonlMessage {
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<TokenUsage>,
}

// ── Codex rollout deserialization types ──

#[derive(Debug, Deserialize)]
struct CodexRolloutLine {
    #[serde(default)]
    timestamp: Option<String>,
    #[serde(default, rename = "type")]
    line_type: String,
    #[serde(default)]
    payload: Option<CodexPayload>,
}

#[derive(Debug, Deserialize, Default)]
struct CodexPayload {
    #[serde(default)]
    cwd: Option<PathBuf>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default, rename = "type")]
    event_type: Option<String>,
    #[serde(default)]
    info: Option<CodexTokenInfo>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct CodexTokenInfo {
    #[serde(default)]
    total_token_usage: Option<CodexTokenUsage>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct CodexTokenUsage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    cached_input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
    #[serde(default)]
    reasoning_output_tokens: i64,
}

// ── Aggregation types ──

#[derive(Debug, Clone, Default)]
pub struct AggregatedUsage {
    pub total: TokenUsage,
    pub by_model: HashMap<String, TokenUsage>,
    pub session_count: usize,
}

impl AggregatedUsage {
    pub fn merge(&mut self, other: &AggregatedUsage) {
        self.total.merge(&other.total);
        self.session_count += other.session_count;
        for (model, usage) in &other.by_model {
            self.by_model.entry(model.clone()).or_default().merge(usage);
        }
    }

    fn add_event(&mut self, model: &str, usage: &TokenUsage) {
        self.total.merge(usage);
        self.by_model
            .entry(model.to_string())
            .or_default()
            .merge(usage);
    }
}

#[derive(Debug)]
pub struct WorkspaceUsage {
    pub workspace_name: String,
    pub usage: AggregatedUsage,
    pub by_agent: HashMap<String, AggregatedUsage>,
}

#[derive(Debug)]
pub struct ProfileUsageSummary {
    pub profile_name: String,
    pub plan: Option<String>,
    pub reset_start: DateTime<Utc>,
    pub reset_end: DateTime<Utc>,
    pub today: AggregatedUsage,
    pub weekly: AggregatedUsage,
    pub by_workspace: Vec<WorkspaceUsage>,
    pub capacity_pct: Option<f64>,
}

// ── Path encoding ──

/// Encode a filesystem path the way Claude Code does: replace `/` with `-`.
/// `/Users/foo/bar` → `-Users-foo-bar`
pub fn encode_project_path(path: &Path) -> String {
    let s = path.to_string_lossy();
    s.replace('/', "-")
}

/// Get the `~/.claude/projects` directory.
fn claude_projects_dir() -> Option<PathBuf> {
    let home = std::env::var("HOME").ok()?;
    let dir = PathBuf::from(home).join(".claude").join("projects");
    if dir.is_dir() { Some(dir) } else { None }
}

/// Find the Claude Code project data directory for a workspace path.
pub fn claude_data_dir(workspace_path: &Path) -> Option<PathBuf> {
    let encoded = encode_project_path(workspace_path);
    let dir = claude_projects_dir()?.join(&encoded);
    if dir.is_dir() { Some(dir) } else { None }
}

/// Get available Codex rollout root directories.
fn codex_rollout_roots() -> Vec<PathBuf> {
    let home = match std::env::var("HOME") {
        Ok(h) => h,
        Err(_) => return Vec::new(),
    };
    let codex_home = PathBuf::from(home).join(".codex");
    let mut roots = Vec::new();
    for rel in ["sessions", "archived_sessions"] {
        let dir = codex_home.join(rel);
        if dir.is_dir() {
            roots.push(dir);
        }
    }
    roots
}

fn is_codex_rollout_file(path: &Path) -> bool {
    path.extension().is_some_and(|ext| ext == "jsonl")
        && path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n.starts_with("rollout-"))
}

// ── JSONL parsing ──

fn parse_timestamp_utc(ts_str: &str) -> Option<DateTime<Utc>> {
    if let Ok(dt) = ts_str.parse::<DateTime<Utc>>() {
        return Some(dt);
    }
    if let Ok(ndt) = chrono::NaiveDateTime::parse_from_str(ts_str, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(Utc.from_utc_datetime(&ndt));
    }
    None
}

/// Parse a JSONL file and aggregate events into the provided buckets.
/// Each bucket has a `since` cutoff — events at or after the cutoff are merged in.
/// Buckets should be ordered from oldest to newest cutoff for efficiency.
pub fn parse_jsonl_into(
    path: &Path,
    buckets: &mut [(DateTime<Utc>, &mut AggregatedUsage)],
) -> Result<bool> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut had_events = false;

    // Find the earliest cutoff to skip lines before it
    let earliest = buckets
        .iter()
        .map(|(ts, _)| *ts)
        .min()
        .unwrap_or(Utc::now());

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        // Fast pre-filter: skip lines that aren't assistant events
        if !line.contains("\"assistant\"") {
            continue;
        }

        let event: JsonlEvent = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(_) => continue,
        };

        if event.event_type != "assistant" {
            continue;
        }

        let message = match event.message {
            Some(m) => m,
            None => continue,
        };

        let usage = match message.usage {
            Some(u) => u,
            None => continue,
        };

        let ts = match event.timestamp.as_deref().and_then(parse_timestamp_utc) {
            Some(ts) => ts,
            None => continue,
        };

        if ts < earliest {
            continue;
        }

        let model = message.model.as_deref().unwrap_or("unknown");

        for (since, agg) in buckets.iter_mut() {
            if ts >= *since {
                agg.add_event(model, &usage);
            }
        }
        had_events = true;
    }

    Ok(had_events)
}

/// Parse a single JSONL file, returning (timestamp, model, usage) for each assistant event after `since`.
#[cfg(test)]
pub fn parse_jsonl_file(
    path: &Path,
    since: DateTime<Utc>,
) -> Result<Vec<(DateTime<Utc>, String, TokenUsage)>> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut results = Vec::new();

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        if !line.contains("\"assistant\"") {
            continue;
        }

        let event: JsonlEvent = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(_) => continue,
        };

        if event.event_type != "assistant" {
            continue;
        }

        let message = match event.message {
            Some(m) => m,
            None => continue,
        };

        let usage = match message.usage {
            Some(u) => u,
            None => continue,
        };

        let ts = match event.timestamp.as_deref().and_then(parse_timestamp_utc) {
            Some(ts) => ts,
            None => continue,
        };

        if ts < since {
            continue;
        }

        let model = message.model.unwrap_or_else(|| "unknown".to_string());
        results.push((ts, model, usage));
    }

    Ok(results)
}

/// Scan all JSONL files in a Claude project directory, aggregating into multiple time buckets.
fn scan_project_dir_multi(
    dir: &Path,
    buckets: &mut [(DateTime<Utc>, &mut AggregatedUsage)],
) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "jsonl") {
            match parse_jsonl_into(&path, buckets) {
                Ok(had_events) => {
                    if had_events {
                        // Increment session_count on all buckets (file had relevant events)
                        for (_, agg) in buckets.iter_mut() {
                            agg.session_count += 1;
                        }
                    }
                }
                Err(_) => continue,
            }
        }
    }

    Ok(())
}

/// Scan all JSONL files in a Claude project directory.
pub fn scan_project_dir(dir: &Path, since: DateTime<Utc>) -> Result<AggregatedUsage> {
    let mut agg = AggregatedUsage::default();
    scan_project_dir_multi(dir, &mut [(since, &mut agg)])?;
    Ok(agg)
}

/// Scan usage for a workspace, including worktree agent subdirectories.
pub fn scan_workspace_usage(
    workspace_path: &Path,
    workspace_name: &str,
    since: DateTime<Utc>,
) -> Result<WorkspaceUsage> {
    let mut total_usage = AggregatedUsage::default();
    let mut by_agent: HashMap<String, AggregatedUsage> = HashMap::new();

    if let Some(data_dir) = claude_data_dir(workspace_path) {
        let main_usage = scan_project_dir(&data_dir, since)?;
        total_usage.merge(&main_usage);
    }

    let projects_dir = claude_projects_dir();
    let encoded_base = encode_project_path(workspace_path);
    let worktree_prefix = format!("{encoded_base}--tutti-worktrees-");

    if let Some(ref projects_dir) = projects_dir
        && let Ok(entries) = fs::read_dir(projects_dir)
    {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(agent_name) = name.strip_prefix(&worktree_prefix)
                && entry.path().is_dir()
            {
                let agent_usage = scan_project_dir(&entry.path(), since)?;
                total_usage.merge(&agent_usage);
                by_agent
                    .entry(agent_name.to_string())
                    .or_default()
                    .merge(&agent_usage);
            }
        }
    }

    let (codex_usage, codex_by_agent) = scan_codex_workspace_usage(workspace_path, since)?;
    total_usage.merge(&codex_usage);
    for (agent, usage) in codex_by_agent {
        by_agent.entry(agent).or_default().merge(&usage);
    }

    Ok(WorkspaceUsage {
        workspace_name: workspace_name.to_string(),
        usage: total_usage,
        by_agent,
    })
}

/// Scan a workspace with two time windows in a single pass (weekly + today).
/// Returns (weekly_usage, today_usage, by_agent based on weekly window).
fn scan_workspace_dual(
    workspace_path: &Path,
    workspace_name: &str,
    weekly_since: DateTime<Utc>,
    today_since: DateTime<Utc>,
) -> Result<(WorkspaceUsage, AggregatedUsage)> {
    let mut weekly_total = AggregatedUsage::default();
    let mut today_total = AggregatedUsage::default();
    let mut by_agent: HashMap<String, AggregatedUsage> = HashMap::new();

    if let Some(data_dir) = claude_data_dir(workspace_path) {
        scan_project_dir_multi(
            &data_dir,
            &mut [
                (weekly_since, &mut weekly_total),
                (today_since, &mut today_total),
            ],
        )?;
    }

    let projects_dir = claude_projects_dir();
    let encoded_base = encode_project_path(workspace_path);
    let worktree_prefix = format!("{encoded_base}--tutti-worktrees-");

    if let Some(ref projects_dir) = projects_dir
        && let Ok(entries) = fs::read_dir(projects_dir)
    {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(agent_name) = name.strip_prefix(&worktree_prefix)
                && entry.path().is_dir()
            {
                let mut agent_weekly = AggregatedUsage::default();
                let mut agent_today = AggregatedUsage::default();
                scan_project_dir_multi(
                    &entry.path(),
                    &mut [
                        (weekly_since, &mut agent_weekly),
                        (today_since, &mut agent_today),
                    ],
                )?;
                weekly_total.merge(&agent_weekly);
                today_total.merge(&agent_today);
                by_agent
                    .entry(agent_name.to_string())
                    .or_default()
                    .merge(&agent_weekly);
            }
        }
    }

    let (codex_weekly, codex_by_agent_weekly) =
        scan_codex_workspace_usage(workspace_path, weekly_since)?;
    let (codex_today, _) = scan_codex_workspace_usage(workspace_path, today_since)?;
    weekly_total.merge(&codex_weekly);
    today_total.merge(&codex_today);
    for (agent, usage) in codex_by_agent_weekly {
        by_agent.entry(agent).or_default().merge(&usage);
    }

    let ws_usage = WorkspaceUsage {
        workspace_name: workspace_name.to_string(),
        usage: weekly_total,
        by_agent,
    };

    Ok((ws_usage, today_total))
}

fn scan_codex_workspace_usage(
    workspace_path: &Path,
    since: DateTime<Utc>,
) -> Result<(AggregatedUsage, HashMap<String, AggregatedUsage>)> {
    let mut total_usage = AggregatedUsage::default();
    let mut by_agent: HashMap<String, AggregatedUsage> = HashMap::new();

    for root in codex_rollout_roots() {
        scan_codex_rollout_tree(
            &root,
            workspace_path,
            since,
            &mut total_usage,
            &mut by_agent,
        )?;
    }

    Ok((total_usage, by_agent))
}

fn scan_codex_rollout_tree(
    dir: &Path,
    workspace_path: &Path,
    since: DateTime<Utc>,
    total_usage: &mut AggregatedUsage,
    by_agent: &mut HashMap<String, AggregatedUsage>,
) -> Result<()> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return Ok(()),
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            scan_codex_rollout_tree(&path, workspace_path, since, total_usage, by_agent)?;
            continue;
        }
        if !is_codex_rollout_file(&path) {
            continue;
        }
        if let Some((agent_name, usage)) = parse_codex_rollout_file(&path, workspace_path, since)? {
            total_usage.merge(&usage);
            if let Some(agent_name) = agent_name {
                by_agent.entry(agent_name).or_default().merge(&usage);
            }
        }
    }

    Ok(())
}

fn parse_codex_rollout_file(
    path: &Path,
    workspace_path: &Path,
    since: DateTime<Utc>,
) -> Result<Option<(Option<String>, AggregatedUsage)>> {
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut agg = AggregatedUsage::default();
    let mut model = "codex".to_string();
    let mut prev_total = CodexTokenUsage::default();
    let mut has_prev_total = false;
    let mut session_scope: Option<Option<String>> = None;
    let mut had_matching_token_event = false;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        let event: CodexRolloutLine = match serde_json::from_str(&line) {
            Ok(e) => e,
            Err(_) => continue,
        };

        match event.line_type.as_str() {
            "session_meta" => {
                if let Some(payload) = event.payload
                    && let Some(cwd) = payload.cwd
                {
                    session_scope = classify_codex_cwd(&cwd, workspace_path);
                }
            }
            "turn_context" => {
                if let Some(payload) = event.payload
                    && let Some(m) = payload.model
                {
                    model = m;
                }
            }
            "event_msg" => {
                let payload = match event.payload {
                    Some(p) => p,
                    None => continue,
                };
                if payload.event_type.as_deref() != Some("token_count") {
                    continue;
                }
                let total = match payload.info.and_then(|info| info.total_token_usage) {
                    Some(t) => t,
                    None => continue,
                };
                let ts = match event.timestamp.as_deref().and_then(parse_timestamp_utc) {
                    Some(ts) => ts,
                    None => continue,
                };

                let delta = if has_prev_total {
                    codex_usage_delta(&prev_total, &total)
                } else {
                    codex_usage_delta(&CodexTokenUsage::default(), &total)
                };
                prev_total = total;
                has_prev_total = true;

                // We only count events that map to this workspace.
                if session_scope.is_none() {
                    continue;
                }
                if ts < since {
                    continue;
                }

                had_matching_token_event = true;
                if delta.input_tokens > 0
                    || delta.cache_read_input_tokens > 0
                    || delta.output_tokens > 0
                {
                    agg.add_event(&model, &delta);
                }
            }
            _ => {}
        }
    }

    if !had_matching_token_event {
        return Ok(None);
    }

    agg.session_count = 1;
    Ok(Some((session_scope.unwrap_or(None), agg)))
}

fn classify_codex_cwd(cwd: &Path, workspace_path: &Path) -> Option<Option<String>> {
    if !cwd.starts_with(workspace_path) {
        return None;
    }

    let worktrees_root = workspace_path.join(".tutti").join("worktrees");
    if cwd.starts_with(&worktrees_root)
        && let Ok(relative) = cwd.strip_prefix(&worktrees_root)
        && let Some(Component::Normal(agent)) = relative.components().next()
    {
        return Some(Some(agent.to_string_lossy().to_string()));
    }

    Some(None)
}

fn codex_usage_delta(previous: &CodexTokenUsage, current: &CodexTokenUsage) -> TokenUsage {
    let output_delta =
        non_negative_to_u64(current.output_tokens.saturating_sub(previous.output_tokens))
            .saturating_add(non_negative_to_u64(
                current
                    .reasoning_output_tokens
                    .saturating_sub(previous.reasoning_output_tokens),
            ));

    TokenUsage {
        input_tokens: non_negative_to_u64(
            current.input_tokens.saturating_sub(previous.input_tokens),
        ),
        output_tokens: output_delta,
        cache_creation_input_tokens: 0,
        cache_read_input_tokens: non_negative_to_u64(
            current
                .cached_input_tokens
                .saturating_sub(previous.cached_input_tokens),
        ),
    }
}

fn non_negative_to_u64(value: i64) -> u64 {
    if value <= 0 { 0 } else { value as u64 }
}

// ── Time calculations ──

/// Compute the start of the current reset period based on the reset day.
pub fn compute_reset_start(reset_day: Option<&str>) -> DateTime<Utc> {
    let weekday = match reset_day {
        Some(d) => match d.to_lowercase().as_str() {
            "monday" | "mon" => Weekday::Mon,
            "tuesday" | "tue" => Weekday::Tue,
            "wednesday" | "wed" => Weekday::Wed,
            "thursday" | "thu" => Weekday::Thu,
            "friday" | "fri" => Weekday::Fri,
            "saturday" | "sat" => Weekday::Sat,
            "sunday" | "sun" => Weekday::Sun,
            _ => Weekday::Mon,
        },
        None => Weekday::Mon,
    };

    let now = Utc::now();
    let today = now.weekday();
    let days_since =
        (today.num_days_from_monday() as i64 - weekday.num_days_from_monday() as i64 + 7) % 7;

    let reset_date = now.date_naive() - chrono::Duration::days(days_since);
    Utc.from_utc_datetime(&reset_date.and_time(NaiveTime::MIN))
}

/// Get the start of today (UTC).
pub fn today_start() -> DateTime<Utc> {
    let now = Utc::now();
    Utc.from_utc_datetime(&now.date_naive().and_time(NaiveTime::MIN))
}

// ── Capacity estimation ──

/// Approximate compute-hours from token usage.
/// These are rough estimates based on publicly observable patterns.
/// Actual billing may differ.
pub fn estimate_compute_hours(usage: &TokenUsage, _model: &str) -> f64 {
    // Rough approximation: ~1M output tokens ≈ 1 compute-hour for Sonnet-class,
    // ~500K output tokens ≈ 1 compute-hour for Opus-class.
    // Input tokens count much less. Cache reads are heavily discounted.
    //
    // This is intentionally a rough estimate — the actual compute-hour formula
    // is not publicly documented.
    let output_hours = usage.output_tokens as f64 / 1_000_000.0;
    let input_hours = usage.input_tokens as f64 / 5_000_000.0;
    let cache_create_hours = usage.cache_creation_input_tokens as f64 / 4_000_000.0;
    let cache_read_hours = usage.cache_read_input_tokens as f64 / 20_000_000.0;

    output_hours + input_hours + cache_create_hours + cache_read_hours
}

/// Estimate total compute-hours for an aggregated usage, using per-model rates.
pub fn estimate_total_hours(agg: &AggregatedUsage) -> f64 {
    let mut hours = 0.0;
    for (model, usage) in &agg.by_model {
        hours += estimate_compute_hours(usage, model);
    }
    // If no per-model breakdown, estimate from total
    if agg.by_model.is_empty() && (agg.total.input_tokens > 0 || agg.total.output_tokens > 0) {
        hours = estimate_compute_hours(&agg.total, "unknown");
    }
    hours
}

// ── Profile summarization ──

/// Build a full usage summary for a profile across all its workspaces.
pub fn summarize_profile(
    profile: &ProfileConfig,
    workspaces: &[(String, PathBuf)],
) -> Result<ProfileUsageSummary> {
    let reset_start = compute_reset_start(profile.reset_day.as_deref());
    let reset_end = reset_start + chrono::Duration::days(7);
    let today = today_start();

    let mut weekly = AggregatedUsage::default();
    let mut today_usage = AggregatedUsage::default();
    let mut by_workspace = Vec::new();

    for (name, path) in workspaces {
        let (ws_usage, ws_today) = scan_workspace_dual(path, name, reset_start, today)?;
        weekly.merge(&ws_usage.usage);
        today_usage.merge(&ws_today);
        by_workspace.push(ws_usage);
    }

    let weekly_hours_used = estimate_total_hours(&weekly);
    let capacity_pct = profile
        .weekly_hours
        .map(|ceiling| (weekly_hours_used / ceiling) * 100.0);

    Ok(ProfileUsageSummary {
        profile_name: profile.name.clone(),
        plan: profile.plan.clone(),
        reset_start,
        reset_end,
        today: today_usage,
        weekly,
        by_workspace,
        capacity_pct,
    })
}

/// Lightweight capacity check for a single workspace (used by `tt up`).
pub fn quick_capacity_check(profile: &ProfileConfig, workspace_path: &Path) -> Result<Option<f64>> {
    let ceiling = match profile.weekly_hours {
        Some(h) => h,
        None => return Ok(None),
    };

    let reset_start = compute_reset_start(profile.reset_day.as_deref());
    let ws_usage = scan_workspace_usage(workspace_path, "", reset_start)?;
    let hours_used = estimate_total_hours(&ws_usage.usage);

    Ok(Some((hours_used / ceiling) * 100.0))
}

// ── Formatting helpers ──

/// Format a token count with thousands separators.
pub fn format_tokens(n: u64) -> String {
    if n == 0 {
        return "0".to_string();
    }
    let s = n.to_string();
    let mut result = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            result.push(',');
        }
        result.push(c);
    }
    result.chars().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_project_path_basic() {
        let path = Path::new("/Users/foo/bar");
        assert_eq!(encode_project_path(path), "-Users-foo-bar");
    }

    #[test]
    fn encode_project_path_home() {
        let path = Path::new("/home/user/projects/my-app");
        assert_eq!(encode_project_path(path), "-home-user-projects-my-app");
    }

    #[test]
    fn encode_project_path_nested() {
        let path = Path::new("/Users/adam/Documents/GitHub/tutti");
        assert_eq!(
            encode_project_path(path),
            "-Users-adam-Documents-GitHub-tutti"
        );
    }

    #[test]
    fn parse_jsonl_file_extracts_usage() {
        let dir = std::env::temp_dir().join("tutti_test_parse");
        let _ = fs::create_dir_all(&dir);
        let file_path = dir.join("test_session.jsonl");

        let jsonl = r#"{"type":"human","timestamp":"2026-03-13T10:00:00Z","message":{"text":"hello"}}
{"type":"assistant","timestamp":"2026-03-13T10:00:01Z","message":{"model":"claude-sonnet-4-5-20250514","usage":{"input_tokens":1000,"output_tokens":200,"cache_creation_input_tokens":500,"cache_read_input_tokens":300}}}
{"type":"assistant","timestamp":"2026-03-13T10:00:05Z","message":{"model":"claude-sonnet-4-5-20250514","usage":{"input_tokens":2000,"output_tokens":400,"cache_creation_input_tokens":0,"cache_read_input_tokens":1500}}}
"#;
        fs::write(&file_path, jsonl).unwrap();

        let since = "2026-03-13T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let events = parse_jsonl_file(&file_path, since).unwrap();

        assert_eq!(events.len(), 2);
        assert_eq!(events[0].2.input_tokens, 1000);
        assert_eq!(events[0].2.output_tokens, 200);
        assert_eq!(events[1].2.input_tokens, 2000);
        assert_eq!(events[1].2.cache_read_input_tokens, 1500);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_jsonl_skips_before_since() {
        let dir = std::env::temp_dir().join("tutti_test_since");
        let _ = fs::create_dir_all(&dir);
        let file_path = dir.join("test_since.jsonl");

        let jsonl = r#"{"type":"assistant","timestamp":"2026-03-12T10:00:00Z","message":{"model":"claude-sonnet-4-5-20250514","usage":{"input_tokens":1000,"output_tokens":200,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}
{"type":"assistant","timestamp":"2026-03-13T10:00:00Z","message":{"model":"claude-sonnet-4-5-20250514","usage":{"input_tokens":2000,"output_tokens":400,"cache_creation_input_tokens":0,"cache_read_input_tokens":0}}}
"#;
        fs::write(&file_path, jsonl).unwrap();

        let since = "2026-03-13T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let events = parse_jsonl_file(&file_path, since).unwrap();

        assert_eq!(events.len(), 1);
        assert_eq!(events[0].2.input_tokens, 2000);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_codex_rollout_file_computes_delta_and_agent() {
        let dir = std::env::temp_dir().join("tutti_test_codex_delta");
        let workspace = dir.join("workspace");
        let worktree = workspace.join(".tutti").join("worktrees").join("frontend");
        let _ = fs::create_dir_all(&worktree);
        let file_path = dir.join("rollout-2026-03-13T10-00-00-test.jsonl");

        let jsonl = format!(
            r#"{{"timestamp":"2026-03-13T10:00:00Z","type":"session_meta","payload":{{"cwd":"{}"}}}}
{{"timestamp":"2026-03-13T10:00:00Z","type":"turn_context","payload":{{"model":"gpt-5.3-codex"}}}}
{{"timestamp":"2026-03-13T10:00:01Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":5,"total_tokens":135}}}}}}}}
{{"timestamp":"2026-03-13T10:00:02Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":130,"cached_input_tokens":30,"output_tokens":16,"reasoning_output_tokens":9,"total_tokens":185}}}}}}}}
{{"timestamp":"2026-03-13T10:00:03Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":130,"cached_input_tokens":30,"output_tokens":16,"reasoning_output_tokens":9,"total_tokens":185}}}}}}}}
"#,
            worktree.display()
        );
        fs::write(&file_path, jsonl).unwrap();

        let since = "2026-03-13T00:00:00Z".parse::<DateTime<Utc>>().unwrap();
        let (agent, usage) = parse_codex_rollout_file(&file_path, &workspace, since)
            .unwrap()
            .unwrap();

        assert_eq!(agent.as_deref(), Some("frontend"));
        assert_eq!(usage.session_count, 1);
        assert_eq!(usage.total.input_tokens, 130);
        assert_eq!(usage.total.cache_read_input_tokens, 30);
        assert_eq!(usage.total.output_tokens, 25);
        assert_eq!(usage.by_model["gpt-5.3-codex"].cache_read_input_tokens, 30);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_codex_rollout_file_respects_since_cutoff() {
        let dir = std::env::temp_dir().join("tutti_test_codex_since");
        let workspace = dir.join("workspace");
        let _ = fs::create_dir_all(&workspace);
        let file_path = dir.join("rollout-2026-03-13T10-00-00-since.jsonl");

        let jsonl = format!(
            r#"{{"timestamp":"2026-03-13T10:00:00Z","type":"session_meta","payload":{{"cwd":"{}"}}}}
{{"timestamp":"2026-03-13T10:00:00Z","type":"turn_context","payload":{{"model":"gpt-5.3-codex"}}}}
{{"timestamp":"2026-03-13T10:00:01Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":100,"cached_input_tokens":10,"output_tokens":20,"reasoning_output_tokens":4,"total_tokens":134}}}}}}}}
{{"timestamp":"2026-03-13T10:00:10Z","type":"event_msg","payload":{{"type":"token_count","info":{{"total_token_usage":{{"input_tokens":160,"cached_input_tokens":20,"output_tokens":25,"reasoning_output_tokens":6,"total_tokens":211}}}}}}}}
"#,
            workspace.display()
        );
        fs::write(&file_path, jsonl).unwrap();

        let since = "2026-03-13T10:00:05Z".parse::<DateTime<Utc>>().unwrap();
        let (_agent, usage) = parse_codex_rollout_file(&file_path, &workspace, since)
            .unwrap()
            .unwrap();

        assert_eq!(usage.total.input_tokens, 60);
        assert_eq!(usage.total.cache_read_input_tokens, 10);
        assert_eq!(usage.total.output_tokens, 7);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn compute_reset_start_monday() {
        let reset = compute_reset_start(Some("monday"));
        assert_eq!(reset.weekday(), Weekday::Mon);
        assert!(reset <= Utc::now());
        // Should be within the last 7 days
        assert!(Utc::now() - reset < chrono::Duration::days(7));
    }

    #[test]
    fn compute_reset_start_each_weekday() {
        for day in &[
            "monday",
            "tuesday",
            "wednesday",
            "thursday",
            "friday",
            "saturday",
            "sunday",
        ] {
            let reset = compute_reset_start(Some(day));
            let expected_wd = match *day {
                "monday" => Weekday::Mon,
                "tuesday" => Weekday::Tue,
                "wednesday" => Weekday::Wed,
                "thursday" => Weekday::Thu,
                "friday" => Weekday::Fri,
                "saturday" => Weekday::Sat,
                "sunday" => Weekday::Sun,
                _ => unreachable!(),
            };
            assert_eq!(reset.weekday(), expected_wd, "Failed for {day}");
            assert!(Utc::now() - reset < chrono::Duration::days(7));
        }
    }

    #[test]
    fn estimate_compute_hours_known_tokens() {
        let usage = TokenUsage {
            input_tokens: 1_000_000,
            output_tokens: 1_000_000,
            cache_creation_input_tokens: 0,
            cache_read_input_tokens: 0,
        };
        let hours = estimate_compute_hours(&usage, "claude-sonnet-4-5-20250514");
        // 1M output / 1M = 1.0, plus 1M input / 5M = 0.2
        assert!((hours - 1.2).abs() < 0.01);
    }

    #[test]
    fn profile_config_with_new_fields() {
        let toml_str = r#"
[user]
name = "Test"

[[profile]]
name = "claude-max"
provider = "anthropic"
command = "claude"
plan = "max"
reset_day = "monday"
weekly_hours = 45.0
"#;
        let config: crate::config::GlobalConfig = toml::from_str(toml_str).unwrap();
        let profile = &config.profiles[0];
        assert_eq!(profile.plan.as_deref(), Some("max"));
        assert_eq!(profile.reset_day.as_deref(), Some("monday"));
        assert_eq!(profile.weekly_hours, Some(45.0));
    }

    #[test]
    fn profile_config_backward_compat() {
        let toml_str = r#"
[[profile]]
name = "claude-personal"
provider = "anthropic"
command = "claude"
max_concurrent = 5
"#;
        let config: crate::config::GlobalConfig = toml::from_str(toml_str).unwrap();
        let profile = &config.profiles[0];
        assert_eq!(profile.name, "claude-personal");
        assert!(profile.plan.is_none());
        assert!(profile.reset_day.is_none());
        assert!(profile.weekly_hours.is_none());
    }

    #[test]
    fn aggregated_usage_merge() {
        let mut a = AggregatedUsage::default();
        a.total = TokenUsage {
            input_tokens: 100,
            output_tokens: 50,
            cache_creation_input_tokens: 10,
            cache_read_input_tokens: 5,
        };
        a.by_model.insert(
            "sonnet".to_string(),
            TokenUsage {
                input_tokens: 100,
                output_tokens: 50,
                cache_creation_input_tokens: 10,
                cache_read_input_tokens: 5,
            },
        );
        a.session_count = 1;

        let mut b = AggregatedUsage::default();
        b.total = TokenUsage {
            input_tokens: 200,
            output_tokens: 100,
            cache_creation_input_tokens: 20,
            cache_read_input_tokens: 10,
        };
        b.by_model.insert(
            "sonnet".to_string(),
            TokenUsage {
                input_tokens: 150,
                output_tokens: 75,
                cache_creation_input_tokens: 15,
                cache_read_input_tokens: 8,
            },
        );
        b.by_model.insert(
            "opus".to_string(),
            TokenUsage {
                input_tokens: 50,
                output_tokens: 25,
                cache_creation_input_tokens: 5,
                cache_read_input_tokens: 2,
            },
        );
        b.session_count = 2;

        a.merge(&b);

        assert_eq!(a.total.input_tokens, 300);
        assert_eq!(a.total.output_tokens, 150);
        assert_eq!(a.session_count, 3);
        assert_eq!(a.by_model["sonnet"].input_tokens, 250);
        assert_eq!(a.by_model["opus"].input_tokens, 50);
    }

    #[test]
    fn format_tokens_thousands_separator() {
        assert_eq!(format_tokens(0), "0");
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1000), "1,000");
        assert_eq!(format_tokens(1_234_567), "1,234,567");
    }
}
