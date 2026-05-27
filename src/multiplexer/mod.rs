use crate::config::{MultiplexerConfig, MultiplexerType, TuttiConfig};
use crate::error::{Result, TuttiError};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::ExitStatus;
use std::sync::{Mutex, OnceLock};

pub mod tmux;
pub mod zellij;

#[derive(Debug, Clone)]
pub struct SessionMetadata {
    /// Stable backend session identifier, usually `tutti-{workspace}-{agent}`.
    pub session_id: String,
    /// Agent name used for backend labels such as Zellij pane names.
    pub target_agent: String,
    /// Working directory where the agent runtime command starts.
    pub worktree_dir: PathBuf,
}

pub trait Multiplexer: Send + Sync {
    /// Verify that the backend executable is available.
    fn check_available(&self) -> Result<()>;
    /// Spawn a detached session or pane that runs the provided agent command.
    fn spawn_detached(
        &self,
        meta: &SessionMetadata,
        exec_cmd: &str,
        env_vars: &HashMap<String, String>,
    ) -> Result<String>;
    /// Attach the user's terminal to the running backend session.
    fn attach_interactive(&self, session_id: &str) -> Result<ExitStatus>;
    /// Terminate a backend session.
    fn kill_session(&self, session_id: &str) -> Result<()>;
    /// Check whether a backend session is still present.
    fn is_alive(&self, session_id: &str) -> Result<bool>;
    /// Capture recent terminal output for status, health, dashboard, and logs.
    fn capture_pane(&self, session_id: &str, lines: u32) -> Result<String>;
    /// Send text to the target pane and submit it.
    fn send_text(&self, session_id: &str, text: &str) -> Result<()>;
    /// Send one or more Enter keypresses to the target pane.
    fn send_enter_presses(&self, session_id: &str, count: u32) -> Result<()>;
    /// Set a backend-specific status hint when supported.
    fn set_status_bar(&self, session_id: &str, text: &str) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct RuntimeMultiplexerConfig {
    /// Selected backend kind.
    pub kind: MultiplexerType,
    /// Backend-specific settings parsed from `tutti.toml`.
    pub config: MultiplexerConfig,
}

impl Default for RuntimeMultiplexerConfig {
    fn default() -> Self {
        Self {
            kind: MultiplexerType::Tmux,
            config: MultiplexerConfig::default(),
        }
    }
}

static CURRENT_CONFIG: OnceLock<Mutex<RuntimeMultiplexerConfig>> = OnceLock::new();

pub fn set_current_config(config: &TuttiConfig) {
    let runtime = RuntimeMultiplexerConfig {
        kind: config.orchestrator.multiplexer_type,
        config: config.multiplexer.clone(),
    };
    let lock = CURRENT_CONFIG.get_or_init(|| Mutex::new(RuntimeMultiplexerConfig::default()));
    let mut guard = lock.lock().unwrap_or_else(|poison| poison.into_inner());
    *guard = runtime;
}

pub fn current_backend() -> Box<dyn Multiplexer> {
    let lock = CURRENT_CONFIG.get_or_init(|| Mutex::new(load_runtime_config_from_cwd()));
    let runtime = lock
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    create_multiplexer(&runtime)
}

pub fn create_multiplexer(runtime: &RuntimeMultiplexerConfig) -> Box<dyn Multiplexer> {
    match runtime.kind {
        MultiplexerType::Zellij => {
            Box::new(zellij::ZellijBackend::new(runtime.config.zellij.clone()))
        }
        MultiplexerType::Tmux => Box::new(tmux::TmuxBackend::new(runtime.config.tmux.clone())),
    }
}

fn load_runtime_config_from_cwd() -> RuntimeMultiplexerConfig {
    let Ok(cwd) = std::env::current_dir() else {
        return RuntimeMultiplexerConfig::default();
    };
    let Ok((config, _)) = TuttiConfig::load_without_side_effect(&cwd) else {
        return RuntimeMultiplexerConfig::default();
    };
    RuntimeMultiplexerConfig {
        kind: config.orchestrator.multiplexer_type,
        config: config.multiplexer,
    }
}

pub(crate) fn command_error(tool: &str, action: &str, session: &str, stderr: &[u8]) -> TuttiError {
    TuttiError::MultiplexerError(format!(
        "{tool} {action} failed for '{session}': {}",
        String::from_utf8_lossy(stderr).trim()
    ))
}

pub(crate) fn shell_escape_value(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

pub(crate) fn should_strip_inherited_env_var(key: &str) -> bool {
    const BLOCKED_INHERITED_ENV_VARS: &[&str] = &["CLAUDECODE"];
    BLOCKED_INHERITED_ENV_VARS
        .iter()
        .any(|blocked| key.eq_ignore_ascii_case(blocked))
}

pub(crate) fn blocked_inherited_env_vars() -> &'static [&'static str] {
    &["CLAUDECODE"]
}

pub(crate) fn is_valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if !(first == '_' || first.is_ascii_alphabetic()) {
        return false;
    }
    chars.all(|c| c == '_' || c.is_ascii_alphanumeric())
}
