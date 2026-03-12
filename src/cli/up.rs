use crate::config::{GlobalConfig, TuttiConfig, topological_sort};
use crate::error::{Result, TuttiError};
use crate::runtime;
use crate::session::TmuxSession;
use crate::state;
use crate::worktree;
use chrono::Utc;
use colored::Colorize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone)]
struct ProfileLimit {
    profile_name: String,
    max_concurrent: u32,
}

pub fn run(agent_filter: Option<&str>, workspace_name: Option<&str>, all: bool) -> Result<()> {
    crate::session::tmux::check_tmux()?;

    if all {
        return run_all();
    }

    let (config, config_path) = if let Some(ws) = workspace_name {
        load_workspace_by_name(ws)?
    } else {
        let cwd = std::env::current_dir()?;
        TuttiConfig::load(&cwd)?
    };
    config.validate()?;

    let project_root = config_path.parent().unwrap();
    state::ensure_tutti_dir(project_root)?;

    // Load global config once for profile resolution and capacity check
    let global = GlobalConfig::load().ok();
    let profile_limit = global
        .as_ref()
        .and_then(|g| resolve_profile_limit(&config, g));
    let mut active_for_profile = match (global.as_ref(), profile_limit.as_ref()) {
        (Some(g), Some(limit)) => {
            count_active_for_profile(g, &limit.profile_name, Some((&config, project_root)))
        }
        _ => 0,
    };

    // Resolve profile command override
    let command_override = resolve_profile_command(&config, global.as_ref());

    // Build workspace-level env vars
    let workspace_env = build_workspace_env(&config);

    let agents: Vec<_> = if let Some(name) = agent_filter {
        let agent = config
            .agents
            .iter()
            .find(|a| a.name == name)
            .ok_or_else(|| TuttiError::AgentNotFound(name.to_string()))?;

        // Warn if dependencies aren't running
        for dep in &agent.depends_on {
            let dep_session = TmuxSession::session_name(&config.workspace.name, dep);
            if !TmuxSession::session_exists(&dep_session) {
                eprintln!("  {} dependency '{}' is not running", "warn".yellow(), dep);
            }
        }

        vec![agent]
    } else {
        // Use topological sort for dependency ordering
        topological_sort(&config.agents)?
    };

    let mut launched = Vec::new();
    let mut refused_by_limit = false;

    for agent in &agents {
        let runtime_name = agent.resolved_runtime(&config.defaults).ok_or_else(|| {
            TuttiError::ConfigValidation(format!(
                "agent '{}' has no runtime (set runtime on agent or in [defaults])",
                agent.name
            ))
        })?;

        let adapter = runtime::get_adapter(&runtime_name, command_override.as_deref())
            .ok_or_else(|| TuttiError::RuntimeUnknown(runtime_name.clone()))?;

        if !adapter.is_available() {
            return Err(TuttiError::RuntimeNotAvailable(runtime_name.clone()));
        }

        let session = TmuxSession::session_name(&config.workspace.name, &agent.name);

        if TmuxSession::session_exists(&session) {
            println!("  {} {} (already running)", "skip".yellow(), agent.name);
            continue;
        }

        if let Some(limit) = &profile_limit
            && active_for_profile >= limit.max_concurrent
        {
            refused_by_limit = true;
            eprintln!(
                "  {} profile '{}' is at max_concurrent ({}/{}) — refusing to launch {}",
                "warn".yellow(),
                limit.profile_name,
                active_for_profile,
                limit.max_concurrent,
                agent.name
            );
            continue;
        }

        // Set up worktree if enabled
        let (working_dir, worktree_path, branch) = if agent.resolved_worktree(&config.defaults) {
            let branch = agent.resolved_branch();
            match worktree::ensure_worktree(project_root, &agent.name, &branch) {
                Ok(wt_path) => {
                    let dir = wt_path.to_str().unwrap().to_string();
                    (dir, Some(wt_path), Some(branch))
                }
                Err(e) => {
                    eprintln!(
                        "  {} worktree for {}: {e} (using project root)",
                        "warn".yellow(),
                        agent.name
                    );
                    let dir = project_root.to_str().unwrap().to_string();
                    (dir, None, None)
                }
            }
        } else {
            let dir = project_root.to_str().unwrap().to_string();
            (dir, None, None)
        };

        // Merge workspace env with agent-level env (agent overrides workspace)
        let mut env = workspace_env.clone();
        for (k, v) in &agent.env {
            env.insert(k.clone(), v.clone());
        }

        let cmd = adapter.build_spawn_command(agent.prompt.as_deref());
        TmuxSession::create_session(&session, &working_dir, &cmd, &env)?;

        // Save state
        let agent_state = state::AgentState {
            name: agent.name.clone(),
            runtime: runtime_name.clone(),
            session_name: session.clone(),
            worktree_path,
            branch,
            status: "Working".to_string(),
            started_at: Utc::now(),
            stopped_at: None,
        };
        state::save_agent_state(project_root, &agent_state)?;

        launched.push((agent.name.clone(), session, runtime_name));
        if profile_limit.is_some() {
            active_for_profile += 1;
        }
    }

    if launched.is_empty() {
        if let Some(limit) = &profile_limit
            && refused_by_limit
        {
            return Err(TuttiError::ConfigValidation(format!(
                "profile '{}' reached max_concurrent ({}/{})",
                limit.profile_name, active_for_profile, limit.max_concurrent
            )));
        }
        println!("No agents to launch.");
        return Ok(());
    }

    // Print summary
    println!();
    println!("{}", "Launched agents:".bold());
    print_launch_summary(&launched);
    println!();

    // Best-effort capacity warning
    capacity_warning(&config, project_root, global.as_ref());

    println!(
        "Use {} to see status, {} to connect.",
        "tt status".cyan(),
        "tt attach <agent>".cyan()
    );

    Ok(())
}

