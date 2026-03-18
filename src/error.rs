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
    fn displays_actionable_tmux_install_message() {
        assert_eq!(
            TuttiError::TmuxNotInstalled.to_string(),
            "tmux is not installed. Install with: brew install tmux"
        );
    }

    #[test]
    fn displays_agent_not_running_with_agent_name() {
        assert_eq!(
            TuttiError::AgentNotRunning("reviewer".to_string()).to_string(),
            "agent 'reviewer' is not running"
        );
    }

    #[test]
    fn converts_io_error_into_tutti_error() {
        let err = std::fs::read_to_string("/definitely/not/here")
            .map_err(TuttiError::from)
            .unwrap_err();

        assert!(matches!(err, TuttiError::Io(_)));
    }
}
