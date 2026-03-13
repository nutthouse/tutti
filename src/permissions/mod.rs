use crate::config::{GlobalConfig, PermissionsConfig};
use crate::error::Result;
use serde_json::json;

const CLAUDE_TOOL_NAMES: &[&str] = &[
    "Bash",
    "Edit",
    "ExitPlanMode",
    "Glob",
    "Grep",
    "LS",
    "MultiEdit",
    "NotebookEdit",
    "NotebookRead",
    "Read",
    "Task",
    "TodoRead",
    "TodoWrite",
    "WebFetch",
    "WebSearch",
    "Write",
];

pub fn has_configured_policy(global: &GlobalConfig) -> bool {
    global
        .permissions
        .as_ref()
        .is_some_and(|policy| !policy.allow.iter().all(|entry| entry.trim().is_empty()))
}

pub fn render_claude_settings(policy: &PermissionsConfig) -> Result<String> {
    let allow: Vec<String> = policy
        .allow
        .iter()
        .map(normalize)
        .filter(|entry| !entry.is_empty())
        .map(|entry| claude_permission_entry(&entry))
        .collect();

    let payload = json!({
        "permissions": {
            "allow": allow
        }
    });

    Ok(serde_json::to_string_pretty(&payload)?)
}

pub fn normalize<S: AsRef<str>>(input: S) -> String {
    input
        .as_ref()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn claude_permission_entry(entry: &str) -> String {
    if is_claude_tool_permission(entry) {
        entry.to_string()
    } else {
        format!("Bash({entry})")
    }
}

fn is_claude_tool_permission(entry: &str) -> bool {
    if entry.contains('(') && entry.ends_with(')') {
        return true;
    }
    CLAUDE_TOOL_NAMES
        .iter()
        .any(|tool| entry.eq_ignore_ascii_case(tool))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GlobalConfig;

    #[test]
    fn has_configured_policy_requires_non_empty_entries() {
        let mut global = GlobalConfig::default();
        assert!(!has_configured_policy(&global));

        global.permissions = Some(PermissionsConfig {
            allow: vec!["   ".to_string()],
        });
        assert!(!has_configured_policy(&global));

        global.permissions = Some(PermissionsConfig {
            allow: vec!["git status".to_string()],
        });
        assert!(has_configured_policy(&global));
    }

    #[test]
    fn render_claude_settings_wraps_bash_permissions() {
        let policy = PermissionsConfig {
            allow: vec!["git status".to_string(), "cargo test --quiet".to_string()],
        };
        let rendered = render_claude_settings(&policy).expect("render should succeed");
        assert!(rendered.contains("Bash(git status)"));
        assert!(rendered.contains("Bash(cargo test --quiet)"));
    }

    #[test]
    fn render_claude_settings_preserves_tool_permissions() {
        let policy = PermissionsConfig {
            allow: vec![
                "Edit".to_string(),
                "Read".to_string(),
                "Bash(git diff)".to_string(),
                "git status".to_string(),
            ],
        };
        let rendered = render_claude_settings(&policy).expect("render should succeed");
        assert!(rendered.contains("\"Edit\""));
        assert!(rendered.contains("\"Read\""));
        assert!(rendered.contains("\"Bash(git diff)\""));
        assert!(rendered.contains("\"Bash(git status)\""));
        assert!(!rendered.contains("Bash(Edit)"));
        assert!(!rendered.contains("Bash(Read)"));
    }
}
