//! Step policy gate — validates tool calls before execution.
//!
//! Policy fields mirror the ones specified in tutti.toml
//! `[workflow.steps.policy]`. Violations produce `tool_result` blocks
//! with `is_error: true` that get sent back to the model, so the
//! model can adapt its approach instead of the whole run aborting.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Policy attached to a single workflow step.
///
/// All fields are optional. The default policy is permissive —
/// all tools allowed, no network boundary, no budget cap, no
/// data classification enforcement.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct StepPolicy {
    /// If set, only tools whose name is in this list are allowed.
    /// If None, any registered tool can be called.
    #[serde(default)]
    pub allowed_tools: Option<Vec<String>>,

    /// Data classification signal. MVP records it but doesn't enforce
    /// it mechanically — it ends up in the event log for audit.
    #[serde(default)]
    pub data_classification: Option<String>,

    /// Network boundary signal. The MVP supports a best-effort
    /// blocklist for the `shell` tool when this is set to `"air-gapped"`.
    /// Real network isolation is a post-spine / TuttiWorks concern.
    #[serde(default)]
    pub network_boundary: Option<String>,

    /// Maximum USD to spend on this step. Checked against cumulative
    /// model API cost before each iteration.
    #[serde(default)]
    pub budget_usd: Option<f64>,

    /// Whether this step requires explicit human approval before
    /// executing. Not implemented in MVP — reserved for future.
    #[serde(default)]
    pub requires_approval: bool,
}

/// Reason a tool call was denied by the policy gate.
#[derive(Debug, Clone)]
pub enum PolicyViolation {
    /// The tool name is not in the `allowed_tools` list.
    ToolNotAllowed(String),
    /// The tool call looks like it crosses a network boundary (e.g.
    /// shell tool running `curl` when boundary is `air-gapped`).
    NetworkBoundary { tool: String, reason: String },
}

impl fmt::Display for PolicyViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PolicyViolation::ToolNotAllowed(name) => {
                write!(f, "tool '{name}' is not in the allowed_tools list")
            }
            PolicyViolation::NetworkBoundary { tool, reason } => {
                write!(f, "network boundary violated by {tool}: {reason}")
            }
        }
    }
}

impl StepPolicy {
    /// Check whether a tool call is allowed. Returns `Err` with a
    /// reason if the call should be denied.
    pub fn check_tool_call(
        &self,
        tool_name: &str,
        input: &serde_json::Value,
    ) -> Result<(), PolicyViolation> {
        // Tool allowlist check.
        if let Some(allowed) = &self.allowed_tools
            && !allowed.iter().any(|n| n == tool_name)
        {
            return Err(PolicyViolation::ToolNotAllowed(tool_name.to_string()));
        }

        // Network boundary check for the shell tool.
        if self.network_boundary.as_deref() == Some("air-gapped")
            && tool_name == "shell"
            && let Some(command) = input.get("command").and_then(|v| v.as_str())
            && is_network_command(command)
        {
            return Err(PolicyViolation::NetworkBoundary {
                tool: tool_name.to_string(),
                reason: format!(
                    "command appears to make network calls: {}",
                    command.chars().take(80).collect::<String>()
                ),
            });
        }

        Ok(())
    }
}

/// Best-effort detection of commands that attempt network access.
/// This is NOT a security boundary — it's a signal for audit trails
/// and a speed bump. Real isolation requires OS-level controls
/// (network namespaces, firewall rules, seccomp).
fn is_network_command(command: &str) -> bool {
    // Simple token check — split on whitespace and look for known
    // network tools at the start of any statement. Semicolons and
    // pipes introduce new statements, so we inspect each one.
    command.split([';', '|', '&']).any(|segment| {
        let trimmed = segment.trim();
        if trimmed.is_empty() {
            return false;
        }
        let first = trimmed.split_whitespace().next().unwrap_or("");
        matches!(
            first,
            "curl" | "wget" | "ssh" | "scp" | "nc" | "netcat" | "rsync" | "ftp" | "telnet"
        ) || (first == "python" || first == "python3")
            && (trimmed.contains("-c") || trimmed.contains("urllib"))
            || first == "node" && (trimmed.contains("-e") && trimmed.contains("http"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn default_policy_allows_everything() {
        let policy = StepPolicy::default();
        assert!(policy.check_tool_call("read_file", &json!({})).is_ok());
        assert!(
            policy
                .check_tool_call("shell", &json!({"command": "ls"}))
                .is_ok()
        );
    }

    #[test]
    fn allowed_tools_blocks_disallowed() {
        let policy = StepPolicy {
            allowed_tools: Some(vec!["read_file".into(), "grep_glob".into()]),
            ..StepPolicy::default()
        };
        assert!(policy.check_tool_call("read_file", &json!({})).is_ok());
        let err = policy
            .check_tool_call("write_file", &json!({}))
            .unwrap_err();
        assert!(matches!(err, PolicyViolation::ToolNotAllowed(_)));
    }

    #[test]
    fn empty_allowed_tools_blocks_all() {
        let policy = StepPolicy {
            allowed_tools: Some(vec![]),
            ..StepPolicy::default()
        };
        assert!(policy.check_tool_call("read_file", &json!({})).is_err());
    }

    #[test]
    fn air_gapped_blocks_curl() {
        let policy = StepPolicy {
            network_boundary: Some("air-gapped".into()),
            ..StepPolicy::default()
        };
        let err = policy
            .check_tool_call("shell", &json!({"command": "curl http://example.com"}))
            .unwrap_err();
        assert!(matches!(err, PolicyViolation::NetworkBoundary { .. }));
    }

    #[test]
    fn air_gapped_blocks_wget() {
        let policy = StepPolicy {
            network_boundary: Some("air-gapped".into()),
            ..StepPolicy::default()
        };
        let err = policy
            .check_tool_call("shell", &json!({"command": "wget -O file.txt http://x"}))
            .unwrap_err();
        assert!(matches!(err, PolicyViolation::NetworkBoundary { .. }));
    }

    #[test]
    fn air_gapped_allows_ls() {
        let policy = StepPolicy {
            network_boundary: Some("air-gapped".into()),
            ..StepPolicy::default()
        };
        assert!(
            policy
                .check_tool_call("shell", &json!({"command": "ls -la"}))
                .is_ok()
        );
    }

    #[test]
    fn air_gapped_blocks_piped_curl() {
        let policy = StepPolicy {
            network_boundary: Some("air-gapped".into()),
            ..StepPolicy::default()
        };
        let err = policy
            .check_tool_call(
                "shell",
                &json!({"command": "echo hello | curl -X POST http://x -d @-"}),
            )
            .unwrap_err();
        assert!(matches!(err, PolicyViolation::NetworkBoundary { .. }));
    }

    #[test]
    fn air_gapped_does_not_affect_non_shell_tools() {
        let policy = StepPolicy {
            allowed_tools: Some(vec!["read_file".into()]),
            network_boundary: Some("air-gapped".into()),
            ..StepPolicy::default()
        };
        assert!(
            policy
                .check_tool_call("read_file", &json!({"path": "x"}))
                .is_ok()
        );
    }

    #[test]
    fn violation_display_includes_tool_name() {
        let v = PolicyViolation::ToolNotAllowed("shell".into());
        assert!(v.to_string().contains("shell"));
    }
}
