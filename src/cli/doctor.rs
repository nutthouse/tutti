use crate::config::{GlobalConfig, ToolPackConfig, TuttiConfig};
use crate::error::{Result, TuttiError};
use crate::runtime;
use colored::Colorize;
use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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

#[derive(Debug, Clone)]
struct DoctorCheck {
    check: String,
    status: DoctorStatus,
    detail: String,
}

pub fn run() -> Result<()> {
    let cwd = std::env::current_dir()?;
    let (config, _) = TuttiConfig::load(&cwd)?;
    config.validate()?;
    let global = GlobalConfig::load()?;

    let checks = evaluate_checks(
        &config,
        &global,
        &|command| which::which(command).is_ok(),
        &|key| std::env::var_os(key).is_some(),
    );

    print_checks(&checks);

    let failures = checks
        .iter()
        .filter(|check| check.status == DoctorStatus::Fail)
        .count();
    if failures > 0 {
        return Err(TuttiError::ConfigValidation(format!(
            "doctor found {failures} failing checks"
        )));
    }

    Ok(())
}

fn evaluate_checks(
    config: &TuttiConfig,
    global: &GlobalConfig,
    command_exists: &dyn Fn(&str) -> bool,
    env_exists: &dyn Fn(&str) -> bool,
) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();

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

    for agent in &config.agents {
        let Some(runtime_name) = agent.resolved_runtime(&config.defaults) else {
            checks.push(DoctorCheck {
                check: format!("runtime/{}", agent.name),
                status: DoctorStatus::Fail,
                detail: "runtime not set on agent or defaults".to_string(),
            });
            continue;
        };

        let command_override = workspace_profile.map(|profile| profile.command.as_str());
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

fn print_checks(checks: &[DoctorCheck]) {
    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["Check", "Status", "Detail"]);

    for check in checks {
        table.add_row(vec![
            check.check.clone(),
            check.status.label(),
            check.detail.clone(),
        ]);
    }

    println!("{table}");

    let total = checks.len();
    let fail = checks
        .iter()
        .filter(|check| check.status == DoctorStatus::Fail)
        .count();
    let warn = checks
        .iter()
        .filter(|check| check.status == DoctorStatus::Warn)
        .count();
    let pass = total.saturating_sub(fail + warn);

    println!("Summary: {} pass, {} warn, {} fail", pass, warn, fail);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        AgentConfig, DefaultsConfig, ProfileConfig, WorkspaceAuth, WorkspaceConfig,
    };
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
}
