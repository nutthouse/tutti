use crate::error::Result;
use crate::multiplexer::{SessionMetadata, current_backend};
use std::collections::HashMap;
use std::path::PathBuf;

/// Check that the configured terminal multiplexer is installed and on PATH.
///
/// Kept under the old name for compatibility with existing call sites.
pub fn check_tmux() -> Result<()> {
    current_backend().check_available()
}

pub struct TmuxSession;

impl TmuxSession {
    /// Build a session name following the convention: tutti-{team}-{agent}
    pub fn session_name(team: &str, agent: &str) -> String {
        format!("tutti-{team}-{agent}")
    }

    pub fn session_exists(session: &str) -> bool {
        current_backend().is_alive(session).unwrap_or(false)
    }

    pub fn create_session(
        session: &str,
        working_dir: &str,
        shell_cmd: &str,
        env_vars: &HashMap<String, String>,
    ) -> Result<()> {
        let meta = SessionMetadata {
            session_id: session.to_string(),
            target_voice: session.to_string(),
            worktree_dir: PathBuf::from(working_dir),
        };
        current_backend()
            .spawn_detached(&meta, shell_cmd, env_vars)
            .map(|_| ())
    }

    pub fn kill_session(session: &str) -> Result<()> {
        current_backend().kill_session(session)
    }

    pub fn capture_pane(session: &str, lines: u32) -> Result<String> {
        current_backend().capture_pane(session, lines)
    }

    pub fn send_text(session: &str, text: &str) -> Result<()> {
        current_backend().send_text(session, text)
    }

    pub fn send_enter_presses(session: &str, count: u32) -> Result<()> {
        current_backend().send_enter_presses(session, count)
    }

    pub fn set_status_bar(session: &str, text: &str) -> Result<()> {
        current_backend().set_status_bar(session, text)
    }

    pub fn attach_session(session: &str) -> Result<()> {
        current_backend().attach_interactive(session).map(|_| ())
    }
}
