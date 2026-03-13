use crate::config::{GlobalConfig, LaunchMode, LaunchPolicyMode, ToolPackConfig, TuttiConfig};
use crate::error::{Result, TuttiError};
use crate::health;
use crate::permissions::has_configured_policy;
use crate::runtime;
use crate::state::{AgentHealth, AuthState};
use colored::Colorize;
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
enum DoctorStatus {
    Pass,
    Warn,
    Fail,
}

impl DoctorStatus {
    fn label(self) -> String {
        match self {
            DoctorStatus::Pass => "PASS".green().to_string(),
            DoctorStatus::Warn => "WARN".yellow().to_string(),
            DoctorStatus::Fail => "FAIL".red().to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
struct DoctorCheck {
    check: String,
    status: DoctorStatus,
    detail: String,
}

#[derive(Debug, Clone, Serialize)]
struct DoctorSummary {
    pass: usize,
    warn: usize,
    fail: usize,
}

#[derive(Debug, Clone, Serialize)]
struct DoctorReport {
    checks: Vec<DoctorCheck>,
    summary: DoctorSummary,
}

pub fn run(json: bool, strict: bool) -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (config, config_path) = TuttiConfig::load(&cwd)?;
    config.validate()?;
    let global = GlobalConfig::load()?;
    let project_root = config_path.parent().ok_or_else(|| {
        TuttiError::ConfigValidation("could not determine workspace root".to_string())
    })?;

    let mut checks = evaluate_checks(
        &config,
        &global,
        &|command| which::which(command).is_ok(),
        &|key| std::env::var_os(key).is_some(),
    );
    if let Ok(records) = health::probe_workspace(&config, project_root, 200) {
        checks.extend(auth_health_checks(&records));
    }

    let report = build_report(checks);
    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_report(&report);
    }

    doctor_exit_status(&report.summary, strict)
}

fn evaluate_checks(
    config: &TuttiConfig,
    global: &GlobalConfig,
    command_exists: &dyn Fn(&str) -> bool,
    env_exists: &dyn Fn(&str) -> bool,
) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    let launch_settings = resolve_launch_settings(config);

    checks.push(if command_exists("tmux") {
        DoctorCheck {
            check: "tmux".to_string(),
            status: DoctorStatus::Pass,
            detail: "tmux is available".to_string(),
        }
    } else {
        DoctorCheck {
            check: "tmux".to_string(),
            status: DoctorStatus::Fail,
            detail: "tmux not found on PATH".to_string(),
        }
    });

    let workspace_profile_name = config
        .workspace
        .auth
        .as_ref()
        .and_then(|auth| auth.default_profile.as_deref());
    let workspace_profile = workspace_profile_name.and_then(|name| global.get_profile(name));

    match workspace_profile_name {
        Some(name) => {
            if let Some(profile) = workspace_profile {
                checks.push(DoctorCheck {
                    check: "auth profile".to_string(),
                    status: DoctorStatus::Pass,
                    detail: format!(
                        "workspace profile '{}' ({})",
                        profile.name, profile.provider
                    ),
                });
                if command_exists(&profile.command) {
                    checks.push(DoctorCheck {
                        check: "profile command".to_string(),
                        status: DoctorStatus::Pass,
                        detail: format!("'{}' is available", profile.command),
                    });
                } else {
                    checks.push(DoctorCheck {
                        check: "profile command".to_string(),
                        status: DoctorStatus::Fail,
                        detail: format!("'{}' is not on PATH", profile.command),
                    });
                }
            } else {
                checks.push(DoctorCheck {
                    check: "auth profile".to_string(),
                    status: DoctorStatus::Fail,
                    detail: format!("workspace references missing profile '{}'", name),
                });
            }
        }
        None => checks.push(DoctorCheck {
            check: "auth profile".to_string(),
            status: DoctorStatus::Warn,
            detail: "workspace.auth.default_profile is not set".to_string(),
        }),
    }

    let launch_targets_supported_runtime = config.agents.iter().any(|agent| {
        agent
            .resolved_runtime(&config.defaults)
            .as_deref()
            .is_some_and(|rt| matches!(rt, "claude-code" | "codex" | "openclaw" | "aider"))
    });
    let policy_configured = has_configured_policy(global);
    if launch_requires_constrained_policy(launch_settings) && launch_targets_supported_runtime {
        if policy_configured {
            checks.push(DoctorCheck {
                check: "launch policy".to_string(),
                status: DoctorStatus::Pass,
                detail: "constrained non-interactive launch policy is configured".to_string(),
            });
        } else {
            checks.push(DoctorCheck {
                check: "launch policy".to_string(),
                status: DoctorStatus::Fail,
                detail: "launch mode requires [permissions] allow rules for constrained non-interactive runs".to_string(),
            });
        }
    }

