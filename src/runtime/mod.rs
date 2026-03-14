pub mod aider;
pub mod claude_code;
pub mod codex;
pub mod openclaw;
use crate::error::{Result, TuttiError};
use serde::{Deserialize, Serialize};

/// Status of an agent as detected from terminal output.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentStatus {
    /// Agent is actively working (generating output).
    Working,
    /// Agent is idle / waiting for input.
    Idle,
    /// Agent's auth token has expired or is invalid.
    AuthFailed(String),
    /// Session exists but status is unknown.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionSignal {
    Explicit(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectionDiagnostics {
    pub status: AgentStatus,
    pub confidence: f32,
    pub matched_patterns: Vec<String>,
    pub auth_match: Option<String>,
    pub rate_limit_match: Option<String>,
    pub provider_down_match: Option<String>,
    pub completion_match: Option<String>,
}

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentStatus::Working => write!(f, "Working"),
            AgentStatus::Idle => write!(f, "Idle"),
            AgentStatus::AuthFailed(msg) => write!(f, "Auth Failed: {msg}"),
            AgentStatus::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Trait for runtime-specific behavior.
pub trait RuntimeAdapter {
    /// The CLI command name (e.g., "claude").
    fn command_name(&self) -> &str;

    /// Check if the runtime CLI is available on PATH.
    fn is_available(&self) -> bool;

    /// Build the shell command string to spawn this runtime with additional pre-args.
    fn build_spawn_command_with_args(&self, pre_args: &[String], prompt: Option<&str>) -> String;

    /// Build the shell command string to spawn this runtime.
    fn build_spawn_command(&self, prompt: Option<&str>) -> String {
        self.build_spawn_command_with_args(&[], prompt)
    }

    /// Detect agent status from captured terminal output.
    fn detect_status(&self, terminal_output: &str) -> AgentStatus;

    /// Check if terminal output indicates an auth failure.
    fn detect_auth_failure(&self, terminal_output: &str) -> Option<String>;

    /// Check if terminal output indicates a provider rate-limit condition.
    fn detect_rate_limit(&self, terminal_output: &str) -> Option<String>;

    /// Check if terminal output indicates provider outage/unavailability.
    fn detect_provider_down(&self, terminal_output: &str) -> Option<String>;

    /// Detect explicit runtime completion markers (preferred over heuristic idle detection).
    fn detect_completion_signal(&self, terminal_output: &str) -> Option<CompletionSignal>;

    /// Whether this runtime exposes explicit completion signals.
    fn supports_completion_signal(&self) -> bool;

    /// Explain runtime detection decisions for a captured terminal output.
    fn diagnose(&self, terminal_output: &str) -> DetectionDiagnostics;
}

/// Shared configuration that drives the default RuntimeAdapter implementation.
struct RuntimeConfig {
    default_command: &'static str,
    /// Flag before the prompt (e.g. "--message"). Empty string means positional arg.
    prompt_flag: &'static str,
    /// Heuristic terminal-output patterns.
    ///
    /// These are intentionally simple string matches against recent pane output
    /// (spinner glyphs, prompt text, auth phrases). Upstream CLI output can change,
    /// so update these lists when status detection drifts after provider upgrades.
    auth_patterns: &'static [&'static str],
    rate_limit_patterns: &'static [&'static str],
    provider_down_patterns: &'static [&'static str],
    working_patterns: &'static [&'static str],
    idle_patterns: &'static [&'static str],
    completion_patterns: &'static [&'static str],
}

/// Common adapter that holds an optional command override and runtime-specific config.
struct CommonAdapter {
    command: Option<String>,
    config: &'static RuntimeConfig,
}

impl RuntimeAdapter for CommonAdapter {
    fn command_name(&self) -> &str {
        self.command
            .as_deref()
            .unwrap_or(self.config.default_command)
    }

    fn is_available(&self) -> bool {
        which::which(self.command_name()).is_ok()
    }

