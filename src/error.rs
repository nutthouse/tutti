use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum TuttiError {
    #[error("tutti.toml already exists in {0}")]
    ConfigAlreadyExists(PathBuf),

    #[error("tutti.toml not found (searched from {0} to filesystem root)")]
    ConfigNotFound(PathBuf),

    #[error("failed to parse tutti.toml: {0}")]
    ConfigParse(String),

    #[error("invalid config: {0}")]
    ConfigValidation(String),

    #[error("tmux is not installed. Install with: brew install tmux")]
    TmuxNotInstalled,

    #[error("tmux command failed: {0}")]
    TmuxError(String),

    #[error("runtime '{0}' is not installed or not on PATH")]
    RuntimeNotAvailable(String),

    #[error("unknown runtime: {0}")]
    RuntimeUnknown(String),

    #[error("agent '{0}' not found in config")]
    AgentNotFound(String),

    #[error("agent '{0}' is not running")]
    AgentNotRunning(String),

    #[error("git error: {0}")]
    Git(String),

    #[error("worktree error: {0}")]
    Worktree(String),

    #[error("state error: {0}")]
    State(String),

    #[error("usage data error: {0}")]
    UsageData(String),

    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Json(#[from] serde_json::Error),

    #[error("issue claim error: {0}")]
    IssueClaim(String),
}

pub type Result<T> = std::result::Result<T, TuttiError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agent_not_found_message_mentions_agent_name() {
        let err = TuttiError::AgentNotFound("backend".to_string());
        assert_eq!(err.to_string(), "agent 'backend' not found in config");
    }

    #[test]
    fn config_already_exists_message_mentions_target_path() {
        let err = TuttiError::ConfigAlreadyExists(PathBuf::from("/tmp/tutti.toml"));
        assert!(err.to_string().contains("/tmp/tutti.toml"));
    }
}