    if launch_uses_bypass(launch_settings) && launch_targets_supported_runtime {
        checks.push(DoctorCheck {
            check: "launch policy".to_string(),
            status: DoctorStatus::Warn,
            detail: "bypass launch mode disables approval prompts for supported runtimes"
                .to_string(),
        });
    }

    if launch_requires_constrained_policy(launch_settings)
        && config.agents.iter().any(|agent| {
            agent
                .resolved_runtime(&config.defaults)
                .as_deref()
                .is_some_and(|rt| matches!(rt, "codex" | "openclaw" | "aider"))
        })
    {
        checks.push(DoctorCheck {
            check: "launch/best_effort".to_string(),
            status: DoctorStatus::Warn,
            detail: "codex/openclaw/aider constrained mode is best-effort; hard allowlist enforcement is currently Claude-only".to_string(),
        });
    }

    for agent in &config.agents {
        let Some(runtime_name) = agent.resolved_runtime(&config.defaults) else {
            checks.push(DoctorCheck {
                check: format!("runtime/{}", agent.name),
                status: DoctorStatus::Fail,
                detail: "runtime not set on agent or defaults".to_string(),
            });
            continue;
        };

        let command_override = runtime::compatible_command_override(
            &runtime_name,
            workspace_profile.map(|profile| profile.provider.as_str()),
            workspace_profile.map(|profile| profile.command.as_str()),
        );
        let Some(adapter) = runtime::get_adapter(&runtime_name, command_override) else {
            checks.push(DoctorCheck {
                check: format!("runtime/{}", agent.name),
                status: DoctorStatus::Fail,
                detail: format!("unknown runtime '{}'", runtime_name),
            });
            continue;
        };

        let command_name = adapter.command_name().to_string();
        if command_exists(&command_name) {
            checks.push(DoctorCheck {
                check: format!("runtime/{}", agent.name),
                status: DoctorStatus::Pass,
                detail: format!("{} via '{}'", runtime_name, command_name),
            });
        } else {
            checks.push(DoctorCheck {
                check: format!("runtime/{}", agent.name),
                status: DoctorStatus::Fail,
                detail: format!("{} command '{}' not found", runtime_name, command_name),
            });
        }
    }

    if config.tool_packs.is_empty() {
        checks.push(DoctorCheck {
            check: "tool packs".to_string(),
            status: DoctorStatus::Warn,
            detail: "no [[tool_pack]] entries configured".to_string(),
        });
    } else {
        checks.extend(check_tool_packs(
            &config.tool_packs,
            command_exists,
            env_exists,
        ));
    }

    checks
}

fn resolve_launch_settings(config: &TuttiConfig) -> (LaunchMode, LaunchPolicyMode) {
    let mode = config
        .launch
        .as_ref()
        .map(|launch| launch.mode)
        .unwrap_or(LaunchMode::Auto);
    let policy = config
        .launch
        .as_ref()
        .map(|launch| launch.policy)
        .unwrap_or(LaunchPolicyMode::Constrained);
    (mode, policy)
}

fn launch_requires_constrained_policy(settings: (LaunchMode, LaunchPolicyMode)) -> bool {
    let (mode, policy) = settings;
    if mode == LaunchMode::Safe {
        return false;
    }
    match mode {
        LaunchMode::Auto => true,
        LaunchMode::Unattended => policy == LaunchPolicyMode::Constrained,
        LaunchMode::Safe => false,
    }
}

fn launch_uses_bypass(settings: (LaunchMode, LaunchPolicyMode)) -> bool {
    let (mode, policy) = settings;
    mode == LaunchMode::Unattended && policy == LaunchPolicyMode::Bypass
}

