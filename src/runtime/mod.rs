pub mod claude_code;

/// Status of an agent as detected from terminal output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentStatus {
    /// Agent is actively working (generating output).
    Working,
    /// Agent is idle / waiting for input.
    Idle,
    /// Agent encountered an error.
    Errored,
    /// Agent's auth token has expired or is invalid.
    AuthFailed(String),
    /// Session exists but status is unknown.
    Unknown,
    /// Session is not running.
    Stopped,
}

impl std::fmt::Display for AgentStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AgentStatus::Working => write!(f, "Working"),
            AgentStatus::Idle => write!(f, "Idle"),
            AgentStatus::Errored => write!(f, "Errored"),
            AgentStatus::AuthFailed(msg) => write!(f, "Auth Failed: {msg}"),
            AgentStatus::Unknown => write!(f, "Unknown"),
            AgentStatus::Stopped => write!(f, "Stopped"),
        }
    }
}

/// Trait for runtime-specific behavior.
pub trait RuntimeAdapter {
    /// The CLI command name (e.g., "claude").
    fn command_name(&self) -> &str;

    /// Check if the runtime CLI is available on PATH.
    fn is_available(&self) -> bool;

    /// Build the shell command string to spawn this runtime.
    fn build_spawn_command(&self, prompt: Option<&str>) -> String;

    /// Detect agent status from captured terminal output.
    fn detect_status(&self, terminal_output: &str) -> AgentStatus;

    /// Check if terminal output indicates an auth failure.
    fn detect_auth_failure(&self, terminal_output: &str) -> Option<String>;
}

/// Get a runtime adapter by name.
pub fn get_adapter(runtime: &str) -> Option<Box<dyn RuntimeAdapter>> {
    match runtime {
        "claude-code" => Some(Box::new(claude_code::ClaudeCodeAdapter)),
        _ => None,
    }
}
