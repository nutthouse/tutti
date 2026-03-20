use crate::config::{BudgetMode, GlobalConfig, TuttiConfig};
use crate::error::{Result, TuttiError};
use crate::state::{ControlEvent, append_control_event};
use crate::usage;
use chrono::Utc;
use serde_json::json;
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct BudgetGuardOutcome {
    pub warnings: Vec<String>,
}

pub fn enforce_pre_exec(
    config: &TuttiConfig,
    project_root: &Path,
    action: &str,
    agent_scope: Option<&str>,
) -> Result<BudgetGuardOutcome> {
    let Some(budget) = config.budget.as_ref() else {
        return Ok(BudgetGuardOutcome::default());
    };

    let global = GlobalConfig::load()?;
    let profile_name = config
        .workspace
        .auth
        .as_ref()
        .and_then(|a| a.default_profile.as_deref());
    let profile = profile_name.and_then(|name| global.get_profile(name));
    let Some(profile) = profile else {
        return Ok(BudgetGuardOutcome::default());
    };

    // Usage-based budget checks are only reliable for API plans.
    if !profile
        .plan
        .as_deref()
        .is_some_and(|p| p.trim().eq_ignore_ascii_case("api"))
    {
        return Ok(BudgetGuardOutcome::default());
    }

    let since = usage::compute_reset_start(profile.reset_day.as_deref());
    let workspace_usage = usage::scan_workspace_usage(project_root, &config.workspace.name, since)?;
    let warn_threshold = budget.warn_threshold_pct;
    let mut outcome = BudgetGuardOutcome::default();

    if let Some(cap) = budget.workspace_weekly_tokens {
        let used = token_total(&workspace_usage.usage.total);
        evaluate_and_apply(
            &mut outcome,
            config,
            project_root,
            CapCheckContext {
                action,
                scope: "workspace",
                agent: None,
                used,
                cap,
                warn_threshold,
                mode: budget.mode,
            },
        )?;
    }

    for (agent, cap) in &budget.agent_weekly_tokens {
        if let Some(scope) = agent_scope
            && scope != agent
        {
            continue;
        }
        let used = workspace_usage
            .by_agent
            .get(agent)
            .map(|u| token_total(&u.total))
            .unwrap_or(0);
        evaluate_and_apply(
            &mut outcome,
            config,
            project_root,
            CapCheckContext {
                action,
                scope: "agent",
                agent: Some(agent),
                used,
                cap: *cap,
                warn_threshold,
                mode: budget.mode,
            },
        )?;
    }

    Ok(outcome)
}

struct CapCheckContext<'a> {
    action: &'a str,
    scope: &'a str,
    agent: Option<&'a str>,
    used: u64,
    cap: u64,
    warn_threshold: f64,
    mode: BudgetMode,
}

fn evaluate_and_apply(
    outcome: &mut BudgetGuardOutcome,
    config: &TuttiConfig,
    project_root: &Path,
    ctx: CapCheckContext<'_>,
) -> Result<()> {
    let pct = (ctx.used as f64 / ctx.cap as f64) * 100.0;

    if pct >= ctx.warn_threshold {
        let message = format!(
            "{} budget at ~{:.0}% ({}/{}) tokens before {}",
            ctx.scope, pct, ctx.used, ctx.cap, ctx.action
        );
        outcome.warnings.push(message.clone());
        emit_budget_event(
            project_root,
            "budget.threshold",
            config,
            ctx.action,
            ctx.scope,
            ctx.agent,
            ctx.used,
            ctx.cap,
            pct,
            ctx.mode,
            false,
        );
    }

    if ctx.used < ctx.cap {
        return Ok(());
    }

    if ctx.mode == BudgetMode::Warn {
        let message = format!(
            "{} budget exceeded ({}/{}) tokens; continuing because budget.mode=warn",
            ctx.scope, ctx.used, ctx.cap
        );
        outcome.warnings.push(message.clone());
        emit_budget_event(
            project_root,
            "budget.threshold",
            config,
            ctx.action,
            ctx.scope,
            ctx.agent,
            ctx.used,
            ctx.cap,
            pct,
            ctx.mode,
            true,
        );
        return Ok(());
    }

    let reason = format!(
        "{} budget exceeded ({}/{}) tokens; blocked {}",
        ctx.scope, ctx.used, ctx.cap, ctx.action
    );
    emit_budget_event(
        project_root,
        "budget.blocked",
        config,
        ctx.action,
        ctx.scope,
        ctx.agent,
        ctx.used,
        ctx.cap,
        pct,
        ctx.mode,
        true,
    );
    Err(TuttiError::ConfigValidation(reason))
}