fn check_tool_packs(
    packs: &[ToolPackConfig],
    command_exists: &dyn Fn(&str) -> bool,
    env_exists: &dyn Fn(&str) -> bool,
) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();

    for pack in packs {
        if pack.required_commands.is_empty() && pack.required_env.is_empty() {
            checks.push(DoctorCheck {
                check: format!("tool-pack/{}", pack.name),
                status: DoctorStatus::Warn,
                detail: "no prerequisites declared".to_string(),
            });
        }

        for command in &pack.required_commands {
            let command = command.trim();
            let status = if command_exists(command) {
                DoctorStatus::Pass
            } else {
                DoctorStatus::Fail
            };
            checks.push(DoctorCheck {
                check: format!("tool-pack/{}/cmd", pack.name),
                status,
                detail: format!("command '{}'", command),
            });
        }

        for key in &pack.required_env {
            let key = key.trim();
            let status = if env_exists(key) {
                DoctorStatus::Pass
            } else {
                DoctorStatus::Fail
            };
            checks.push(DoctorCheck {
                check: format!("tool-pack/{}/env", pack.name),
                status,
                detail: format!("env '{}'", key),
            });
        }
    }

    checks
}

fn auth_health_checks(records: &[AgentHealth]) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    for record in records.iter().filter(|r| r.running) {
        let (status, detail) = match record.auth_state {
            AuthState::Ok => (DoctorStatus::Pass, "auth healthy".to_string()),
            AuthState::Failed => (
                DoctorStatus::Fail,
                format!(
                    "auth failed{}",
                    record
                        .reason
                        .as_deref()
                        .map(|r| format!(": {r}"))
                        .unwrap_or_default()
                ),
            ),
            AuthState::Unknown => (DoctorStatus::Warn, "auth state unknown".to_string()),
        };
        checks.push(DoctorCheck {
            check: format!("auth/{}", record.agent),
            status,
            detail,
        });
    }
    checks
}

fn build_report(checks: Vec<DoctorCheck>) -> DoctorReport {
    let fail = checks
        .iter()
        .filter(|check| check.status == DoctorStatus::Fail)
        .count();
    let warn = checks
        .iter()
        .filter(|check| check.status == DoctorStatus::Warn)
        .count();
    let pass = checks.len().saturating_sub(fail + warn);
    DoctorReport {
        checks,
        summary: DoctorSummary { pass, warn, fail },
    }
}

fn print_report(report: &DoctorReport) {
    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["Check", "Status", "Detail", "Suggested Fix"]);

    for check in &report.checks {
        let suggestion = suggestion_for_check(check).unwrap_or("—").to_string();
        table.add_row(vec![
            check.check.clone(),
            check.status.label(),
            check.detail.clone(),
            suggestion,
        ]);
    }

    println!("{table}");

    println!(
        "Summary: {} pass, {} warn, {} fail",
        report.summary.pass, report.summary.warn, report.summary.fail
    );
}

fn suggestion_for_check(check: &DoctorCheck) -> Option<&'static str> {
    match check.check.as_str() {
        "tmux" if check.status == DoctorStatus::Fail => Some("Install tmux and re-run doctor"),
        "auth profile" if check.status == DoctorStatus::Fail => {
            Some("Set workspace.auth.default_profile to an existing profile")
        }
        "auth profile" if check.status == DoctorStatus::Warn => {
            Some("Configure workspace.auth.default_profile in tutti.toml")
        }
        "profile command" if check.status == DoctorStatus::Fail => {
            Some("Install the profile command or update profile.command in global config")
        }
        "launch policy" if check.status == DoctorStatus::Fail => Some(
            "Add [permissions].allow in global config, or launch with --mode safe / --policy bypass",
        ),
        "launch policy" if check.status == DoctorStatus::Warn => {
            Some("Prefer constrained mode for safer unattended runs")
        }
        "launch/best_effort" => {
            Some("Use claude-code for hard allowlist enforcement in constrained mode")
        }
        check_name if check_name.starts_with("auth/") && check.status == DoctorStatus::Fail => {
            Some("Re-authenticate runtime account and re-run doctor")
        }
        check_name if check_name.starts_with("auth/") && check.status == DoctorStatus::Warn => {
            Some("Inspect session output (tt peek) and verify runtime auth state")
        }
        check_name if check_name.starts_with("runtime/") && check.status == DoctorStatus::Fail => {
            Some("Install runtime CLI or adjust runtime/profile command mapping")
        }
        check_name
            if check_name.starts_with("tool-pack/") && check.status == DoctorStatus::Fail =>
        {
            Some("Install the missing command/env required by this tool pack")
        }
        check_name
            if check_name.starts_with("tool-pack/") && check.status == DoctorStatus::Warn =>
        {
            Some("Declare required_commands/required_env to make this check meaningful")
        }
        _ => None,
    }
}