/// Build environment variables from workspace config.
fn build_workspace_env(config: &TuttiConfig) -> HashMap<String, String> {
    let mut env = HashMap::new();

    if let Some(ws_env) = &config.workspace.env {
        if let Some(ref name) = ws_env.git_name {
            env.insert("GIT_AUTHOR_NAME".to_string(), name.clone());
            env.insert("GIT_COMMITTER_NAME".to_string(), name.clone());
        }
        if let Some(ref email) = ws_env.git_email {
            env.insert("GIT_AUTHOR_EMAIL".to_string(), email.clone());
            env.insert("GIT_COMMITTER_EMAIL".to_string(), email.clone());
        }
        for (k, v) in &ws_env.extra {
            env.insert(k.clone(), v.clone());
        }
    }

    env
}

/// Resolve the command override from the default profile, if set.
fn resolve_profile_command(config: &TuttiConfig, global: Option<&GlobalConfig>) -> Option<String> {
    let profile_name = config.workspace.auth.as_ref()?.default_profile.as_ref()?;
    let profile = global?.get_profile(profile_name)?;
    Some(profile.command.clone())
}

fn resolve_profile_limit(config: &TuttiConfig, global: &GlobalConfig) -> Option<ProfileLimit> {
    let profile_name = config.workspace.auth.as_ref()?.default_profile.as_ref()?;
    let profile = global.get_profile(profile_name)?;
    Some(ProfileLimit {
        profile_name: profile_name.clone(),
        max_concurrent: profile.max_concurrent?,
    })
}

fn workspace_default_profile(config: &TuttiConfig) -> Option<&str> {
    config.workspace.auth.as_ref()?.default_profile.as_deref()
}

fn count_active_for_profile(
    global: &GlobalConfig,
    profile_name: &str,
    extra_workspace: Option<(&TuttiConfig, &Path)>,
) -> u32 {
    let mut count = 0;
    let mut seen_roots = HashSet::new();

    for ws in &global.registered_workspaces {
        if let Ok((config, config_path)) = TuttiConfig::load(&ws.path)
            && workspace_default_profile(&config) == Some(profile_name)
        {
            let project_root = config_path.parent().unwrap();
            seen_roots.insert(project_root.to_path_buf());
            count += count_running_agents(&config);
        }
    }

    if let Some((config, project_root)) = extra_workspace {
        let root_buf = project_root.to_path_buf();
        if !seen_roots.contains(&root_buf)
            && workspace_default_profile(config) == Some(profile_name)
        {
            count += count_running_agents(config);
        }
    }

    count
}

fn count_running_agents(config: &TuttiConfig) -> u32 {
    config
        .agents
        .iter()
        .filter(|agent| {
            let session = TmuxSession::session_name(&config.workspace.name, &agent.name);
            TmuxSession::session_exists(&session)
        })
        .count() as u32
}

fn print_launch_summary(launched: &[(String, String, String)]) {
    use comfy_table::{Table, presets::UTF8_BORDERS_ONLY};

    let mut table = Table::new();
    table.load_preset(UTF8_BORDERS_ONLY);
    table.set_header(vec!["Agent", "Runtime", "Session"]);

    for (name, session, runtime) in launched {
        table.add_row(vec![name, runtime, session]);
    }

    println!("{table}");
}

