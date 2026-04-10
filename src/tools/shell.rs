//! shell tool — execute arbitrary shell commands with a timeout.

use super::{Tool, ToolDefinition, ToolOutput};
use crate::error::{Result, TuttiError};
use serde::Deserialize;
use serde_json::json;
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;
use wait_timeout::ChildExt;

const DEFAULT_TIMEOUT_SECS: u64 = 60;
const MAX_OUTPUT_BYTES: usize = 100 * 1024; // 100KB of captured output per stream

pub struct ShellTool {
    default_timeout_secs: u64,
}

impl Default for ShellTool {
    fn default() -> Self {
        Self {
            default_timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }
}

#[derive(Debug, Deserialize)]
struct ShellArgs {
    command: String,
    #[serde(default)]
    timeout_secs: Option<u64>,
}

impl Tool for ShellTool {
    fn definition(&self) -> ToolDefinition {
        ToolDefinition {
            name: "shell".into(),
            description: format!(
                "Execute a shell command in the working directory. Default timeout is {DEFAULT_TIMEOUT_SECS}s (configurable via timeout_secs). Captures stdout and stderr. Returns exit code and captured output."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute (passed to `sh -c`)"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": "Override the default timeout in seconds"
                    }
                },
                "required": ["command"]
            }),
        }
    }

    fn execute(&self, args: &serde_json::Value, workdir: &Path) -> Result<ToolOutput> {
        let args: ShellArgs =
            serde_json::from_value(args.clone()).map_err(|e| TuttiError::ToolExecution {
                name: "shell".into(),
                reason: format!("invalid arguments: {e}"),
            })?;

        let timeout = Duration::from_secs(args.timeout_secs.unwrap_or(self.default_timeout_secs));

        let mut command = Command::new("sh");
        command
            .arg("-c")
            .arg(&args.command)
            .current_dir(workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        // On Unix, put the shell and its descendants in a new process
        // group so we can signal the whole tree on timeout. Without
        // this, `sh -c "sleep 10"` leaks the grandchild `sleep`
        // process when we kill `sh` — it gets reparented to PID 1
        // and keeps running.
        #[cfg(unix)]
        command.process_group(0);

        let mut child = command.spawn().map_err(|e| TuttiError::ToolExecution {
            name: "shell".into(),
            reason: format!("failed to spawn shell: {e}"),
        })?;

        let status = match child
            .wait_timeout(timeout)
            .map_err(|e| TuttiError::ToolExecution {
                name: "shell".into(),
                reason: format!("wait_timeout failed: {e}"),
            })? {
            Some(status) => status,
            None => {
                // Timed out. Kill the entire process group on Unix so
                // grandchildren don't get reparented to PID 1 and keep
                // running. On non-Unix, fall back to killing the direct
                // child.
                #[cfg(unix)]
                kill_process_group(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return Ok(ToolOutput::error(format!(
                    "command timed out after {}s: {}",
                    timeout.as_secs(),
                    args.command
                )));
            }
        };

        let stdout = child.stdout.take().map(read_capped).unwrap_or_default();
        let stderr = child.stderr.take().map(read_capped).unwrap_or_default();

        let exit_code = status.code().unwrap_or(-1);
        let output = format_output(exit_code, &stdout, &stderr);

        Ok(ToolOutput {
            content: output,
            files_modified: Vec::new(),
            // A non-zero exit is surfaced as is_error so the model can
            // see the command failed, even though tutti itself didn't
            // fail to execute it.
            is_error: exit_code != 0,
        })
    }
}

/// Send SIGKILL to a Unix process group. Used on timeout so that any
/// grandchildren spawned by `sh -c ...` are killed along with the
/// shell itself. Silently no-ops on non-Unix platforms.
#[cfg(unix)]
fn kill_process_group(pid: u32) {
    // SAFETY: libc::killpg is safe to call with any pgid; if the group
    // doesn't exist, it returns -1 and sets errno (which we ignore).
    // We intentionally target the process group, not the process, so
    // grandchildren get the signal too.
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
}

fn read_capped<R: std::io::Read>(mut reader: R) -> String {
    let mut buf = Vec::with_capacity(4096);
    let _ = reader.read_to_end(&mut buf);
    if buf.len() > MAX_OUTPUT_BYTES {
        let truncated = &buf[..MAX_OUTPUT_BYTES];
        let mut s = String::from_utf8_lossy(truncated).into_owned();
        s.push_str(&format!(
            "\n...[truncated {} bytes]",
            buf.len() - MAX_OUTPUT_BYTES
        ));
        s
    } else {
        String::from_utf8_lossy(&buf).into_owned()
    }
}

fn format_output(exit_code: i32, stdout: &str, stderr: &str) -> String {
    let mut parts = Vec::new();
    parts.push(format!("exit_code: {exit_code}"));
    if !stdout.is_empty() {
        parts.push(format!("stdout:\n{}", stdout.trim_end()));
    }
    if !stderr.is_empty() {
        parts.push(format!("stderr:\n{}", stderr.trim_end()));
    }
    if stdout.is_empty() && stderr.is_empty() {
        parts.push("(no output)".to_string());
    }
    parts.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(args: serde_json::Value) -> Result<ToolOutput> {
        let tmp = tempfile::tempdir().unwrap();
        ShellTool::default().execute(&args, tmp.path())
    }

    #[test]
    fn successful_command_returns_zero_exit() {
        let out = run(json!({"command": "echo hello"})).unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("exit_code: 0"));
        assert!(out.content.contains("hello"));
    }

    #[test]
    fn failing_command_flags_error() {
        let out = run(json!({"command": "false"})).unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("exit_code: 1"));
    }

    #[test]
    fn captures_stderr() {
        let out = run(json!({"command": "echo oops 1>&2"})).unwrap();
        assert!(out.content.contains("stderr"));
        assert!(out.content.contains("oops"));
    }

    #[test]
    fn timeout_kills_runaway_command() {
        let out = run(json!({"command": "sleep 10", "timeout_secs": 1})).unwrap();
        assert!(out.is_error);
        assert!(out.content.contains("timed out"));
    }

    #[test]
    fn runs_in_workdir() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("marker.txt"), "x").unwrap();
        let out = ShellTool::default()
            .execute(&json!({"command": "ls marker.txt"}), tmp.path())
            .unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("marker.txt"));
    }

    #[test]
    fn invalid_args_fail() {
        let tmp = tempfile::tempdir().unwrap();
        let result = ShellTool::default().execute(&json!({}), tmp.path());
        assert!(result.is_err());
    }
}
