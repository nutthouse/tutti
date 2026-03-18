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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandPolicyDecision {
    pub command: String,
    pub allowed: bool,
    pub policy_configured: bool,
    pub matched_rule: Option<String>,
    pub suggested_rule: Option<String>,
    pub reason: Option<String>,
}

pub fn evaluate_command_policy(
    policy: Option<&PermissionsConfig>,
    command_line: &str,
) -> CommandPolicyDecision {
    let normalized = normalize(command_line);
    let Some(policy) = policy else {
        return CommandPolicyDecision {
            command: normalized,
            allowed: true,
            policy_configured: false,
            matched_rule: None,
            suggested_rule: None,
            reason: Some("policy not configured".to_string()),
        };
    };

    if let Some(matched_rule) = matching_allow_rule(policy, &normalized) {
        return CommandPolicyDecision {
            command: normalized,
            allowed: true,
            policy_configured: true,
            matched_rule: Some(matched_rule.to_string()),
            suggested_rule: None,
            reason: None,
        };
    }

    CommandPolicyDecision {
        command: normalized.clone(),
        allowed: false,
        policy_configured: true,
        matched_rule: None,
        suggested_rule: suggested_prefix_rule(&normalized),
        reason: Some("blocked by permissions policy".to_string()),
    }
}

fn suggested_prefix_rule(command_line: &str) -> Option<String> {
    let tokens: Vec<&str> = command_line.split_whitespace().collect();
    if tokens.len() < 2 {
        return None;
    }

    Some(format!("{} {}", tokens[0], tokens[1]))
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

pub fn shell_command_allow_rules(policy: &PermissionsConfig) -> Vec<String> {
    policy
        .allow
        .iter()
        .filter_map(|entry| extract_shell_rule(entry))
        .collect()
}

pub fn matching_allow_rule<'a>(
    policy: &'a PermissionsConfig,
    command_line: &str,
) -> Option<&'a str> {
    for raw_rule in &policy.allow {
        let trimmed = raw_rule.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(prefix) = trimmed.strip_suffix('*') {
            let prefix = normalize(prefix);
            if !prefix.is_empty() && command_line.starts_with(&prefix) {
                return Some(trimmed);
            }
            continue;
        }

        let normalized_rule = normalize(trimmed);
        if normalized_rule.is_empty() {
            continue;
        }
        if command_line == normalized_rule {
            return Some(trimmed);
        }
        let mut bounded = normalized_rule;
        bounded.push(' ');
        if command_line.starts_with(&bounded) {
            return Some(trimmed);
        }
    }

    None
}

fn claude_permission_entry(entry: &str) -> String {
    if is_claude_tool_permission(entry) {
        entry.to_string()
    } else {
        format!("Bash({entry})")
    }
}

fn extract_shell_rule(entry: &str) -> Option<String> {
    let normalized = normalize(entry);
    if normalized.is_empty() {
        return None;
    }

    if is_claude_tool_permission(&normalized) {
        if let Some(inner) = normalized
            .strip_prefix("Bash(")
            .and_then(|s| s.strip_suffix(')'))
        {
            let cmd = normalize(inner);
            return (!cmd.is_empty()).then_some(cmd);
        }
        return None;
    }

    Some(normalized)
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

    #[test]
    fn evaluate_command_policy_allows_when_policy_unset() {
        let decision = evaluate_command_policy(None, "git status");
        assert!(decision.allowed);
        assert!(!decision.policy_configured);
        assert_eq!(decision.reason.as_deref(), Some("policy not configured"));
    }

    #[test]
    fn evaluate_command_policy_blocks_when_rule_missing() {
        let policy = PermissionsConfig {
            allow: vec!["git status".to_string()],
        };
        let decision = evaluate_command_policy(Some(&policy), "git stash");
        assert!(!decision.allowed);
        assert!(decision.policy_configured);
        assert_eq!(
            decision.reason.as_deref(),
            Some("blocked by permissions policy")
        );
        assert_eq!(decision.suggested_rule.as_deref(), Some("git stash"));
    }

    #[test]
    fn evaluate_command_policy_suggests_rule_from_first_two_tokens_only() {
        let policy = PermissionsConfig {
            allow: vec!["git status".to_string()],
        };
        let decision = evaluate_command_policy(Some(&policy), "cargo test --quiet --all");
        assert_eq!(decision.suggested_rule.as_deref(), Some("cargo test"));
    }

    #[test]
    fn evaluate_command_policy_matches_prefix_rule() {
        let policy = PermissionsConfig {
            allow: vec!["cargo test".to_string()],
        };
        let decision = evaluate_command_policy(Some(&policy), "cargo test --quiet");
        assert!(decision.allowed);
        assert!(decision.policy_configured);
        assert_eq!(decision.matched_rule.as_deref(), Some("cargo test"));
    }

    #[test]
    fn shell_command_allow_rules_extracts_bash_and_plain_rules() {
        let policy = PermissionsConfig {
            allow: vec![
                "Read".to_string(),
                "Edit".to_string(),
                "Bash(git status)".to_string(),
                "cargo test *".to_string(),
                "   ".to_string(),
            ],
        };
        let rules = shell_command_allow_rules(&policy);
        assert_eq!(
            rules,
            vec!["git status".to_string(), "cargo test *".to_string()]
        );
    }
}
