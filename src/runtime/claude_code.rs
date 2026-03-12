use super::{AgentStatus, RuntimeAdapter};

pub struct ClaudeCodeAdapter;

/// Patterns that indicate auth failures in Claude Code output.
const AUTH_FAILURE_PATTERNS: &[&str] = &[
    "invalid_api_key",
    "authentication_error",
    "token has expired",
    "unauthorized",
    "OAuth token",
    "session expired",
    "please log in",
    "not authenticated",
    "APIError: 401",
    "APIError: 403",
];

/// Patterns that indicate the agent is actively working.
const WORKING_PATTERNS: &[&str] = &[
    "⠋",
    "⠙",
    "⠹",
    "⠸",
    "⠼",
    "⠴",
    "⠦",
    "⠧",
    "⠇",
    "⠏", // spinner
    "Thinking",
    "Reading",
    "Writing",
    "Editing",
    "Running",
    "Searching",
];

/// Patterns that indicate the agent is idle / waiting for input.
const IDLE_PATTERNS: &[&str] = &[
    "What would you like to do?",
    "How can I help",
    "> ",
    "Claude Code",
];

impl RuntimeAdapter for ClaudeCodeAdapter {
    fn command_name(&self) -> &str {
        "claude"
    }

    fn is_available(&self) -> bool {
        which::which("claude").is_ok()
    }

    fn build_spawn_command(&self, prompt: Option<&str>) -> String {
        match prompt {
            Some(p) => format!("claude --prompt {}", shell_escape(p)),
            None => "claude".to_string(),
        }
    }

    fn detect_status(&self, terminal_output: &str) -> AgentStatus {
        // Check auth failure first (highest priority)
        if let Some(reason) = self.detect_auth_failure(terminal_output) {
            return AgentStatus::AuthFailed(reason);
        }

        // Check last ~20 lines for activity signals
        let recent: String = terminal_output
            .lines()
            .rev()
            .take(20)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("\n");

        // Check for working indicators
        for pattern in WORKING_PATTERNS {
            if recent.contains(pattern) {
                return AgentStatus::Working;
            }
        }

        // Check for idle indicators
        for pattern in IDLE_PATTERNS {
            if recent.contains(pattern) {
                return AgentStatus::Idle;
            }
        }

        AgentStatus::Unknown
    }

    fn detect_auth_failure(&self, terminal_output: &str) -> Option<String> {
        let lower = terminal_output.to_lowercase();
        for pattern in AUTH_FAILURE_PATTERNS {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_working_from_spinner() {
        let adapter = ClaudeCodeAdapter;
        let output = "Some previous output\n⠋ Thinking...";
        assert_eq!(adapter.detect_status(output), AgentStatus::Working);
    }

    #[test]
    fn detect_idle() {
        let adapter = ClaudeCodeAdapter;
        let output = "Done.\n\nWhat would you like to do?";
        assert_eq!(adapter.detect_status(output), AgentStatus::Idle);
    }

    #[test]
    fn detect_auth_failure() {
        let adapter = ClaudeCodeAdapter;
        let output = "Error: authentication_error - your token has expired";
        assert!(matches!(
            adapter.detect_status(output),
            AgentStatus::AuthFailed(_)
        ));
    }

    #[test]
    fn detect_unknown_when_no_signals() {
        let adapter = ClaudeCodeAdapter;
        let output = "random output with nothing recognizable";
        assert_eq!(adapter.detect_status(output), AgentStatus::Unknown);
    }

    #[test]
    fn build_spawn_with_prompt() {
        let adapter = ClaudeCodeAdapter;
        let cmd = adapter.build_spawn_command(Some("You are a backend developer"));
        assert!(cmd.contains("claude"));
        assert!(cmd.contains("--prompt"));
    }

    #[test]
    fn build_spawn_without_prompt() {
        let adapter = ClaudeCodeAdapter;
        let cmd = adapter.build_spawn_command(None);
        assert_eq!(cmd, "claude");
    }
}
