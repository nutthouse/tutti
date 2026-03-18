use crate::automation::{ExecuteOptions, ExecutionOrigin, ResolvedStep, WorkflowResolver};
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
        PermissionsSubcommand::Suggest {
            workflow,
            apply,
            json,
        } => run_suggest(&workflow, apply, json),
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
    suggested_rule: Option<String>,
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
        decision.suggested_rule.as_deref(),
        decision.reason.as_deref(),
    );

    if as_json {
        let report = PermissionCheckReport {
            command: decision.command.clone(),
            allowed: decision.allowed,
            policy_configured: decision.policy_configured,
            matched_rule: decision.matched_rule.clone(),
            suggested_rule: decision.suggested_rule.clone(),
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
        if let Some(rule) = decision.suggested_rule.as_deref() {
            eprintln!("hint: add this allow rule: {rule}");
        }
    }

    if decision.allowed {
        Ok(())
    } else {
        let mut message = format!(
            "command blocked by permissions policy: '{}'",
            decision.command
        );
        if let Some(rule) = decision.suggested_rule.as_deref() {
            message.push_str(&format!(" (hint: add allow rule '{rule}')"));
        }
        Err(TuttiError::ConfigValidation(message))
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
    suggested_rule: Option<&str>,
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
                "matched_rule": matched_rule,
                "suggested_rule": suggested_rule
            })),
        },
    );
}

#[derive(Debug, Serialize)]
struct PermissionSuggestion {
    command: String,
    suggested_rule: String,
    reason: Option<String>,
}

#[derive(Debug, Serialize)]
struct PermissionSuggestReport {
    workflow: String,
    total_commands: usize,
    blocked: Vec<PermissionSuggestion>,
    applied_rules: Vec<String>,
}

#[allow(clippy::too_many_arguments)]
fn collect_blocked_commands(
    resolver: &WorkflowResolver,
    workflow: &str,
    options: &ExecuteOptions,
    active_workflows: &mut std::collections::BTreeSet<String>,
    seen_commands: &mut std::collections::BTreeSet<String>,
    blocked: &mut Vec<PermissionSuggestion>,
    global: &GlobalConfig,
    total_commands: &mut usize,
) -> Result<()> {
    // Cycle detection only for the current recursion stack.
    if active_workflows.contains(workflow) {
        return Err(TuttiError::ConfigValidation(format!(
            "cyclic workflow reference detected at '{workflow}'; remove or fix the cycle before running `tt permissions suggest`"
        )));
    }
    active_workflows.insert(workflow.to_string());

    let result = (|| -> Result<()> {
        let resolved = resolver.resolve(workflow, None, options)?;
        for step in resolved.steps {
            match step {
                ResolvedStep::Command { run, .. } => {
                    *total_commands += 1;
                    let cmd = normalize(run);
                    if cmd.is_empty() {
                        continue;
                    }
                    let decision = evaluate_command_policy(global.permissions.as_ref(), &cmd);
                    if !decision.allowed && seen_commands.insert(cmd.clone()) {
                        blocked.push(PermissionSuggestion {
                            command: cmd.clone(),
                            suggested_rule: decision.suggested_rule.unwrap_or_else(|| cmd.clone()),
                            reason: decision.reason,
                        });
                    }
                }
                ResolvedStep::Workflow { workflow, .. } => {
                    collect_blocked_commands(
                        resolver,
                        &workflow,
                        options,
                        active_workflows,
                        seen_commands,
                        blocked,
                        global,
                        total_commands,
                    )?;
                }
                _ => {}
            }
        }
        Ok(())
    })();

    active_workflows.remove(workflow);
    result
}

fn suggest_workflow_permissions(
    workflow: &str,
    apply: bool,
    config: &TuttiConfig,
    project_root: &std::path::Path,
    global: &mut GlobalConfig,
) -> Result<PermissionSuggestReport> {
    let options = ExecuteOptions {
        strict: false,
        force_open_commands: false,
        command_policy: global.permissions.clone(),
        retry_policy: None,
        origin: ExecutionOrigin::Run,
        hook_event: None,
        hook_agent: None,
    };

    let resolver = WorkflowResolver::new(config, project_root);
    let mut blocked: Vec<PermissionSuggestion> = Vec::new();
    let mut seen_commands = std::collections::BTreeSet::new();
    let mut active_workflows = std::collections::BTreeSet::new();
    let mut total_commands = 0usize;

    collect_blocked_commands(
        &resolver,
        workflow,
        &options,
        &mut active_workflows,
        &mut seen_commands,
        &mut blocked,
        global,
        &mut total_commands,
    )?;

    let mut applied_rules = Vec::new();
    if apply && !blocked.is_empty() {
        let policy = global
            .permissions
            .get_or_insert_with(PermissionsConfig::default);
        for item in &blocked {
            if !policy
                .allow
                .iter()
                .any(|existing| existing == &item.suggested_rule)
            {
                policy.allow.push(item.suggested_rule.clone());
                applied_rules.push(item.suggested_rule.clone());
            }
        }
        if !applied_rules.is_empty() {
            global.save()?;
        }
    }

    Ok(PermissionSuggestReport {
        workflow: workflow.to_string(),
        total_commands,
        blocked,
        applied_rules,
    })
}

