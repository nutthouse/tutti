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
}

pub type Result<T> = std::result::Result<T, TuttiError>;

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn config_already_exists_display() {
        let err = TuttiError::ConfigAlreadyExists(PathBuf::from("/tmp/project"));
        assert_eq!(err.to_string(), "tutti.toml already exists in /tmp/project");
    }

    #[test]
    fn config_not_found_display() {
        let err = TuttiError::ConfigNotFound(PathBuf::from("/home/user"));
        assert!(err
            .to_string()
            .contains("tutti.toml not found (searched from /home/user"));
    }

    #[test]
    fn tmux_not_installed_display() {
        let err = TuttiError::TmuxNotInstalled;
        assert!(err.to_string().contains("tmux is not installed"));
        assert!(err.to_string().contains("brew install tmux"));
    }

    #[test]
    fn agent_not_found_display() {
        let err = TuttiError::AgentNotFound("ghost".to_string());
        assert_eq!(err.to_string(), "agent 'ghost' not found in config");
    }

    #[test]
    fn agent_not_running_display() {
        let err = TuttiError::AgentNotRunning("backend".to_string());
        assert_eq!(err.to_string(), "agent 'backend' is not running");
    }

    #[test]
    fn runtime_unknown_display() {
        let err = TuttiError::RuntimeUnknown("myruntime".to_string());
        assert_eq!(err.to_string(), "unknown runtime: myruntime");
    }

    #[test]
    fn io_error_converts() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let tutti_err: TuttiError = io_err.into();
        assert!(matches!(tutti_err, TuttiError::Io(_)));
        assert!(tutti_err.to_string().contains("file missing"));
    }

    #[test]
    fn json_error_converts() {
        let json_err = serde_json::from_str::<serde_json::Value>("not json").unwrap_err();
        let tutti_err: TuttiError = json_err.into();
        assert!(matches!(tutti_err, TuttiError::Json(_)));
    }
}
