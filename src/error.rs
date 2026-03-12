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

    #[error("agent '{0}' is already running")]
    AgentAlreadyRunning(String),

    #[error("agent '{0}' is not running")]
    AgentNotRunning(String),

    #[error("git error: {0}")]
    Git(String),

    #[error("worktree error: {0}")]
    Worktree(String),

    #[error("state error: {0}")]
    State(String),

    #[error("auth failure detected for agent '{0}': {1}")]
    AuthFailure(String, String),

    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, TuttiError>;