/// Best-effort capacity warning after launch. Never blocks or errors.
fn capacity_warning(
    config: &crate::config::TuttiConfig,
    project_root: &Path,
    global: Option<&GlobalConfig>,
) {
    let profile_name = match config
        .workspace
        .auth
        .as_ref()
        .and_then(|a| a.default_profile.as_ref())
    {
        Some(name) => name,
        None => return,
    };

    let global = match global {
        Some(g) => g,
        None => return,
    };

    let profile = match global.get_profile(profile_name) {
        Some(p) => p,
        None => return,
    };

    // Usage/capacity tracking is API-only.
    if !is_api_usage_plan(profile.plan.as_deref()) {
        return;
    }

    if profile.weekly_hours.is_none() {
        return;
    }

    match crate::usage::quick_capacity_check(profile, project_root) {
        Ok(Some(pct)) if pct > 80.0 => {
            eprintln!(
                "  {} capacity at ~{:.0}% — run {} for details",
                "warn".yellow(),
                pct,
                "tt usage".cyan()
            );
            eprintln!();
        }
        _ => {}
    }
}

fn is_api_usage_plan(plan: Option<&str>) -> bool {
    plan.is_some_and(|p| p.trim().eq_ignore_ascii_case("api"))
}

/// Check that git is available (needed for worktrees).
#[allow(dead_code)]
pub fn check_git(project_root: &Path) -> Result<()> {
    let output = std::process::Command::new("git")
        .args(["rev-parse", "--git-dir"])
        .current_dir(project_root)
        .output()?;

    if !output.status.success() {
        return Err(TuttiError::Git(
            "not a git repository (worktrees require git)".to_string(),
        ));
    }
    Ok(())
}

/// Load config for a named workspace from the global registry.
pub fn load_workspace_by_name(ws_name: &str) -> Result<(TuttiConfig, std::path::PathBuf)> {
    let global = crate::config::GlobalConfig::load()?;
    let ws = global
        .registered_workspaces
        .iter()
        .find(|w| w.name == ws_name)
        .ok_or_else(|| {
            TuttiError::ConfigValidation(format!(
                "workspace '{ws_name}' not found in global config. Run `tt init` in that project first."
            ))
        })?;
    TuttiConfig::load(&ws.path)
}

