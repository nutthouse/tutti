use crate::cli::PermissionsSubcommand;
use crate::config::{GlobalConfig, PermissionsConfig, TuttiConfig, global_config_path};
use crate::error::{Result, TuttiError};
use crate::permissions::{evaluate_command_policy, normalize, render_claude_settings};
use crate::state::{PolicyDecisionRecord, append_policy_decision};
use chrono::Utc;
use serde::Serialize;
use serde_json::json;
use std::path::Path;

pub fn run(command: PermissionsSubcommand) -> Result<()> {
    match command {
        PermissionsSubcommand::Check { command, json } => run_check(&command, json),
        PermissionsSubcommand::Export { runtime, output } => {
            run_export(&runtime, output.as_deref())
        }
    }
}

#[derive(Debug, Serialize)]
struct PermissionCheckReport {
    command: String,
    allowed: bool,
    policy_configured: bool,
    matched_rule: Option<String>,
    reason: Option<String>,
}

fn run_check(parts: &[String], as_json: bool) -> Result<()> {
    let command_line = normalize(parts.join(" "));
    if command_line.is_empty() {
        return Err(TuttiError::ConfigValidation(
            "command cannot be empty".to_string(),
        ));
    }

    let global = GlobalConfig::load()?;
    let workspace_ctx = resolve_workspace_context();
    let decision = evaluate_command_policy(global.permissions.as_ref(), &command_line);
    persist_permission_check_decision(
        workspace_ctx.as_ref(),
        &decision.command,
        decision.allowed,
        decision.policy_configured,
        decision.matched_rule.as_deref(),
        decision.reason.as_deref(),
    );

    if as_json {
        let report = PermissionCheckReport {
            command: decision.command.clone(),
            allowed: decision.allowed,
            policy_configured: decision.policy_configured,
            matched_rule: decision.matched_rule.clone(),
            reason: decision.reason.clone(),
        };
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if !decision.policy_configured {
        println!(
            "Permissions policy is not configured in {}; allowing command.",
            global_config_path().display()
        );
        println!("allowed: {}", decision.command);
    } else if decision.allowed {
        println!("allowed: {}", decision.command);
        if let Some(rule) = decision.matched_rule.as_deref() {
            println!("matched rule: {rule}");
        }
    } else if let Some(reason) = decision.reason.as_deref() {
        eprintln!("{reason}: '{}'", decision.command);
    }

    if decision.allowed {
        Ok(())
    } else {
        Err(TuttiError::ConfigValidation(format!(
            "command blocked by permissions policy: '{}'",
            decision.command
        )))
    }
}

#[derive(Debug, Clone)]
struct WorkspaceContext {
    workspace: String,
    project_root: std::path::PathBuf,
}

fn resolve_workspace_context() -> Option<WorkspaceContext> {
    let cwd = std::env::current_dir().ok()?;
    let (config, config_path) = TuttiConfig::load(&cwd).ok()?;
    let project_root = config_path.parent()?.to_path_buf();
    Some(WorkspaceContext {
        workspace: config.workspace.name,
        project_root,
    })
}

fn persist_permission_check_decision(
    workspace_ctx: Option<&WorkspaceContext>,
    command: &str,
    allowed: bool,
    policy_configured: bool,
    matched_rule: Option<&str>,
    reason: Option<&str>,
) {
    let Some(ctx) = workspace_ctx else {
        return;
    };

    let _ = append_policy_decision(
        &ctx.project_root,
        &PolicyDecisionRecord {
            timestamp: Utc::now(),
            workspace: ctx.workspace.clone(),
            agent: None,
            runtime: None,
            action: "permission_check".to_string(),
            mode: "n/a".to_string(),
            policy: if policy_configured {
                "configured".to_string()
            } else {
                "unset".to_string()
            },
            enforcement: "hard".to_string(),
            decision: if allowed {
                "allow".to_string()
            } else {
                "block".to_string()
            },
            reason: reason.map(ToString::to_string),
            data: Some(json!({
                "command": command,
                "matched_rule": matched_rule
            })),
        },
    );
}

fn run_export(runtime: &str, output: Option<&Path>) -> Result<()> {
    let global = GlobalConfig::load()?;
    let policy = global.permissions.unwrap_or_default();
    let rendered = render_export(runtime, &policy)?;

    if let Some(path) = output {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let payload = format!("{rendered}\n");
        std::fs::write(path, payload)?;
        println!("wrote {}", path.display());
    } else {
        println!("{rendered}");
    }

    Ok(())
}

fn render_export(runtime: &str, policy: &PermissionsConfig) -> Result<String> {
    match normalize(runtime).to_ascii_lowercase().as_str() {
        "claude" | "claude-code" => render_claude_settings(policy),
        other => Err(TuttiError::ConfigValidation(format!(
            "unsupported runtime '{}'; supported: claude",
            other
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::load_policy_decisions;

    #[test]
    fn matching_rule_supports_exact_and_prefix() {
        let policy = PermissionsConfig {
            allow: vec!["git status".to_string(), "cargo test".to_string()],
        };

        assert_eq!(
            crate::permissions::matching_allow_rule(&policy, "git status"),
            Some("git status")
        );
        assert_eq!(
            crate::permissions::matching_allow_rule(&policy, "cargo test --quiet"),
            Some("cargo test")
        );
        assert_eq!(
            crate::permissions::matching_allow_rule(&policy, "git stash"),
            None
        );
    }

    #[test]
    fn matching_rule_supports_wildcard_prefix() {
        let policy = PermissionsConfig {
            allow: vec!["npm run *".to_string()],
        };

        assert_eq!(
            crate::permissions::matching_allow_rule(&policy, "npm run build"),
            Some("npm run *")
        );
        assert_eq!(
            crate::permissions::matching_allow_rule(&policy, "npm test"),
            None
        );
    }

    #[test]
    fn normalize_compacts_whitespace() {
        assert_eq!(normalize("  cargo   test   --all "), "cargo test --all");
    }

    #[test]
    fn render_claude_settings_wraps_allow_entries_as_bash_permissions() {
        let policy = PermissionsConfig {
            allow: vec!["git status".to_string(), "cargo test --quiet".to_string()],
        };
        let rendered = render_claude_settings(&policy).expect("render should succeed");
        assert!(rendered.contains("Bash(git status)"));
        assert!(rendered.contains("Bash(cargo test --quiet)"));
    }

    #[test]
    fn render_export_rejects_unknown_runtime() {
        let policy = PermissionsConfig::default();
        let err = render_export("codex", &policy).expect_err("should fail");
        assert!(err.to_string().contains("unsupported runtime"));
    }

    #[test]
    fn persist_permission_check_decision_writes_policy_log() {
        let dir = std::env::temp_dir().join(format!(
            "tutti-test-permissions-policy-log-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".tutti/state")).unwrap();

        let ctx = WorkspaceContext {
            workspace: "ws".to_string(),
            project_root: dir.clone(),
        };
        persist_permission_check_decision(
            Some(&ctx),
            "git status",
            true,
            true,
            Some("git status"),
            None,
        );

        let records = load_policy_decisions(&dir).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].action, "permission_check");
        assert_eq!(records[0].decision, "allow");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
