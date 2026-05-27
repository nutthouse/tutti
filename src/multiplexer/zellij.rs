use super::{
    Multiplexer, SessionMetadata, blocked_inherited_env_vars, command_error, is_valid_env_key,
    shell_escape_value, should_strip_inherited_env_var,
};
use crate::config::ZellijMultiplexerConfig;
use crate::error::{Result, TuttiError};
use std::collections::HashMap;
use std::process::{Command, ExitStatus};
use std::thread;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct ZellijBackend {
    config: ZellijMultiplexerConfig,
}

impl ZellijBackend {
    pub fn new(config: ZellijMultiplexerConfig) -> Self {
        Self { config }
    }

    fn primary_pane_id(&self, session_id: &str) -> Result<String> {
        let output = Command::new("zellij")
            .args([
                "--session",
                session_id,
                "action",
                "list-panes",
                "--json",
                "--all",
                "--command",
                "--state",
                "--tab",
            ])
            .output()?;
        if !output.status.success() {
            return Err(command_error(
                "zellij",
                "list-panes",
                session_id,
                &output.stderr,
            ));
        }
        parse_primary_pane_id(&output.stdout).ok_or_else(|| {
            TuttiError::MultiplexerError(format!(
                "zellij session '{session_id}' has no selectable terminal pane"
            ))
        })
    }
}

impl Multiplexer for ZellijBackend {
    fn check_available(&self) -> Result<()> {
        which::which("zellij").map_err(|_| {
            TuttiError::MultiplexerError(
                "zellij is not installed. Install it and re-run the command".to_string(),
            )
        })?;
        Ok(())
    }

    fn spawn_detached(
        &self,
        meta: &SessionMetadata,
        exec_cmd: &str,
        env_vars: &HashMap<String, String>,
    ) -> Result<String> {
        let mut create = Command::new("zellij");
        create.args(["attach", "--create-background", &meta.session_id]);
        let create_output = create.output()?;
        if !create_output.status.success() {
            return Err(command_error(
                "zellij",
                "attach --create-background",
                &meta.session_id,
                &create_output.stderr,
            ));
        }

        let launch_script = build_launch_script(exec_cmd, env_vars);
        let mut run = Command::new("zellij");
        run.args([
            "--session",
            &meta.session_id,
            "run",
            "--cwd",
            &meta.worktree_dir.to_string_lossy(),
            "--name",
            &meta.target_agent,
            "--",
            "bash",
            "-lc",
            &launch_script,
        ]);
        if let Some(theme) = self.config.theme.as_deref()
            && !theme.trim().is_empty()
        {
            run.env("ZELLIJ_THEME", theme);
        }
        let output = run.output()?;
        if !output.status.success() {
            let _ = self.kill_session(&meta.session_id);
            return Err(command_error(
                "zellij",
                "run",
                &meta.session_id,
                &output.stderr,
            ));
        }

        let pane_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if pane_id.is_empty() {
            thread::sleep(Duration::from_millis(100));
            return self.primary_pane_id(&meta.session_id);
        }
        Ok(pane_id)
    }

    fn attach_interactive(&self, session_id: &str) -> Result<ExitStatus> {
        let status = Command::new("zellij")
            .args(["attach", session_id])
            .status()?;
        if !status.success() {
            return Err(TuttiError::MultiplexerError(format!(
                "failed to attach to zellij session '{session_id}'"
            )));
        }
        Ok(status)
    }

    fn kill_session(&self, session_id: &str) -> Result<()> {
        let output = Command::new("zellij")
            .args(["kill-session", session_id])
            .output()?;
        if !output.status.success() {
            return Err(command_error(
                "zellij",
                "kill-session",
                session_id,
                &output.stderr,
            ));
        }
        Ok(())
    }