#[allow(clippy::too_many_arguments)]
fn emit_budget_event(
    project_root: &Path,
    event: &str,
    config: &TuttiConfig,
    action: &str,
    scope: &str,
    agent: Option<&str>,
    used: u64,
    cap: u64,
    pct: f64,
    mode: BudgetMode,
    over_cap: bool,
) {
    let _ = append_control_event(
        project_root,
        &ControlEvent {
            event: event.to_string(),
            workspace: config.workspace.name.clone(),
            agent: agent.map(ToString::to_string),
            timestamp: Utc::now(),
            correlation_id: format!("budget-{}-{}", Utc::now().timestamp_millis(), action),
            data: Some(json!({
                "action": action,
                "scope": scope,
                "used_tokens": used,
                "cap_tokens": cap,
                "pct": pct,
                "mode": match mode {
                    BudgetMode::Warn => "warn",
                    BudgetMode::Enforce => "enforce",
                },
                "over_cap": over_cap
            })),
        },
    );
}

fn token_total(usage: &crate::usage::TokenUsage) -> u64 {
    usage.total_input() + usage.output_tokens
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AgentConfig, BudgetConfig, DefaultsConfig, WorkspaceAuth, WorkspaceConfig,
    };
    use std::collections::HashMap;

    fn sample_config(mode: BudgetMode) -> TuttiConfig {
        let mut caps = HashMap::new();
        caps.insert("backend".to_string(), 100);
        TuttiConfig {
            workspace: WorkspaceConfig {
                name: "ws".to_string(),
                description: None,
                env: None,
                auth: Some(WorkspaceAuth {
                    default_profile: Some("api".to_string()),
                }),
            },
            defaults: DefaultsConfig {
                worktree: true,
                runtime: Some("claude-code".to_string()),
            },
            launch: None,
            agents: vec![AgentConfig {
                name: "backend".to_string(),
                runtime: Some("claude-code".to_string()),
                scope: None,
                prompt: None,
                depends_on: vec![],
                worktree: None,
                fresh_worktree: None,
                branch: None,
                persistent: false,
                memory: None,
                env: HashMap::new(),
            }],
            tool_packs: vec![],
            workflows: vec![],
            hooks: vec![],
            handoff: None,
            observe: None,
            budget: Some(BudgetConfig {
                mode,
                warn_threshold_pct: 80.0,
                workspace_weekly_tokens: Some(200),
                agent_weekly_tokens: caps,
            }),
            webhooks: vec![],
        }
    }

    #[test]
    fn token_total_counts_input_cache_and_output() {
        let usage = crate::usage::TokenUsage {
            input_tokens: 10,
            output_tokens: 7,
            cache_creation_input_tokens: 5,
            cache_read_input_tokens: 3,
        };
        assert_eq!(token_total(&usage), 25);
    }

    #[test]
    fn warn_mode_over_cap_returns_warning_not_error() {
        let config = sample_config(BudgetMode::Warn);
        let mut out = BudgetGuardOutcome::default();
        let result = evaluate_and_apply(
            &mut out,
            &config,
            Path::new("/tmp"),
            CapCheckContext {
                action: "send",
                scope: "agent",
                agent: Some("backend"),
                used: 120,
                cap: 100,
                warn_threshold: 80.0,
                mode: BudgetMode::Warn,
            },
        );
        assert!(result.is_ok());
        assert!(!out.warnings.is_empty());
    }

    #[test]
    fn enforce_mode_over_cap_errors() {
        let config = sample_config(BudgetMode::Enforce);
        let mut out = BudgetGuardOutcome::default();
        let result = evaluate_and_apply(
            &mut out,
            &config,
            Path::new("/tmp"),
            CapCheckContext {
                action: "send",
                scope: "agent",
                agent: Some("backend"),
                used: 120,
                cap: 100,
                warn_threshold: 80.0,
                mode: BudgetMode::Enforce,
            },
        );
        assert!(result.is_err());
    }
}
