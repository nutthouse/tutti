use super::{
    Multiplexer, SessionMetadata, blocked_inherited_env_vars, command_error, is_valid_env_key,
    shell_escape_value, should_strip_inherited_env_var,
};
use crate::config::TmuxMultiplexerConfig;
use crate::error::{Result, TuttiError};
use std::collections::HashMap;
use std::io::Write;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct TmuxBackend {
    #[allow(dead_code)]
    config: TmuxMultiplexerConfig,
}

impl TmuxBackend {
    pub fn new(config: TmuxMultiplexerConfig) -> Self {
        Self { config }
    }
}

impl Multiplexer for TmuxBackend {
    fn check_available(&self) -> Result<()> {
        which::which("tmux").map_err(|_| TuttiError::TmuxNotInstalled)?;
        Ok(())
    }

    fn spawn_detached(
        &self,
        meta: &SessionMetadata,
        exec_cmd: &str,
        env_vars: &HashMap<String, String>,
    ) -> Result<String> {
        let working_dir = meta.worktree_dir.to_string_lossy();
        let output = Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-s",
                &meta.session_id,
                "-c",
                &working_dir,
            ])
            .output()?;

        if !output.status.success() {
            return Err(command_error(
                "tmux",
                "new-session",
                &meta.session_id,
                &output.stderr,
            ));
        }

        for key in blocked_inherited_env_vars() {
            self.send_text(&meta.session_id, &format!("unset {key}"))?;
        }

        for (key, value) in env_vars {
            if should_strip_inherited_env_var(key) || !is_valid_env_key(key) {
                continue;
            }
            let export_cmd = format!("export {}={}", key, shell_escape_value(value));
            self.send_text(&meta.session_id, &export_cmd)?;
        }

        self.send_text(&meta.session_id, exec_cmd)?;
        Ok(meta.session_id.clone())
    }

    fn attach_interactive(&self, session_id: &str) -> Result<ExitStatus> {
        let status = Command::new("tmux")
            .args(["attach-session", "-t", session_id])
            .status()?;
        if !status.success() {
            return Err(TuttiError::TmuxError(format!(
                "failed to attach to session '{session_id}'"
            )));
        }
        Ok(status)
    }

    fn kill_session(&self, session_id: &str) -> Result<()> {
        let output = Command::new("tmux")
            .args(["kill-session", "-t", session_id])
            .output()?;

        if !output.status.success() {
            return Err(command_error(
                "tmux",
                "kill-session",
                session_id,
                &output.stderr,
            ));
        }
        Ok(())
    }

    fn is_alive(&self, session_id: &str) -> Result<bool> {
        Ok(Command::new("tmux")
            .args(["has-session", "-t", session_id])
            .output()
            .is_ok_and(|out| out.status.success()))
    }

    fn capture_pane(&self, session_id: &str, lines: u32) -> Result<String> {
        let start_line = -(lines as i64);
        let output = Command::new("tmux")
            .args([
                "capture-pane",
                "-t",
                session_id,
                "-p",
                "-S",
                &start_line.to_string(),
            ])
            .output()?;

        if !output.status.success() {
            return Err(command_error(
                "tmux",
                "capture-pane",
                session_id,
                &output.stderr,
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn send_text(&self, session_id: &str, text: &str) -> Result<()> {
        if !self.is_alive(session_id)? {
            return Err(TuttiError::TmuxError(format!(
                "session '{}' is not running",
                session_id
            )));
        }

        let is_multiline = text.contains('\n');

        if !text.is_empty() {
            send_text_via_tmux_buffer(session_id, text, is_multiline)?;
        }

        self.send_enter_presses(session_id, if is_multiline { 2 } else { 1 })?;
        Ok(())
    }

    fn send_enter_presses(&self, session_id: &str, count: u32) -> Result<()> {
        if !self.is_alive(session_id)? {
            return Err(TuttiError::TmuxError(format!(
                "session '{}' is not running",
                session_id
            )));
        }
        for _ in 0..count.max(1) {
            send_enter(session_id)?;
        }
        Ok(())
    }

    fn set_status_bar(&self, session_id: &str, text: &str) -> Result<()> {
        let _ = Command::new("tmux")
            .args(["set-option", "-t", session_id, "status", "on"])
            .output();
        let _ = Command::new("tmux")
            .args([
                "set-option",
                "-t",
                session_id,
                "status-style",
                "bg=#1a1a2e,fg=#e0e0e0",
            ])
            .output();
        let _ = Command::new("tmux")
            .args(["set-option", "-t", session_id, "status-left-length", "120"])
            .output();
        let _ = Command::new("tmux")
            .args(["set-option", "-t", session_id, "status-left", text])
            .output();
        let _ = Command::new("tmux")
            .args(["set-option", "-t", session_id, "status-right", ""])
            .output();
        Ok(())
    }
}

fn send_enter(session: &str) -> Result<()> {
    let out = Command::new("tmux")
        .args(["send-keys", "-t", session, "Enter"])
        .output()?;
    if !out.status.success() {
        return Err(command_error("tmux", "send-keys", session, &out.stderr));
    }
    Ok(())
}

fn send_text_via_tmux_buffer(session: &str, text: &str, bracketed: bool) -> Result<()> {
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
        return Err(command_error(
            "tmux",
            "load-buffer",
            session,
            &load_output.stderr,
        ));
    }

    let mut paste_args = vec!["paste-buffer"];
    if bracketed {
        paste_args.push("-p");
    }
    paste_args.extend(["-b", &buffer_name, "-t", session]);
    let paste_output = Command::new("tmux").args(&paste_args).output()?;
    if !paste_output.status.success() {
        return Err(command_error(
            "tmux",
            "paste-buffer",
            session,
            &paste_output.stderr,
        ));
    }

    let delete_output = Command::new("tmux")
        .args(["delete-buffer", "-b", &buffer_name])
        .output()?;
    if !delete_output.status.success() {
        return Err(command_error(
            "tmux",
            "delete-buffer",
            session,
            &delete_output.stderr,
        ));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{is_valid_env_key, should_strip_inherited_env_var};

    #[test]
    fn strips_claudecode_env_var_case_insensitive() {
        assert!(should_strip_inherited_env_var("CLAUDECODE"));
        assert!(should_strip_inherited_env_var("claudecode"));
    }

    #[test]
    fn validates_shell_env_identifiers() {
        assert!(is_valid_env_key("FOO"));
        assert!(is_valid_env_key("_FOO_1"));
        assert!(!is_valid_env_key(""));
        assert!(!is_valid_env_key("1FOO"));
        assert!(!is_valid_env_key("BAD-NAME"));
        assert!(!is_valid_env_key("BAD; echo nope"));
    }
}
