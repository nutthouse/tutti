use crate::error::{Result, TuttiError};
use std::process::Command;

/// Check that tmux is installed and on PATH.
pub fn check_tmux() -> Result<()> {
    which::which("tmux").map_err(|_| TuttiError::TmuxNotInstalled)?;
    Ok(())
}

pub struct TmuxSession;

impl TmuxSession {
    /// Build a session name following the convention: tutti-{team}-{agent}
    pub fn session_name(team: &str, agent: &str) -> String {
        format!("tutti-{team}-{agent}")
    }

    /// Check if a tmux session exists.
    pub fn session_exists(session: &str) -> bool {
        Command::new("tmux")
            .args(["has-session", "-t", session])
            .output()
            .is_ok_and(|out| out.status.success())
    }

    /// Create a new tmux session running the given command.
    /// The session starts detached.
    pub fn create_session(session: &str, working_dir: &str, shell_cmd: &str) -> Result<()> {
        let output = Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-s",
                session,
                "-c",
                working_dir,
                shell_cmd,
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TuttiError::TmuxError(format!(
                "failed to create session '{session}': {stderr}"
            )));
        }
        Ok(())
    }

    /// Kill a tmux session.
    pub fn kill_session(session: &str) -> Result<()> {
        let output = Command::new("tmux")
            .args(["kill-session", "-t", session])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TuttiError::TmuxError(format!(
                "failed to kill session '{session}': {stderr}"
            )));
        }
        Ok(())
    }

    /// Capture the visible pane content of a session.
    pub fn capture_pane(session: &str, lines: u32) -> Result<String> {
        let start_line = -(lines as i64);
        let output = Command::new("tmux")
            .args([
                "capture-pane",
                "-t",
                session,
                "-p",
                "-S",
                &start_line.to_string(),
            ])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TuttiError::TmuxError(format!(
                "failed to capture pane for '{session}': {stderr}"
            )));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// List all tutti-prefixed tmux sessions.
    pub fn list_tutti_sessions() -> Result<Vec<String>> {
        let output = Command::new("tmux")
            .args(["list-sessions", "-F", "#{session_name}"])
            .output()?;

        if !output.status.success() {
            // No server running = no sessions, not an error
            return Ok(vec![]);
        }

        let sessions = String::from_utf8_lossy(&output.stdout)
            .lines()
            .filter(|s| s.starts_with("tutti-"))
            .map(|s| s.to_string())
            .collect();
        Ok(sessions)
    }

    /// Exec into tmux attach (replaces the current process on unix).
    pub fn attach_session(session: &str) -> Result<()> {
        let status = Command::new("tmux")
            .args(["attach-session", "-t", session])
            .status()?;

        if !status.success() {
            return Err(TuttiError::TmuxError(format!(
                "failed to attach to session '{session}'"
            )));
        }
        Ok(())
    }

    /// Attach read-only to a tmux session.
    pub fn attach_readonly(session: &str) -> Result<()> {
        let status = Command::new("tmux")
            .args(["attach-session", "-t", session, "-r"])
            .status()?;

        if !status.success() {
            return Err(TuttiError::TmuxError(format!(
                "failed to attach (readonly) to session '{session}'"
            )));
        }
        Ok(())
    }
}