    fn build_spawn_command_with_args(&self, pre_args: &[String], prompt: Option<&str>) -> String {
        let cmd = self.command_name();
        let mut command = cmd.to_string();

        for arg in pre_args {
            command.push(' ');
            command.push_str(&shell_escape(arg));
        }

        match prompt {
            Some(p) if self.config.prompt_flag.is_empty() => {
                command.push(' ');
                command.push_str(&shell_escape(p));
            }
            Some(p) => {
                command.push(' ');
                command.push_str(self.config.prompt_flag);
                command.push(' ');
                command.push_str(&shell_escape(p));
            }
            None => {}
        }

        command
    }

    fn detect_status(&self, terminal_output: &str) -> AgentStatus {
        self.diagnose(terminal_output).status
    }

    fn detect_auth_failure(&self, terminal_output: &str) -> Option<String> {
        self.diagnose(terminal_output).auth_match
    }

    fn detect_rate_limit(&self, terminal_output: &str) -> Option<String> {
        self.diagnose(terminal_output).rate_limit_match
    }

    fn detect_provider_down(&self, terminal_output: &str) -> Option<String> {
        self.diagnose(terminal_output).provider_down_match
    }

    fn detect_completion_signal(&self, terminal_output: &str) -> Option<CompletionSignal> {
        self.diagnose(terminal_output)
            .completion_match
            .map(CompletionSignal::Explicit)
    }

    fn supports_completion_signal(&self) -> bool {
        !self.config.completion_patterns.is_empty()
    }

