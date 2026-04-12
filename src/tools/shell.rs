//! shell tool — execute arbitrary shell commands with a timeout.
//!
//! Stdout/stderr are redirected to temp files (not pipes) so we
//! never deadlock when the child produces more than 64KB of output.
//! After the child exits or times out, we read the temp files with
//! a hard byte cap to prevent OOM on chatty commands.

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
/// Hard upper bound — the model cannot set timeout_secs beyond this.
const MAX_TIMEOUT_SECS: u64 = 600;
/// Maximum bytes we'll read from stdout or stderr. Anything beyond
/// this is discarded (never loaded into memory).
const MAX_OUTPUT_BYTES: u64 = 100 * 1024; // 100KB per stream

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
                "Execute a shell command in the working directory. Default timeout is {DEFAULT_TIMEOUT_SECS}s \
                 (configurable via timeout_secs, max {MAX_TIMEOUT_SECS}s). Captures stdout and stderr. \
                 Returns exit code and captured output."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute (passed to `/bin/sh -c`)"
                    },
                    "timeout_secs": {
                        "type": "integer",
                        "description": format!("Override the default timeout in seconds (max {MAX_TIMEOUT_SECS})")
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

        // Clamp timeout — model cannot exceed MAX_TIMEOUT_SECS.
        let timeout_secs = args
            .timeout_secs
            .unwrap_or(self.default_timeout_secs)
            .min(MAX_TIMEOUT_SECS);
        let timeout = Duration::from_secs(timeout_secs);

        // Redirect stdout/stderr to temp files so the child never
        // blocks on a pipe write. This eliminates the deadlock that
        // occurs with piped stdout/stderr when output exceeds the
        // OS pipe buffer (~64KB).
        let stdout_file = tempfile::tempfile().map_err(|e| TuttiError::ToolExecution {
            name: "shell".into(),
            reason: format!("failed to create stdout temp file: {e}"),
        })?;
        let stderr_file = tempfile::tempfile().map_err(|e| TuttiError::ToolExecution {
            name: "shell".into(),
            reason: format!("failed to create stderr temp file: {e}"),
        })?;

        // Use /bin/sh explicitly to avoid picking up a malicious sh
        // earlier on PATH.
        let mut command = Command::new("/bin/sh");
        command
            .arg("-c")
            .arg(&args.command)
            .current_dir(workdir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout_file.try_clone().map_err(|e| {
                TuttiError::ToolExecution {
                    name: "shell".into(),
                    reason: format!("failed to clone stdout fd: {e}"),
                }
            })?))
            .stderr(Stdio::from(stderr_file.try_clone().map_err(|e| {
                TuttiError::ToolExecution {
                    name: "shell".into(),
                    reason: format!("failed to clone stderr fd: {e}"),
                }
            })?));

        // On Unix, put the shell and its descendants in a new process
        // group so we can signal the whole tree on timeout.
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
                #[cfg(unix)]
                kill_process_group(child.id());
                let _ = child.kill();
                let _ = child.wait();
                return Ok(ToolOutput::error(format!(
                    "command timed out after {timeout_secs}s: {}",
                    args.command
                )));
            }
        };

        // Read temp files with a hard byte cap. `take()` means we
        // never allocate more than MAX_OUTPUT_BYTES + 1 regardless
        // of how much output the command produced.
        let stdout = read_capped(stdout_file);
        let stderr = read_capped(stderr_file);

        let exit_code = status.code().unwrap_or(-1);
        let output = format_output(exit_code, &stdout, &stderr);

        Ok(ToolOutput {
            content: output,
            files_modified: Vec::new(),
            is_error: exit_code != 0,
        })
    }
}

#[cfg(unix)]
fn kill_process_group(pid: u32) {
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
}

/// Read up to MAX_OUTPUT_BYTES from a file, seeking to the start first.
/// Never allocates more than MAX_OUTPUT_BYTES + 1.
fn read_capped(mut file: std::fs::File) -> String {
    use std::io::{Read, Seek, SeekFrom};
    if file.seek(SeekFrom::Start(0)).is_err() {
        return String::new();
    }
    let file_len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut buf = Vec::with_capacity((file_len.min(MAX_OUTPUT_BYTES + 1)) as usize);
    let _ = file.take(MAX_OUTPUT_BYTES + 1).read_to_end(&mut buf);
    let truncated = buf.len() as u64 > MAX_OUTPUT_BYTES;
    if truncated {
        buf.truncate(MAX_OUTPUT_BYTES as usize);
    }
    let mut s = String::from_utf8_lossy(&buf).into_owned();
    if truncated {
        s.push_str(&format!(
            "\n...[truncated, showing first {} bytes]",
            MAX_OUTPUT_BYTES
        ));
    }
    s
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

    #[test]
    fn timeout_clamped_to_max() {
        // Model passes u64::MAX — should be clamped to MAX_TIMEOUT_SECS.
        // We can't easily test the actual timeout, but we verify the
        // command still works (doesn't panic or overflow).
        let out = run(json!({"command": "echo ok", "timeout_secs": 999999})).unwrap();
        assert!(!out.is_error);
        assert!(out.content.contains("ok"));
    }

    #[test]
    fn large_output_does_not_deadlock() {
        // Generate >64KB of output. With piped stdout this would
        // deadlock because wait_timeout blocks before draining.
        // With temp-file redirection it completes normally.
        let out = run(json!({
            "command": "dd if=/dev/zero bs=1024 count=128 2>/dev/null | base64",
            "timeout_secs": 10
        }))
        .unwrap();
        assert!(!out.is_error, "should not deadlock: {}", out.content);
        assert!(out.content.contains("exit_code: 0"));
    }

    #[test]
    fn output_capped_at_max_bytes() {
        // Generate ~200KB of output, verify we get the truncation notice.
        let out = run(json!({
            "command": "dd if=/dev/zero bs=1024 count=200 2>/dev/null | base64",
            "timeout_secs": 10
        }))
        .unwrap();
        assert!(
            out.content.contains("truncated"),
            "expected truncation notice for large output"
        );
    }
}
