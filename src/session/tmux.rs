use crate::error::{Result, TuttiError};
use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const BLOCKED_INHERITED_ENV_VARS: &[&str] = &["CLAUDECODE"];

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

    /// Create a new tmux session and run the given command inside it.
    /// The session starts a normal shell, then sends the command via `send-keys`
    /// so the shell survives if the command exits. Environment variables are
    /// exported before the command runs.
    pub fn create_session(
        session: &str,
        working_dir: &str,
        shell_cmd: &str,
        env_vars: &HashMap<String, String>,
    ) -> Result<()> {
        // Create a detached session with a normal shell
        let output = Command::new("tmux")
            .args(["new-session", "-d", "-s", session, "-c", working_dir])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(TuttiError::TmuxError(format!(
                "failed to create session '{session}': {stderr}"
            )));
        }

        // Avoid nested Claude Code detection when Tutti is launched from inside Claude Code.
        for key in BLOCKED_INHERITED_ENV_VARS {
            Self::send_text(session, &format!("unset {key}"))?;
        }

        // Export env vars into the shell
        for (key, value) in env_vars {
            if should_strip_inherited_env_var(key) {
                continue;
            }
            let export_cmd = format!("export {}={}", key, shell_escape_value(value));
            Self::send_text(session, &export_cmd)?;
        }

        // Send the actual command
        Self::send_text(session, shell_cmd)?;

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

    /// Send text to a running session and press Enter.
    ///
    /// The entire text is pasted as a single tmux buffer, then one Enter
    /// is sent to submit it. This ensures multi-line prompts arrive
    /// atomically instead of being interpreted line-by-line (which would
    /// break non-TUI targets like a bare shell prompt).
    pub fn send_text(session: &str, text: &str) -> Result<()> {
        if !Self::session_exists(session) {
            return Err(TuttiError::TmuxError(format!(
                "session '{}' is not running",
                session
            )));
        }

        if !text.is_empty() {
            send_text_via_tmux_buffer(session, text)?;
        }

        let out = Command::new("tmux")
            .args(["send-keys", "-t", session, "Enter"])
            .output()?;
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(TuttiError::TmuxError(format!(
                "failed to send Enter to '{session}': {stderr}"
            )));
        }

        Ok(())
    }

    /// Set a sticky status bar on a session (bottom line).
    pub fn set_status_bar(session: &str, text: &str) -> Result<()> {
        // Enable status bar for this session
        let _ = Command::new("tmux")
            .args(["set-option", "-t", session, "status", "on"])
            .output();
        let _ = Command::new("tmux")
            .args([
                "set-option",
                "-t",
                session,
                "status-style",
                "bg=#1a1a2e,fg=#e0e0e0",
            ])
            .output();
        let _ = Command::new("tmux")
            .args(["set-option", "-t", session, "status-left-length", "120"])
            .output();
        let _ = Command::new("tmux")
            .args(["set-option", "-t", session, "status-left", text])
            .output();
        let _ = Command::new("tmux")
            .args(["set-option", "-t", session, "status-right", ""])
            .output();
        Ok(())
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
}

/// Shell-escape a value for use in `env KEY=VALUE` commands.
fn shell_escape_value(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Load text into a tmux buffer and paste it into the target session.
///
/// Handles multi-line text atomically — the entire payload arrives as a
/// single paste event so the receiving application (claude-code, codex, zsh)
/// sees it all at once rather than line-by-line.
fn send_text_via_tmux_buffer(session: &str, text: &str) -> Result<()> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let buffer_name = format!("tutti-send-{}-{nanos}", std::process::id());

    let mut child = Command::new("tmux")
        .args(["load-buffer", "-b", &buffer_name, "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(text.as_bytes())?;
    }
    let load_output = child.wait_with_output()?;
    if !load_output.status.success() {
        let stderr = String::from_utf8_lossy(&load_output.stderr);
        return Err(TuttiError::TmuxError(format!(
            "failed to load tmux buffer for '{session}': {stderr}"
        )));
    }

    // -p enables bracketed paste mode (\e[200~ ... \e[201~) so the
    // receiving application treats the entire payload as a single paste
    // event rather than splitting on embedded newlines.
    let paste_output = Command::new("tmux")
        .args([
            "paste-buffer",
            "-d",
            "-p",
            "-b",
            &buffer_name,
            "-t",
            session,
        ])
        .output()?;
    if !paste_output.status.success() {
        let stderr = String::from_utf8_lossy(&paste_output.stderr);
        return Err(TuttiError::TmuxError(format!(
            "failed to paste text to '{session}': {stderr}"
        )));
    }

    Ok(())
}

fn should_strip_inherited_env_var(key: &str) -> bool {
    BLOCKED_INHERITED_ENV_VARS
        .iter()
        .any(|blocked| key.eq_ignore_ascii_case(blocked))
}

#[cfg(test)]
mod tests {
    use super::should_strip_inherited_env_var;

    #[test]
    fn strips_claudecode_env_var_case_insensitive() {
        assert!(should_strip_inherited_env_var("CLAUDECODE"));
        assert!(should_strip_inherited_env_var("claudecode"));
    }

    #[test]
    fn does_not_strip_unrelated_env_var() {
        assert!(!should_strip_inherited_env_var("OPENAI_API_KEY"));
    }
}