fn doctor_exit_status(summary: &DoctorSummary, strict: bool) -> Result<()> {
    if summary.fail > 0 {
        return Err(TuttiError::ConfigValidation(format!(
            "doctor found {} failing checks",
            summary.fail
        )));
    }
    if strict && summary.warn > 0 {
        return Err(TuttiError::ConfigValidation(format!(
            "doctor strict mode failed with {} warning checks",
            summary.warn
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AgentConfig, DefaultsConfig, ProfileConfig, WorkspaceAuth, WorkspaceConfig,
    };
    use crate::state::ActivityState;
    use chrono::Utc;
    use std::collections::HashMap;

    fn sample_config(default_profile: Option<&str>) -> TuttiConfig {
        TuttiConfig {
            workspace: WorkspaceConfig {
                name: "ws".to_string(),
                description: None,
                env: None,
                auth: Some(WorkspaceAuth {
                    default_profile: default_profile.map(|s| s.to_string()),
                }),
            },
            defaults: DefaultsConfig {
                worktree: true,
                runtime: Some("claude-code".to_string()),
            },
            launch: None,
            agents: vec![AgentConfig {
                name: "backend".to_string(),
                runtime: None,
                scope: None,
                prompt: None,
                depends_on: vec![],
                worktree: None,
                branch: None,
                persistent: false,
                env: HashMap::new(),
            }],
            tool_packs: vec![],
            workflows: vec![],
            hooks: vec![],
            handoff: None,
            observe: None,
            budget: None,
        }
    }

    fn sample_global(profile_name: &str, command: &str) -> GlobalConfig {
        GlobalConfig {
            user: None,
            profiles: vec![ProfileConfig {
                name: profile_name.to_string(),
                provider: "anthropic".to_string(),
                command: command.to_string(),
                max_concurrent: None,
                monthly_budget: None,
                priority: None,
                plan: None,
                reset_day: None,
                weekly_hours: None,
            }],
            registered_workspaces: vec![],
            dashboard: None,
            resilience: None,
            permissions: None,
        }
    }

    #[test]
    fn warns_when_default_profile_is_missing() {
        let config = sample_config(None);
        let global = GlobalConfig::default();
        let checks = evaluate_checks(&config, &global, &|_| true, &|_| true);

        assert!(
            checks
                .iter()
                .any(|c| c.check == "auth profile" && c.status == DoctorStatus::Warn)
        );
    }

    #[test]
    fn fails_when_workspace_profile_is_unknown() {
        let config = sample_config(Some("missing"));
        let global = GlobalConfig::default();
        let checks = evaluate_checks(&config, &global, &|_| true, &|_| true);

        assert!(checks.iter().any(|c| {
            c.check == "auth profile"
                && c.status == DoctorStatus::Fail
                && c.detail.contains("missing profile")
        }));
    }

    #[test]
    fn uses_profile_command_override_for_runtime_availability() {
        let config = sample_config(Some("work"));
        let global = sample_global("work", "claude-work");
        let checks = evaluate_checks(
            &config,
            &global,
            &|cmd| cmd == "tmux" || cmd == "claude-work",
            &|_| true,
        );

        assert!(checks.iter().any(|c| {
            c.check == "runtime/backend"
                && c.status == DoctorStatus::Pass
                && c.detail.contains("claude-work")
        }));
    }

    #[test]
    fn ignores_mismatched_profile_command_override_for_runtime() {
        let mut config = sample_config(Some("work"));
        config.defaults.runtime = Some("codex".to_string());
        let global = sample_global("work", "claude-work");
        let checks = evaluate_checks(
            &config,
            &global,
            &|cmd| cmd == "tmux" || cmd == "codex",
            &|_| true,
        );

        assert!(checks.iter().any(|c| {
            c.check == "runtime/backend"
                && c.status == DoctorStatus::Pass
                && c.detail.contains("codex")
        }));
        assert!(!checks.iter().any(|c| {
            c.check == "runtime/backend"
                && c.status == DoctorStatus::Pass
                && c.detail.contains("claude-work")
        }));
    }

    #[test]
    fn tool_pack_checks_fail_for_missing_command_and_env() {
        let mut config = sample_config(None);
        config.tool_packs = vec![ToolPackConfig {
            name: "analytics".to_string(),
            description: None,
            required_commands: vec!["bq".to_string()],
            required_env: vec!["GCP_PROJECT".to_string()],
        }];

        let checks = evaluate_checks(
            &config,
            &GlobalConfig::default(),
            &|cmd| cmd == "tmux",
            &|_| false,
        );

        assert!(checks.iter().any(|c| {
            c.check == "tool-pack/analytics/cmd"
                && c.status == DoctorStatus::Fail
                && c.detail.contains("bq")
        }));
        assert!(checks.iter().any(|c| {
            c.check == "tool-pack/analytics/env"
                && c.status == DoctorStatus::Fail
                && c.detail.contains("GCP_PROJECT")
        }));
    }

    #[test]
    fn build_report_counts_status_totals() {
        let report = build_report(vec![
            DoctorCheck {
                check: "a".to_string(),
                status: DoctorStatus::Pass,
                detail: "ok".to_string(),
            },
            DoctorCheck {
                check: "b".to_string(),
                status: DoctorStatus::Warn,
                detail: "warn".to_string(),
            },
            DoctorCheck {
                check: "c".to_string(),
                status: DoctorStatus::Fail,
                detail: "fail".to_string(),
            },
        ]);
        assert_eq!(report.summary.pass, 1);
        assert_eq!(report.summary.warn, 1);
        assert_eq!(report.summary.fail, 1);
    }

    #[test]
    fn fails_when_launch_requires_policy_but_permissions_missing() {
        let config = sample_config(None);
        let global = GlobalConfig::default();
        let checks = evaluate_checks(&config, &global, &|_| true, &|_| true);

        assert!(checks.iter().any(|c| {
            c.check == "launch policy"
                && c.status == DoctorStatus::Fail
                && c.detail.contains("[permissions]")
        }));
    }

    #[test]
    fn warns_when_bypass_launch_mode_selected() {
        let mut config = sample_config(None);
        config.launch = Some(crate::config::LaunchConfig {
            mode: crate::config::LaunchMode::Unattended,
            policy: crate::config::LaunchPolicyMode::Bypass,
        });
        let checks = evaluate_checks(
            &config,
            &GlobalConfig::default(),
            &|cmd| cmd == "tmux" || cmd == "claude",
            &|_| true,
        );

        assert!(checks.iter().any(|c| {
            c.check == "launch policy"
                && c.status == DoctorStatus::Warn
                && c.detail.contains("bypass")
        }));
    }

    #[test]
    fn doctor_exit_status_fails_on_warnings_in_strict_mode() {
        let summary = DoctorSummary {
            pass: 3,
            warn: 1,
            fail: 0,
        };
        assert!(doctor_exit_status(&summary, true).is_err());
    }

    #[test]
    fn doctor_exit_status_allows_warnings_when_not_strict() {
        let summary = DoctorSummary {
            pass: 3,
            warn: 1,
            fail: 0,
        };
        assert!(doctor_exit_status(&summary, false).is_ok());
    }

    #[test]
    fn suggestion_for_launch_policy_failure_is_actionable() {
        let check = DoctorCheck {
            check: "launch policy".to_string(),
            status: DoctorStatus::Fail,
            detail: "missing policy".to_string(),
        };
        let suggestion = suggestion_for_check(&check).unwrap();
        assert!(suggestion.contains("[permissions]"));
    }

    #[test]
    fn auth_health_checks_map_auth_states() {
        let records = vec![
            AgentHealth {
                workspace: "ws".to_string(),
                agent: "backend".to_string(),
                runtime: "claude-code".to_string(),
                session_name: "tutti-ws-backend".to_string(),
                running: true,
                activity_state: ActivityState::Working,
                auth_state: AuthState::Ok,
                last_output_change_at: None,
                last_probe_at: Utc::now(),
                reason: None,
                pane_hash: None,
            },
            AgentHealth {
                workspace: "ws".to_string(),
                agent: "frontend".to_string(),
                runtime: "codex".to_string(),
                session_name: "tutti-ws-frontend".to_string(),
                running: true,
                activity_state: ActivityState::Idle,
                auth_state: AuthState::Failed,
                last_output_change_at: None,
                last_probe_at: Utc::now(),
                reason: Some("token expired".to_string()),
                pane_hash: None,
            },
        ];

        let checks = auth_health_checks(&records);
        assert!(
            checks
                .iter()
                .any(|c| c.check == "auth/backend" && c.status == DoctorStatus::Pass)
        );
        assert!(
            checks
                .iter()
                .any(|c| c.check == "auth/frontend" && c.status == DoctorStatus::Fail)
        );
    }
}