    fn diagnose(&self, terminal_output: &str) -> DetectionDiagnostics {
        diagnose_with_config(self.config, terminal_output)
    }
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn recent_window(terminal_output: &str, max_lines: usize) -> String {
    terminal_output
        .lines()
        .rev()
        .take(max_lines)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

fn trailing_non_empty_window(text: &str, max_lines: usize) -> String {
    text.lines()
        .rev()
        .filter(|line| !line.trim().is_empty())
        .take(max_lines)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

fn detect_pattern_ci(haystack_lower: &str, patterns: &[&str]) -> Option<String> {
    patterns
        .iter()
        .find(|pattern| haystack_lower.contains(&pattern.to_lowercase()))
        .map(|pattern| (*pattern).to_string())
}

fn collect_pattern_matches(haystack_lower: &str, patterns: &[&str]) -> Vec<String> {
    patterns
        .iter()
        .filter_map(|pattern| {
            if haystack_lower.contains(&pattern.to_lowercase()) {
                Some((*pattern).to_string())
            } else {
                None
            }
        })
        .collect()
}

fn contains_spinner_glyph(text: &str) -> bool {
    const SPINNER_GLYPHS: [char; 10] = ['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    text.chars().any(|ch| SPINNER_GLYPHS.contains(&ch))
}

fn weighted_pattern_score(matches: usize) -> f32 {
    if matches == 0 {
        return 0.0;
    }
    let base = 0.40;
    let incremental = 0.12 * (matches.saturating_sub(1) as f32);
    (base + incremental).min(0.92)
}

fn diagnose_with_config(config: &RuntimeConfig, terminal_output: &str) -> DetectionDiagnostics {
    let recent = recent_window(terminal_output, 40);
    let recent_lower = recent.to_lowercase();
    let trailing = trailing_non_empty_window(&recent, 4);
    let trailing_lower = trailing.to_lowercase();
    let completion_haystack_lower = if trailing_lower.is_empty() {
        &recent_lower
    } else {
        &trailing_lower
    };

    let auth_match = detect_pattern_ci(&recent_lower, config.auth_patterns);
    let rate_limit_match = detect_pattern_ci(&recent_lower, config.rate_limit_patterns);
    let provider_down_match = detect_pattern_ci(&recent_lower, config.provider_down_patterns);
    let completion_match = detect_pattern_ci(completion_haystack_lower, config.completion_patterns);
    let working_matches = collect_pattern_matches(&recent_lower, config.working_patterns);
    let idle_matches = collect_pattern_matches(completion_haystack_lower, config.idle_patterns);

    if let Some(reason) = auth_match.clone() {
        return DetectionDiagnostics {
            status: AgentStatus::AuthFailed(reason.clone()),
            confidence: 1.0,
            matched_patterns: vec![format!("auth:{reason}")],
            auth_match,
            rate_limit_match,
            provider_down_match,
            completion_match,
        };
    }

    let mut working_score = weighted_pattern_score(working_matches.len());
    let mut idle_score = weighted_pattern_score(idle_matches.len());

    if contains_spinner_glyph(&recent) {
        working_score = working_score.max(0.70);
    }
    if completion_match.is_some() {
        // Structured completion markers outrank single-word pattern hits.
        idle_score = idle_score.max(0.85);
    }

    let mut matched_patterns = Vec::new();
    matched_patterns.extend(
        working_matches
            .iter()
            .map(|pattern| format!("working:{pattern}")),
    );
    matched_patterns.extend(idle_matches.iter().map(|pattern| format!("idle:{pattern}")));
    if let Some(pattern) = completion_match.as_deref() {
        matched_patterns.push(format!("completion:{pattern}"));
    }

    let (status, confidence) = if working_score == 0.0 && idle_score == 0.0 {
        (AgentStatus::Unknown, 0.0)
    } else if (working_score - idle_score).abs() < 0.10 {
        // Avoid brittle flips when both states weakly match.
        (AgentStatus::Unknown, 0.45)
    } else if working_score > idle_score {
        (AgentStatus::Working, working_score.min(0.99))
    } else {
        (AgentStatus::Idle, idle_score.min(0.99))
    };

    DetectionDiagnostics {
        status,
        confidence,
        matched_patterns,
        auth_match,
        rate_limit_match,
        provider_down_match,
        completion_match,
    }
}

/// Get a runtime adapter by name, with an optional command override from a profile.
pub fn get_adapter(
    runtime: &str,
    command_override: Option<&str>,
) -> Option<Box<dyn RuntimeAdapter>> {
    let config: &'static RuntimeConfig = match runtime {
        "claude-code" => &claude_code::CONFIG,
        "codex" => &codex::CONFIG,
        "aider" => &aider::CONFIG,
        "openclaw" => &openclaw::CONFIG,
        _ => return None,
    };
    Some(Box::new(CommonAdapter {
        command: command_override.map(|s| s.to_string()),
        config,
    }))
}

pub fn diagnose_output(
    runtime: &str,
    terminal_output: &str,
    command_override: Option<&str>,
) -> Result<DetectionDiagnostics> {
    let adapter = get_adapter(runtime, command_override)
        .ok_or_else(|| TuttiError::RuntimeUnknown(runtime.to_string()))?;
    Ok(adapter.diagnose(terminal_output))
}

/// Return a profile command override only when it is compatible with the agent runtime.
///
/// This prevents cross-runtime leakage where, for example, a Claude profile command
/// would accidentally override Codex agents in mixed-runtime workspaces.
pub fn compatible_command_override<'a>(
    runtime: &str,
    profile_provider: Option<&str>,
    profile_command: Option<&'a str>,
) -> Option<&'a str> {
    let command = profile_command?.trim();
    if command.is_empty() {
        return None;
    }

    let provider = profile_provider
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let command_lc = command.to_ascii_lowercase();

    let compatible = match runtime {
        "claude-code" => command_lc.contains("claude") || provider == "anthropic",
        "codex" => command_lc.contains("codex") || provider == "openai",
        "aider" => command_lc.contains("aider"),
        "openclaw" => command_lc.contains("openclaw") || provider == "openclaw",
        _ => false,
    };

    if compatible { Some(command) } else { None }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(runtime: &str) -> Box<dyn RuntimeAdapter> {
        get_adapter(runtime, None).unwrap()
    }

    // -- Claude Code tests --

    #[test]
    fn claude_detect_working_from_spinner() {
        let a = adapter("claude-code");
        assert_eq!(
            a.detect_status("Some previous output\n⠋ Thinking..."),
            AgentStatus::Working
        );
    }

    #[test]
    fn claude_detect_idle() {
        let a = adapter("claude-code");
        assert_eq!(
            a.detect_status("Done.\n\nWhat would you like to do?"),
            AgentStatus::Idle
        );
    }

    #[test]
    fn claude_detect_completion_signal() {
        let a = adapter("claude-code");
        assert!(
            a.detect_completion_signal("Done.\n\nWhat would you like to do?")
                .is_some()
        );
        assert!(a.supports_completion_signal());
    }

    #[test]
    fn claude_detect_auth_failure() {
        let a = adapter("claude-code");
        assert!(matches!(
            a.detect_status("Error: authentication_error - your token has expired"),
            AgentStatus::AuthFailed(_)
        ));
    }

    #[test]
    fn claude_detect_unknown() {
        let a = adapter("claude-code");
        assert_eq!(
            a.detect_status("random output with nothing recognizable"),
            AgentStatus::Unknown
        );
    }

    #[test]
    fn claude_spawn_with_prompt() {
        let a = adapter("claude-code");
        let cmd = a.build_spawn_command(Some("You are a backend developer"));
        assert!(cmd.starts_with("claude "));
        assert!(!cmd.contains("--prompt"));
        assert!(cmd.contains("'You are a backend developer'"));
    }

    #[test]
    fn claude_spawn_without_prompt() {
        let a = adapter("claude-code");
        assert_eq!(a.build_spawn_command(None), "claude");
    }

    #[test]
    fn claude_command_override() {
        let a = get_adapter("claude-code", Some("/usr/local/bin/claude-dev")).unwrap();
        assert_eq!(a.command_name(), "/usr/local/bin/claude-dev");
        assert_eq!(a.build_spawn_command(None), "/usr/local/bin/claude-dev");
    }

    // -- Codex tests --

    #[test]
    fn codex_detect_working_from_spinner() {
        let a = adapter("codex");
        assert_eq!(
            a.detect_status("Some previous output\n⠋ Thinking..."),
            AgentStatus::Working
        );
    }

    #[test]
    fn codex_detect_idle() {
        let a = adapter("codex");
        assert_eq!(
            a.detect_status("Done.\n\nWhat would you like to do?"),
            AgentStatus::Idle
        );
    }

    #[test]
    fn codex_detect_completion_signal() {
        let a = adapter("codex");
        assert!(
            a.detect_completion_signal("Done.\n\nWhat would you like to do?")
                .is_some()
        );
        assert!(a.supports_completion_signal());
    }

    #[test]
    fn codex_detect_auth_failure() {
        let a = adapter("codex");
        assert!(matches!(
            a.detect_status("Error: invalid_api_key - check your OpenAI API key"),
            AgentStatus::AuthFailed(_)
        ));
    }

    #[test]
    fn codex_detect_rate_limit_signal() {
        let a = adapter("codex");
        assert!(
            a.detect_rate_limit("Error: rate_limit_exceeded - try again later")
                .is_some()
        );
    }

    #[test]
    fn codex_detect_unknown() {
        let a = adapter("codex");
        assert_eq!(
            a.detect_status("random output with nothing recognizable"),
            AgentStatus::Unknown
        );
    }

    #[test]
    fn codex_spawn_with_prompt() {
        let a = adapter("codex");
        let cmd = a.build_spawn_command(Some("You are a backend developer"));
        assert!(cmd.contains("codex"));
        assert!(cmd.contains("--prompt"));
    }

    #[test]
    fn codex_spawn_without_prompt() {
        let a = adapter("codex");
        assert_eq!(a.build_spawn_command(None), "codex");
    }

    // -- Aider tests --

    #[test]
    fn aider_detect_working_from_spinner() {
        let a = adapter("aider");
        assert_eq!(
            a.detect_status("Some previous output\n⠋ Applying changes..."),
            AgentStatus::Working
        );
    }

    #[test]
    fn aider_detect_idle() {
        let a = adapter("aider");
        assert_eq!(a.detect_status("Done.\n\naider> "), AgentStatus::Idle);
    }

    #[test]
    fn aider_detect_completion_signal() {
        let a = adapter("aider");
        assert!(a.detect_completion_signal("Done.\n\naider> ").is_some());
        assert!(a.supports_completion_signal());
    }

    #[test]
    fn aider_detect_auth_failure() {
        let a = adapter("aider");
        assert!(matches!(
            a.detect_status("Error: AuthenticationError - invalid credentials"),
            AgentStatus::AuthFailed(_)
        ));
    }

    #[test]
    fn aider_detect_unknown() {
        let a = adapter("aider");
        assert_eq!(
            a.detect_status("random output with nothing recognizable"),
            AgentStatus::Unknown
        );
    }

    #[test]
    fn aider_spawn_with_prompt() {
        let a = adapter("aider");
        let cmd = a.build_spawn_command(Some("You are a backend developer"));
        assert!(cmd.contains("aider"));
        assert!(cmd.contains("--message"));
    }

    #[test]
    fn aider_spawn_without_prompt() {
        let a = adapter("aider");
        assert_eq!(a.build_spawn_command(None), "aider");
    }

    // -- OpenClaw tests --

    #[test]
    fn openclaw_detect_working_from_spinner() {
        let a = adapter("openclaw");
        assert_eq!(
            a.detect_status("Some previous output\n⠋ Thinking..."),
            AgentStatus::Working
        );
    }

    #[test]
    fn openclaw_detect_idle() {
        let a = adapter("openclaw");
        assert_eq!(
            a.detect_status("Done.\n\nWhat would you like to do?"),
            AgentStatus::Idle
        );
    }

    #[test]
    fn openclaw_detect_completion_signal() {
        let a = adapter("openclaw");
        assert!(
            a.detect_completion_signal("Done.\n\nWhat would you like to do?")
                .is_some()
        );
        assert!(a.supports_completion_signal());
    }

    #[test]
    fn openclaw_detect_auth_failure() {
        let a = adapter("openclaw");
        assert!(matches!(
            a.detect_status("Error: authentication_error - token has expired"),
            AgentStatus::AuthFailed(_)
        ));
    }

    #[test]
    fn claude_detect_provider_down_signal() {
        let a = adapter("claude-code");
        assert!(
            a.detect_provider_down("Error: 503 Service Unavailable")
                .is_some()
        );
    }

    #[test]
    fn openclaw_detect_unknown() {
        let a = adapter("openclaw");
        assert_eq!(
            a.detect_status("random output with nothing recognizable"),
            AgentStatus::Unknown
        );
    }

    #[test]
    fn openclaw_spawn_with_prompt() {
        let a = adapter("openclaw");
        let cmd = a.build_spawn_command(Some("You are a backend developer"));
        assert!(cmd.contains("openclaw"));
        assert!(cmd.contains("--prompt"));
    }

    #[test]
    fn openclaw_spawn_without_prompt() {
        let a = adapter("openclaw");
        assert_eq!(a.build_spawn_command(None), "openclaw");
    }

    #[test]
    fn unknown_runtime_returns_none() {
        assert!(get_adapter("unknown", None).is_none());
    }

    #[test]
    fn compatible_override_matches_runtime() {
        assert_eq!(
            compatible_command_override("claude-code", Some("anthropic"), Some("claude-work")),
            Some("claude-work")
        );
        assert_eq!(
            compatible_command_override("codex", Some("openai"), Some("codex-enterprise")),
            Some("codex-enterprise")
        );
        assert_eq!(
            compatible_command_override("openclaw", Some("openclaw"), Some("openclaw")),
            Some("openclaw")
        );
    }

    #[test]
    fn compatible_override_rejects_mismatch() {
        assert_eq!(
            compatible_command_override("codex", Some("anthropic"), Some("claude")),
            None
        );
        assert_eq!(
            compatible_command_override("claude-code", Some("openai"), Some("codex")),
            None
        );
        assert_eq!(
            compatible_command_override("openclaw", Some("openai"), Some("codex")),
            None
        );
    }

    #[test]
    fn spawn_command_with_pre_args_quotes_values() {
        let a = adapter("claude-code");
        let args = vec![
            "--permission-mode".to_string(),
            "dontAsk".to_string(),
            "--settings".to_string(),
            "/tmp/my settings.json".to_string(),
        ];
        let cmd = a.build_spawn_command_with_args(&args, Some("hello"));
        assert!(cmd.contains("'--permission-mode'"));
        assert!(cmd.contains("'dontAsk'"));
        assert!(cmd.contains("'/tmp/my settings.json'"));
        assert!(cmd.ends_with("'hello'"));
    }

    #[test]
    fn fixture_claude_status_variants() {
        let adapter = adapter("claude-code");
        let working = include_str!("../../tests/fixtures/runtime/claude_working_spinner.txt");
        let idle = include_str!("../../tests/fixtures/runtime/claude_idle_prompt.txt");
        let auth_failed = include_str!("../../tests/fixtures/runtime/claude_auth_failed.txt");

        let working_diag = adapter.diagnose(working);
        assert_eq!(working_diag.status, AgentStatus::Working);
        assert!(working_diag.confidence >= 0.65);
        assert!(!working_diag.matched_patterns.is_empty());

        let idle_diag = adapter.diagnose(idle);
        assert_eq!(idle_diag.status, AgentStatus::Idle);
        assert!(idle_diag.confidence >= 0.70);
        assert!(idle_diag.completion_match.is_some());

        let auth_diag = adapter.diagnose(auth_failed);
        assert!(matches!(auth_diag.status, AgentStatus::AuthFailed(_)));
        assert_eq!(auth_diag.confidence, 1.0);
        assert!(auth_diag.auth_match.is_some());
    }

    #[test]
    fn fixture_codex_status_and_signal_variants() {
        let adapter = adapter("codex");
        let working = include_str!("../../tests/fixtures/runtime/codex_working_generating.txt");
        let idle = include_str!("../../tests/fixtures/runtime/codex_idle_completion_variant.txt");
        let rate_limit = include_str!("../../tests/fixtures/runtime/codex_rate_limit.txt");

        let working_diag = adapter.diagnose(working);
        assert_eq!(working_diag.status, AgentStatus::Working);
        assert!(working_diag.confidence >= 0.35);

        let idle_diag = adapter.diagnose(idle);
        assert_eq!(idle_diag.status, AgentStatus::Idle);
        assert!(idle_diag.completion_match.is_some());
        assert!(idle_diag.confidence >= 0.70);

        let signal_diag = adapter.diagnose(rate_limit);
        assert!(signal_diag.rate_limit_match.is_some());
    }

    #[test]
    fn fixture_aider_and_openclaw_signal_variants() {
        let aider = adapter("aider");
        let openclaw = adapter("openclaw");
        let aider_idle = include_str!("../../tests/fixtures/runtime/aider_idle_prompt.txt");
        let provider_down = include_str!("../../tests/fixtures/runtime/openclaw_provider_down.txt");

        let aider_diag = aider.diagnose(aider_idle);
        assert_eq!(aider_diag.status, AgentStatus::Idle);
        assert!(aider_diag.completion_match.is_some());

        let openclaw_diag = openclaw.diagnose(provider_down);
        assert!(openclaw_diag.provider_down_match.is_some());
    }

    #[test]
    fn diagnostics_report_match_explanations() {
        let output = "Generating code...\n⠋ Thinking...\n";
        let diagnostics = diagnose_output("codex", output, None).unwrap();
        assert_eq!(diagnostics.status, AgentStatus::Working);
        assert!(
            diagnostics
                .matched_patterns
                .iter()
                .any(|entry| entry.starts_with("working:"))
        );
    }

    #[test]
    fn diagnostics_prefers_idle_when_completion_marker_present() {
        let output = "Generating...\nWhat would you like to do?\n";
        let diagnostics = diagnose_output("codex", output, None).unwrap();
        assert_eq!(diagnostics.status, AgentStatus::Idle);
    }

    #[test]
    fn diagnose_output_returns_runtime_error_for_unknown_runtime() {
        let err = diagnose_output("unknown", "anything", None).unwrap_err();
        assert!(matches!(err, TuttiError::RuntimeUnknown(runtime) if runtime == "unknown"));
    }
}
