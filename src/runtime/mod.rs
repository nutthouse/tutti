pub mod aider;
pub mod claude_code;
pub mod codex;
pub mod openclaw;

/// Status of an agent as detected from terminal output.
#[derive(Debug, Clone, PartialEq, Eq)]
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
    working_patterns: &'static [&'static str],
    idle_patterns: &'static [&'static str],
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
        if let Some(reason) = self.detect_auth_failure(terminal_output) {
            return AgentStatus::AuthFailed(reason);
        }

        let recent: String = terminal_output
            .lines()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");

        for pattern in self.config.working_patterns {
            if recent.contains(pattern) {
                return AgentStatus::Working;
            }
        }

        for pattern in self.config.idle_patterns {
            if recent.contains(pattern) {
                return AgentStatus::Idle;
            }
        }

        AgentStatus::Unknown
    }

    fn detect_auth_failure(&self, terminal_output: &str) -> Option<String> {
        let lower = terminal_output.to_lowercase();
        for pattern in self.config.auth_patterns {
            if lower.contains(&pattern.to_lowercase()) {
                return Some(pattern.to_string());
            }
        }
        None
    }
}

fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
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
    fn codex_detect_auth_failure() {
        let a = adapter("codex");
        assert!(matches!(
            a.detect_status("Error: invalid_api_key - check your OpenAI API key"),
            AgentStatus::AuthFailed(_)
        ));
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
    fn openclaw_detect_auth_failure() {
        let a = adapter("openclaw");
        assert!(matches!(
            a.detect_status("Error: authentication_error - token has expired"),
            AgentStatus::AuthFailed(_)
        ));
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
}
