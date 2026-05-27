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
    pub session_id: String,
    pub target_agent: String,
    pub worktree_dir: PathBuf,
}

pub trait Multiplexer: Send + Sync {
    fn check_available(&self) -> Result<()>;
    fn spawn_detached(
        &self,
        meta: &SessionMetadata,
        exec_cmd: &str,
        env_vars: &HashMap<String, String>,
    ) -> Result<String>;
    fn attach_interactive(&self, session_id: &str) -> Result<ExitStatus>;
    fn kill_session(&self, session_id: &str) -> Result<()>;
    fn is_alive(&self, session_id: &str) -> Result<bool>;
    fn capture_pane(&self, session_id: &str, lines: u32) -> Result<String>;
    fn send_text(&self, session_id: &str, text: &str) -> Result<()>;
    fn send_enter_presses(&self, session_id: &str, count: u32) -> Result<()>;
    fn set_status_bar(&self, session_id: &str, text: &str) -> Result<()>;
}

#[derive(Debug, Clone)]
pub struct RuntimeMultiplexerConfig {
    pub kind: MultiplexerType,
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
    if let Ok(mut guard) = lock.lock() {
        *guard = runtime;
    }
}

pub fn current_backend() -> Box<dyn Multiplexer> {
    let runtime = CURRENT_CONFIG
        .get_or_init(|| Mutex::new(load_runtime_config_from_cwd()))
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
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