fn run_suggest(workflow: &str, apply: bool, as_json: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (config, config_path) = TuttiConfig::load(&cwd)?;
    config.validate()?;
    let project_root = config_path.parent().ok_or_else(|| {
        TuttiError::ConfigValidation(
            "could not determine workspace root; run `tt permissions suggest` from a workspace containing tutti.toml"
                .to_string(),
        )
    })?;

    let mut global = GlobalConfig::load()?;
    let report = suggest_workflow_permissions(workflow, apply, &config, project_root, &mut global)?;

    if as_json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    if report.blocked.is_empty() {
        println!("No blocked workflow commands detected for '{}'.", workflow);
    } else {
        println!("The following commands should be added to [permissions].allow:");
        for item in &report.blocked {
            println!("  {}", item.suggested_rule);
        }
    }

    if apply {
        if report.applied_rules.is_empty() {
            println!("No new rules were applied.");
        } else {
            println!(
                "Applied {} rule(s) to {}",
                report.applied_rules.len(),
                global_config_path().display()
            );
        }
    }

    Ok(())
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
    use serial_test::serial;

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
            None,
        );

        let records = load_policy_decisions(&dir).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].action, "permission_check");
        assert_eq!(records[0].decision, "allow");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn suggest_reports_blocked_commands_including_nested_workflow_and_deduplicates() {
        let temp = std::env::temp_dir().join(format!(
            "tutti-test-permissions-suggest-nested-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let config_text = r#"
[workspace]
name = "ws"

[[workflow]]
name = "child"

[[workflow.step]]
type = "command"
run = "echo nested"

[[workflow]]
name = "root"

[[workflow.step]]
type = "command"
run = "echo top"

[[workflow.step]]
type = "command"
run = "echo top"

[[workflow.step]]
type = "workflow"
workflow = "child"
"#;
        std::fs::write(temp.join("tutti.toml"), config_text).unwrap();

        let (config, config_path) = TuttiConfig::load(&temp).unwrap();
        config.validate().unwrap();
        let project_root = config_path.parent().unwrap();
        let mut global = GlobalConfig {
            permissions: Some(PermissionsConfig {
                allow: vec!["echo top".to_string()],
            }),
            ..Default::default()
        };

        let report =
            suggest_workflow_permissions("root", false, &config, project_root, &mut global)
                .expect("suggest should work");

        assert_eq!(report.total_commands, 3);
        assert_eq!(report.blocked.len(), 1);
        assert_eq!(report.blocked[0].command, "echo nested");
        assert_eq!(report.blocked[0].suggested_rule, "echo nested");

        let _ = std::fs::remove_dir_all(&temp);
    }

    #[test]
    #[serial]
    fn suggest_apply_writes_global_permissions_and_json_shape_is_stable() {
        struct HomeGuard(Option<String>);

        impl Drop for HomeGuard {
            fn drop(&mut self) {
                if let Some(value) = self.0.take() {
                    unsafe {
                        std::env::set_var("HOME", value);
                    }
                } else {
                    unsafe {
                        std::env::remove_var("HOME");
                    }
                }
            }
        }

        let temp = std::env::temp_dir().join(format!(
            "tutti-test-permissions-suggest-apply-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&temp);
        std::fs::create_dir_all(&temp).unwrap();

        let _home_guard = HomeGuard(std::env::var("HOME").ok());
        unsafe {
            std::env::set_var("HOME", &temp);
        }

        let config_text = r#"
[workspace]
name = "ws"

[[workflow]]
name = "root"

[[workflow.step]]
type = "command"
run = "echo blocked"
"#;
        std::fs::write(temp.join("tutti.toml"), config_text).unwrap();

        let (config, config_path) = TuttiConfig::load(&temp).unwrap();
        config.validate().unwrap();
        let project_root = config_path.parent().unwrap();
        let mut global = GlobalConfig {
            permissions: Some(PermissionsConfig::default()),
            ..Default::default()
        };

        let report = suggest_workflow_permissions("root", true, &config, project_root, &mut global)
            .expect("suggest should work");

        assert_eq!(report.workflow, "root");
        assert_eq!(report.total_commands, 1);
        assert_eq!(report.blocked.len(), 1);
        assert_eq!(report.applied_rules, vec!["echo blocked".to_string()]);

        let report_json = serde_json::to_value(&report).unwrap();
        assert_eq!(report_json["workflow"], "root");
        assert_eq!(report_json["total_commands"], 1);
        assert!(report_json["blocked"].is_array());
        assert!(report_json["applied_rules"].is_array());

        let saved = std::fs::read_to_string(global_config_path()).unwrap();
        assert!(saved.contains("echo blocked"));

        let _ = std::fs::remove_dir_all(&temp);
    }
}