    fn is_alive(&self, session_id: &str) -> Result<bool> {
        let output = Command::new("zellij")
            .args(["list-sessions", "--short", "--no-formatting"])
            .output()?;
        if !output.status.success() {
            return Err(command_error(
                "zellij",
                "list-sessions",
                session_id,
                &output.stderr,
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .any(|line| line.trim() == session_id))
    }

    fn capture_pane(&self, session_id: &str, lines: u32) -> Result<String> {
        let pane_id = self.primary_pane_id(session_id)?;
        let output = Command::new("zellij")
            .args([
                "--session",
                session_id,
                "action",
                "dump-screen",
                "--pane-id",
                &pane_id,
                "--full",
                "--ansi",
            ])
            .output()?;
        if !output.status.success() {
            return Err(command_error(
                "zellij",
                "dump-screen",
                session_id,
                &output.stderr,
            ));
        }
        Ok(tail_lines(&String::from_utf8_lossy(&output.stdout), lines))
    }

    fn send_text(&self, session_id: &str, text: &str) -> Result<()> {
        let pane_id = self.primary_pane_id(session_id)?;
        if !text.is_empty() {
            let action = if text.contains('\n') {
                "paste"
            } else {
                "write-chars"
            };
            let output = Command::new("zellij")
                .args([
                    "--session",
                    session_id,
                    "action",
                    action,
                    "--pane-id",
                    &pane_id,
                    text,
                ])
                .output()?;
            if !output.status.success() {
                return Err(command_error("zellij", action, session_id, &output.stderr));
            }
        }
        self.send_enter_presses(session_id, if text.contains('\n') { 2 } else { 1 })
    }

    fn send_enter_presses(&self, session_id: &str, count: u32) -> Result<()> {
        let pane_id = self.primary_pane_id(session_id)?;
        for _ in 0..count.max(1) {
            let output = Command::new("zellij")
                .args([
                    "--session",
                    session_id,
                    "action",
                    "send-keys",
                    "--pane-id",
                    &pane_id,
                    "Enter",
                ])
                .output()?;
            if !output.status.success() {
                return Err(command_error(
                    "zellij",
                    "send-keys",
                    session_id,
                    &output.stderr,
                ));
            }
        }
        Ok(())
    }

    fn set_status_bar(&self, _session_id: &str, _text: &str) -> Result<()> {
        Ok(())
    }
}

fn build_launch_script(exec_cmd: &str, env_vars: &HashMap<String, String>) -> String {
    let mut lines = Vec::new();
    for key in blocked_inherited_env_vars() {
        lines.push(format!("unset {key}"));
    }
    for (key, value) in env_vars {
        if should_strip_inherited_env_var(key) || !is_valid_env_key(key) {
            continue;
        }
        lines.push(format!("export {}={}", key, shell_escape_value(value)));
    }
    lines.push(exec_cmd.to_string());
    lines.join("\n")
}

fn parse_primary_pane_id(stdout: &[u8]) -> Option<String> {
    let panes: serde_json::Value = serde_json::from_slice(stdout).ok()?;
    panes.as_array()?.iter().find_map(|pane| {
        if pane
            .get("is_plugin")
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
        {
            return None;
        }
        if pane
            .get("exited")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            return None;
        }
        if pane
            .get("is_selectable")
            .and_then(|v| v.as_bool())
            .unwrap_or(true)
        {
            let id = pane.get("id")?.as_i64()?;
            return Some(format!("terminal_{id}"));
        }
        None
    })
}

fn tail_lines(output: &str, lines: u32) -> String {
    let requested = lines as usize;
    if requested == 0 {
        return String::new();
    }
    let mut selected: Vec<&str> = output.lines().rev().take(requested).collect();
    selected.reverse();
    let mut out = selected.join("\n");
    if output.ends_with('\n') && !out.is_empty() {
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{build_launch_script, parse_primary_pane_id, tail_lines};
    use std::collections::HashMap;

    #[test]
    fn parses_selectable_terminal_pane_id() {
        let json = br#"
        [
          {"id": 1, "is_plugin": true, "exited": false, "is_selectable": true},
          {"id": 2, "is_plugin": false, "exited": true, "is_selectable": true},
          {"id": 3, "is_plugin": false, "exited": false, "is_selectable": true}
        ]
        "#;
        assert_eq!(parse_primary_pane_id(json).as_deref(), Some("terminal_3"));
    }

    #[test]
    fn launch_script_exports_env_and_strips_blocked_vars() {
        let mut env = HashMap::new();
        env.insert("FOO".to_string(), "bar baz".to_string());
        env.insert("CLAUDECODE".to_string(), "1".to_string());
        env.insert("BAD-NAME".to_string(), "ignored".to_string());

        let script = build_launch_script("codex --help", &env);
        assert!(script.contains("unset CLAUDECODE"));
        assert!(script.contains("export FOO='bar baz'"));
        assert!(!script.contains("export CLAUDECODE"));
        assert!(!script.contains("BAD-NAME"));
        assert!(script.ends_with("codex --help"));
    }

    #[test]
    fn tail_lines_returns_requested_suffix() {
        assert_eq!(tail_lines("a\nb\nc\nd\n", 2), "c\nd\n");
    }
}