/// Launch all agents in all registered workspaces.
fn run_all() -> Result<()> {
    let global = crate::config::GlobalConfig::load()?;
    if global.registered_workspaces.is_empty() {
        println!("No registered workspaces. Run `tt init` in your projects first.");
        return Ok(());
    }

    let mut active_by_profile = HashMap::<String, u32>::new();
    for ws in &global.registered_workspaces {
        if let Ok((config, _)) = TuttiConfig::load(&ws.path)
            && let Some(profile_name) = workspace_default_profile(&config)
        {
            let running = count_running_agents(&config);
            *active_by_profile
                .entry(profile_name.to_string())
                .or_insert(0) += running;
        }
    }

    for ws in &global.registered_workspaces {
        println!("Workspace: {}", ws.name);
        match TuttiConfig::load(&ws.path) {
            Ok((config, config_path)) => {
                if let Err(e) = config.validate() {
                    eprintln!("  Skipping {} (invalid config): {e}", ws.name);
                    continue;
                }
                let project_root = config_path.parent().unwrap();
                state::ensure_tutti_dir(project_root)?;

                let command_override = resolve_profile_command(&config, Some(&global));
                let profile_limit = resolve_profile_limit(&config, &global);
                let workspace_env = build_workspace_env(&config);

                // Use topological sort for dependency ordering
                let sorted = match topological_sort(&config.agents) {
                    Ok(s) => s,
                    Err(e) => {
                        eprintln!("  Skipping {} (dependency error): {e}", ws.name);
                        continue;
                    }
                };

                for agent in sorted {
                    let runtime_name = match agent.resolved_runtime(&config.defaults) {
                        Some(rt) => rt,
                        None => {
                            eprintln!("  Skipping {} (no runtime)", agent.name);
                            continue;
                        }
                    };
                    let adapter =
                        match runtime::get_adapter(&runtime_name, command_override.as_deref()) {
                            Some(a) => a,
                            None => {
                                eprintln!(
                                    "  Skipping {} (unknown runtime '{runtime_name}')",
                                    agent.name
                                );
                                continue;
                            }
                        };
                    if !adapter.is_available() {
                        eprintln!(
                            "  Skipping {} (runtime '{runtime_name}' not installed)",
                            agent.name
                        );
                        continue;
                    }

                    let session = TmuxSession::session_name(&config.workspace.name, &agent.name);
                    if TmuxSession::session_exists(&session) {
                        println!("  skip {} (already running)", agent.name);
                        continue;
                    }

                    if let Some(limit) = &profile_limit {
                        let active = *active_by_profile.get(&limit.profile_name).unwrap_or(&0);
                        if active >= limit.max_concurrent {
                            eprintln!(
                                "  Skipping {} (profile '{}' at max_concurrent {}/{})",
                                agent.name, limit.profile_name, active, limit.max_concurrent
                            );
                            continue;
                        }
                    }

                    let mut env = workspace_env.clone();
                    for (k, v) in &agent.env {
                        env.insert(k.clone(), v.clone());
                    }

                    let working_dir = project_root.to_str().unwrap().to_string();
                    let cmd = adapter.build_spawn_command(agent.prompt.as_deref());
                    if let Err(e) = TmuxSession::create_session(&session, &working_dir, &cmd, &env)
                    {
                        eprintln!("  Failed to launch {}: {e}", agent.name);
                        continue;
                    }

                    let agent_state = state::AgentState {
                        name: agent.name.clone(),
                        runtime: runtime_name,
                        session_name: session,
                        worktree_path: None,
                        branch: None,
                        status: "Working".to_string(),
                        started_at: Utc::now(),
                        stopped_at: None,
                    };
                    let _ = state::save_agent_state(project_root, &agent_state);
                    println!("  launched {}", agent.name);

                    if let Some(limit) = &profile_limit {
                        *active_by_profile
                            .entry(limit.profile_name.clone())
                            .or_insert(0) += 1;
                    }
                }
            }
            Err(e) => {
                eprintln!("  Skipping {} (config error): {e}", ws.name);
            }
        }
        println!();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{AgentConfig, DefaultsConfig, GlobalConfig, ProfileConfig, WorkspaceAuth};

    fn make_agent(name: &str, deps: Vec<&str>) -> AgentConfig {
        AgentConfig {
            name: name.to_string(),
            runtime: Some("claude-code".to_string()),
            scope: None,
            prompt: None,
            depends_on: deps.into_iter().map(|s| s.to_string()).collect(),
            worktree: None,
            branch: None,
            persistent: false,
            env: HashMap::new(),
        }
    }

    #[test]
    fn topo_sort_linear_chain() {
        let agents = vec![
            make_agent("c", vec!["b"]),
            make_agent("b", vec!["a"]),
            make_agent("a", vec![]),
        ];
        let sorted = topological_sort(&agents).unwrap();
        let names: Vec<&str> = sorted.iter().map(|a| a.name.as_str()).collect();
        // a must come before b, b before c
        let pos_a = names.iter().position(|&n| n == "a").unwrap();
        let pos_b = names.iter().position(|&n| n == "b").unwrap();
        let pos_c = names.iter().position(|&n| n == "c").unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_b < pos_c);
    }

    #[test]
    fn topo_sort_diamond() {
        // a -> b, a -> c, b -> d, c -> d
        let agents = vec![
            make_agent("d", vec!["b", "c"]),
            make_agent("b", vec!["a"]),
            make_agent("c", vec!["a"]),
            make_agent("a", vec![]),
        ];
        let sorted = topological_sort(&agents).unwrap();
        let names: Vec<&str> = sorted.iter().map(|a| a.name.as_str()).collect();
        let pos_a = names.iter().position(|&n| n == "a").unwrap();
        let pos_b = names.iter().position(|&n| n == "b").unwrap();
        let pos_c = names.iter().position(|&n| n == "c").unwrap();
        let pos_d = names.iter().position(|&n| n == "d").unwrap();
        assert!(pos_a < pos_b);
        assert!(pos_a < pos_c);
        assert!(pos_b < pos_d);
        assert!(pos_c < pos_d);
    }

    #[test]
    fn topo_sort_cycle_detected() {
        let agents = vec![make_agent("a", vec!["b"]), make_agent("b", vec!["a"])];
        let err = topological_sort(&agents).unwrap_err();
        assert!(err.to_string().contains("cycle"));
    }

    #[test]
    fn topo_sort_no_deps_passthrough() {
        let agents = vec![
            make_agent("a", vec![]),
            make_agent("b", vec![]),
            make_agent("c", vec![]),
        ];
        let sorted = topological_sort(&agents).unwrap();
        assert_eq!(sorted.len(), 3);
    }

    #[test]
    fn build_workspace_env_from_config() {
        let config = TuttiConfig {
            workspace: crate::config::WorkspaceConfig {
                name: "test".to_string(),
                description: None,
                env: Some(crate::config::WorkspaceEnv {
                    git_name: Some("Test User".to_string()),
                    git_email: Some("test@example.com".to_string()),
                    extra: HashMap::from([("CUSTOM_VAR".to_string(), "value".to_string())]),
                }),
                auth: None,
            },
            defaults: DefaultsConfig {
                worktree: true,
                runtime: Some("claude-code".to_string()),
            },
            agents: vec![],
            handoff: None,
            observe: None,
        };
        let env = build_workspace_env(&config);
        assert_eq!(env.get("GIT_AUTHOR_NAME").unwrap(), "Test User");
        assert_eq!(env.get("GIT_COMMITTER_NAME").unwrap(), "Test User");
        assert_eq!(env.get("GIT_AUTHOR_EMAIL").unwrap(), "test@example.com");
        assert_eq!(env.get("GIT_COMMITTER_EMAIL").unwrap(), "test@example.com");
        assert_eq!(env.get("CUSTOM_VAR").unwrap(), "value");
    }

    #[test]
    fn agent_env_overrides_workspace() {
        let config = TuttiConfig {
            workspace: crate::config::WorkspaceConfig {
                name: "test".to_string(),
                description: None,
                env: Some(crate::config::WorkspaceEnv {
                    git_name: Some("Workspace User".to_string()),
                    git_email: None,
                    extra: HashMap::new(),
                }),
                auth: None,
            },
            defaults: DefaultsConfig {
                worktree: true,
                runtime: Some("claude-code".to_string()),
            },
            agents: vec![],
            handoff: None,
            observe: None,
        };
        let mut env = build_workspace_env(&config);
        // Simulate agent-level override
        let agent_env: HashMap<String, String> =
            HashMap::from([("GIT_AUTHOR_NAME".to_string(), "Agent User".to_string())]);
        for (k, v) in &agent_env {
            env.insert(k.clone(), v.clone());
        }
        assert_eq!(env.get("GIT_AUTHOR_NAME").unwrap(), "Agent User");
        assert_eq!(env.get("GIT_COMMITTER_NAME").unwrap(), "Workspace User");
    }

    #[test]
    fn resolve_profile_limit_reads_max_concurrent() {
        let config = TuttiConfig {
            workspace: crate::config::WorkspaceConfig {
                name: "test".to_string(),
                description: None,
                env: None,
                auth: Some(WorkspaceAuth {
                    default_profile: Some("personal".to_string()),
                }),
            },
            defaults: DefaultsConfig {
                worktree: true,
                runtime: Some("claude-code".to_string()),
            },
            agents: vec![],
            handoff: None,
            observe: None,
        };
        let global = GlobalConfig {
            user: None,
            profiles: vec![ProfileConfig {
                name: "personal".to_string(),
                provider: "anthropic".to_string(),
                command: "claude".to_string(),
                max_concurrent: Some(3),
                monthly_budget: None,
                priority: None,
                plan: None,
                reset_day: None,
                weekly_hours: None,
            }],
            registered_workspaces: vec![],
            dashboard: None,
            resilience: None,
        };

        let limit = resolve_profile_limit(&config, &global).unwrap();
        assert_eq!(limit.profile_name, "personal");
        assert_eq!(limit.max_concurrent, 3);
    }

    #[test]
    fn resolve_profile_limit_none_when_unset() {
        let config = TuttiConfig {
            workspace: crate::config::WorkspaceConfig {
                name: "test".to_string(),
                description: None,
                env: None,
                auth: Some(WorkspaceAuth {
                    default_profile: Some("personal".to_string()),
                }),
            },
            defaults: DefaultsConfig {
                worktree: true,
                runtime: Some("claude-code".to_string()),
            },
            agents: vec![],
            handoff: None,
            observe: None,
        };
        let global = GlobalConfig {
            user: None,
            profiles: vec![ProfileConfig {
                name: "personal".to_string(),
                provider: "anthropic".to_string(),
                command: "claude".to_string(),
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
        };

        assert!(resolve_profile_limit(&config, &global).is_none());
    }

    #[test]
    fn is_api_usage_plan_is_case_insensitive() {
        assert!(is_api_usage_plan(Some("api")));
        assert!(is_api_usage_plan(Some("API")));
        assert!(is_api_usage_plan(Some(" Api ")));
    }

    #[test]
    fn is_api_usage_plan_rejects_non_api_and_missing() {
        assert!(!is_api_usage_plan(None));
        assert!(!is_api_usage_plan(Some("max")));
        assert!(!is_api_usage_plan(Some("pro")));
    }
}
