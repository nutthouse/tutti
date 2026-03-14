use crate::config::{
    AgentConfig, DefaultsConfig, GlobalConfig, LaunchMode, LaunchPolicyMode, PermissionsConfig,
    TuttiConfig, global_config_path, topological_sort,
};
use crate::error::{Result, TuttiError};
use crate::permissions::{has_configured_policy, normalize, render_claude_settings};
use crate::runtime;
use crate::session::TmuxSession;
use crate::state;
use crate::state::{ControlEvent, PolicyDecisionRecord};
use crate::worktree;
use crate::{budget, budget::BudgetGuardOutcome};
use chrono::Utc;
use colored::Colorize;
use std::collections::{HashMap, HashSet};
use std::path::Path;

#[derive(Debug, Clone)]
struct ProfileLimit {
    profile_name: String,
    max_concurrent: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LaunchSettings {
    mode: LaunchMode,
    policy: LaunchPolicyMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LaunchCommandWarnings {
    constrained_best_effort: bool,
    unsupported_constrained_runtime: bool,
    bypass_mode: bool,
}

impl LaunchSettings {
    fn effective_policy(self) -> LaunchPolicyMode {
        match self.mode {
            LaunchMode::Auto => LaunchPolicyMode::Constrained,
            _ => self.policy,
        }
    }

    fn requires_constrained_policy(self) -> bool {
        self.effective_policy() == LaunchPolicyMode::Constrained && self.mode != LaunchMode::Safe
    }

    fn is_bypass(self) -> bool {
        self.effective_policy() == LaunchPolicyMode::Bypass && self.mode != LaunchMode::Safe
    }
}

pub fn run(
    agent_filter: Option<&str>,
    workspace_name: Option<&str>,
    all: bool,
    fresh_worktree: bool,
    mode_override: Option<super::UpLaunchMode>,
    policy_override: Option<super::UpLaunchPolicy>,
) -> Result<()> {
    crate::session::tmux::check_tmux()?;

    if all {
        return run_all(fresh_worktree, mode_override, policy_override);
    }

    let (config, config_path) = if let Some(ws) = workspace_name {
        load_workspace_by_name(ws)?
    } else {
        let cwd = std::env::current_dir()?;
        TuttiConfig::load(&cwd)?
    };
    config.validate()?;
    let launch_settings = resolve_launch_settings(&config, mode_override, policy_override);

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
    let permissions_policy =
        resolve_launch_permissions(global.as_ref(), &config, &agents, launch_settings)?;

    let mut launched = Vec::new();
    let mut refused_by_limit = false;
    let mut warned_best_effort = false;
    let mut warned_unsupported_constrained = false;
    let mut warned_bypass = false;

    for agent in &agents {
        let budget_outcome =
            budget::enforce_pre_exec(&config, project_root, "up", Some(&agent.name))?;
        print_budget_warnings(&budget_outcome);

        let runtime_name = agent.resolved_runtime(&config.defaults).ok_or_else(|| {
            TuttiError::ConfigValidation(format!(
                "agent '{}' has no runtime (set runtime on agent or in [defaults])",
                agent.name
            ))
        })?;

        let command_override =
            resolve_profile_command_for_runtime(&config, global.as_ref(), &runtime_name);
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
        let (working_dir, worktree_path, branch) =
            prepare_agent_working_dir(project_root, &config.defaults, agent, fresh_worktree);

        // Merge workspace env with agent-level env (agent overrides workspace)
        let mut env = workspace_env.clone();
        for (k, v) in &agent.env {
            env.insert(k.clone(), v.clone());
        }

        let (cmd, warnings) = build_launch_command(
            adapter.as_ref(),
            &runtime_name,
            launch_settings,
            permissions_policy,
            project_root,
            &agent.name,
            agent.prompt.as_deref(),
        )?;
        if warnings.constrained_best_effort && !warned_best_effort {
            eprintln!(
                "  {} constrained mode is best-effort for {}; hard allowlist enforcement is currently Claude-only",
                "warn".yellow(),
                runtime_name
            );
            warned_best_effort = true;
        }
        if warnings.unsupported_constrained_runtime && !warned_unsupported_constrained {
            eprintln!(
                "  {} constrained no-prompt policy is not supported for runtime {}; falling back to runtime defaults",
                "warn".yellow(),
                runtime_name
            );
            warned_unsupported_constrained = true;
        }
        if warnings.bypass_mode && !warned_bypass {
            eprintln!(
                "  {} launch policy is bypass; commands may run without permission prompts",
                "warn".yellow()
            );
            warned_bypass = true;
        }
        let _ = state::append_policy_decision(
            project_root,
            &launch_policy_record(
                &config.workspace.name,
                &agent.name,
                &runtime_name,
                launch_settings,
                permissions_policy,
                warnings,
                &cmd,
            ),
        );
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
        let _ = state::append_control_event(
            project_root,
            &ControlEvent {
                event: "agent.started".to_string(),
                workspace: config.workspace.name.clone(),
                agent: Some(agent.name.clone()),
                timestamp: Utc::now(),
                correlation_id: format!("launch-{}-{}", Utc::now().timestamp_millis(), agent.name),
                data: Some(serde_json::json!({
                    "runtime": runtime_name,
                    "session_name": session
                })),
            },
        );

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
    if let Some(event) = super::handoff::auto_handoff_post_launch(&config, project_root)? {
        eprintln!("  {} {}", "info".cyan(), event);
    }

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

fn prepare_agent_working_dir(
    project_root: &Path,
    defaults: &DefaultsConfig,
    agent: &AgentConfig,
    fresh_worktree: bool,
) -> (String, Option<std::path::PathBuf>, Option<String>) {
    if !agent.resolved_worktree(defaults) {
        return (project_root.to_string_lossy().to_string(), None, None);
    }

    let branch = agent.resolved_branch();
    let use_fresh = fresh_worktree || agent.resolved_fresh_worktree();

    if !use_fresh
        && let Ok(snapshot) = worktree::inspect_worktree(project_root, &agent.name)
        && snapshot.exists
    {
        if snapshot.dirty {
            eprintln!(
                "  {} {} worktree has uncommitted changes; reusing existing state",
                "warn".yellow(),
                agent.name
            );
        }
        if !snapshot.at_project_head {
            eprintln!(
                "  {} {} worktree is not at current HEAD; pass --fresh-worktree to reset",
                "warn".yellow(),
                agent.name
            );
        }
    }

    let ensure_result = if use_fresh {
        worktree::ensure_fresh_worktree(project_root, &agent.name, &branch)
    } else {
        worktree::ensure_worktree(project_root, &agent.name, &branch)
    };

    match ensure_result {
        Ok(wt_path) => (
            wt_path.to_string_lossy().to_string(),
            Some(wt_path),
            Some(branch),
        ),
        Err(e) => {
            eprintln!(
                "  {} worktree for {}: {e} (using project root)",
                "warn".yellow(),
                agent.name
            );
            (project_root.to_string_lossy().to_string(), None, None)
        }
    }
}

fn resolve_launch_settings(
    config: &TuttiConfig,
    mode_override: Option<super::UpLaunchMode>,
    policy_override: Option<super::UpLaunchPolicy>,
) -> LaunchSettings {
    let mode = mode_override
        .map(map_mode_override)
        .or(config.launch.as_ref().map(|launch| launch.mode))
        .unwrap_or(LaunchMode::Auto);

    let policy = policy_override
        .map(map_policy_override)
        .or(config.launch.as_ref().map(|launch| launch.policy))
        .unwrap_or(LaunchPolicyMode::Constrained);

    LaunchSettings { mode, policy }
}

fn map_mode_override(mode: super::UpLaunchMode) -> LaunchMode {
    match mode {
        super::UpLaunchMode::Safe => LaunchMode::Safe,
        super::UpLaunchMode::Auto => LaunchMode::Auto,
        super::UpLaunchMode::Unattended => LaunchMode::Unattended,
    }
}

fn map_policy_override(policy: super::UpLaunchPolicy) -> LaunchPolicyMode {
    match policy {
        super::UpLaunchPolicy::Constrained => LaunchPolicyMode::Constrained,
        super::UpLaunchPolicy::Bypass => LaunchPolicyMode::Bypass,
    }
}

fn resolve_launch_permissions<'a>(
    global: Option<&'a GlobalConfig>,
    config: &TuttiConfig,
    agents: &[&crate::config::AgentConfig],
    launch_settings: LaunchSettings,
) -> Result<Option<&'a PermissionsConfig>> {
    let policy = global.and_then(|g| {
        if has_configured_policy(g) {
            g.permissions.as_ref()
        } else {
            None
        }
    });

    if !launch_settings.requires_constrained_policy() {
        return Ok(policy);
    }

    let needs_supported_runtime_policy = agents.iter().any(|agent| {
        agent
            .resolved_runtime(&config.defaults)
            .as_deref()
            .is_some_and(runtime_supports_policy_constrained_no_prompt)
    });
    if !needs_supported_runtime_policy {
        return Ok(policy);
    }

    if policy.is_some() {
        return Ok(policy);
    }

    let path = global_config_path();
    Err(TuttiError::ConfigValidation(format!(
        "launch mode requires [permissions] allow rules in {} for constrained non-interactive runs. Configure [permissions], or run `tt up --mode safe`, or run `tt up --mode unattended --policy bypass`",
        path.display()
    )))
}

fn runtime_supports_policy_constrained_no_prompt(runtime_name: &str) -> bool {
    matches!(runtime_name, "claude-code" | "codex" | "openclaw" | "aider")
}

fn build_launch_command(
    adapter: &dyn runtime::RuntimeAdapter,
    runtime_name: &str,
    launch_settings: LaunchSettings,
    permissions_policy: Option<&PermissionsConfig>,
    project_root: &Path,
    agent_name: &str,
    base_prompt: Option<&str>,
) -> Result<(String, LaunchCommandWarnings)> {
    if launch_settings.mode == LaunchMode::Safe {
        return Ok((
            adapter.build_spawn_command(base_prompt),
            LaunchCommandWarnings {
                constrained_best_effort: false,
                unsupported_constrained_runtime: false,
                bypass_mode: false,
            },
        ));
    }

    if launch_settings.is_bypass() {
        let pre_args = match runtime_name {
            "claude-code" => vec![
                "--permission-mode".to_string(),
                "bypassPermissions".to_string(),
            ],
            "codex" => vec!["--dangerously-bypass-approvals-and-sandbox".to_string()],
            _ => vec![],
        };
        let cmd = adapter.build_spawn_command_with_args(&pre_args, base_prompt);
        return Ok((
            cmd,
            LaunchCommandWarnings {
                constrained_best_effort: false,
                unsupported_constrained_runtime: false,
                bypass_mode: true,
            },
        ));
    }

    match runtime_name {
        "claude-code" => {
            let policy = permissions_policy.ok_or_else(|| {
                TuttiError::ConfigValidation(
                    "claude constrained launch requires configured [permissions] policy"
                        .to_string(),
                )
            })?;
            let settings_path = write_claude_settings_file(project_root, agent_name, policy)?;
            let pre_args = vec![
                "--permission-mode".to_string(),
                "dontAsk".to_string(),
                "--settings".to_string(),
                settings_path.display().to_string(),
            ];
            Ok((
                adapter.build_spawn_command_with_args(&pre_args, base_prompt),
                LaunchCommandWarnings {
                    constrained_best_effort: false,
                    unsupported_constrained_runtime: false,
                    bypass_mode: false,
                },
            ))
        }
        "codex" => {
            let policy = permissions_policy.ok_or_else(|| {
                TuttiError::ConfigValidation(
                    "codex constrained launch requires configured [permissions] policy".to_string(),
                )
            })?;
            let policy_appendix = best_effort_policy_appendix("Codex", policy);
            let prompt = append_policy_prompt(base_prompt, &policy_appendix);
            let pre_args = vec![
                "-a".to_string(),
                "never".to_string(),
                "-s".to_string(),
                "workspace-write".to_string(),
            ];
            Ok((
                adapter.build_spawn_command_with_args(&pre_args, prompt.as_deref()),
                LaunchCommandWarnings {
                    constrained_best_effort: true,
                    unsupported_constrained_runtime: false,
                    bypass_mode: false,
                },
            ))
        }
        "openclaw" => {
            let policy = permissions_policy.ok_or_else(|| {
                TuttiError::ConfigValidation(
                    "openclaw constrained launch requires configured [permissions] policy"
                        .to_string(),
                )
            })?;
            let policy_appendix = best_effort_policy_appendix("OpenClaw", policy);
            let prompt = append_policy_prompt(base_prompt, &policy_appendix);
            Ok((
                adapter.build_spawn_command(prompt.as_deref()),
                LaunchCommandWarnings {
                    constrained_best_effort: true,
                    unsupported_constrained_runtime: false,
                    bypass_mode: false,
                },
            ))
        }
        "aider" => {
            let policy = permissions_policy.ok_or_else(|| {
                TuttiError::ConfigValidation(
                    "aider constrained launch requires configured [permissions] policy".to_string(),
                )
            })?;
            let policy_appendix = best_effort_policy_appendix("Aider", policy);
            let prompt = append_policy_prompt(base_prompt, &policy_appendix);
            Ok((
                adapter.build_spawn_command(prompt.as_deref()),
                LaunchCommandWarnings {
                    constrained_best_effort: true,
                    unsupported_constrained_runtime: false,
                    bypass_mode: false,
                },
            ))
        }
        _ => Ok((
            adapter.build_spawn_command(base_prompt),
            LaunchCommandWarnings {
                constrained_best_effort: false,
                unsupported_constrained_runtime: true,
                bypass_mode: false,
            },
        )),
    }
}

fn write_claude_settings_file(
    project_root: &Path,
    agent_name: &str,
    policy: &PermissionsConfig,
) -> Result<std::path::PathBuf> {
    let settings_dir = project_root
        .join(".tutti")
        .join("state")
        .join("runtime-settings");
    std::fs::create_dir_all(&settings_dir)?;
    let settings_path = settings_dir.join(format!("{agent_name}-claude-settings.json"));
    let rendered = render_claude_settings(policy)?;
    std::fs::write(&settings_path, format!("{rendered}\n"))?;
    Ok(settings_path)
}

fn best_effort_policy_appendix(runtime_label: &str, policy: &PermissionsConfig) -> String {
    let rules: Vec<String> = policy
        .allow
        .iter()
        .map(normalize)
        .filter(|entry| !entry.is_empty())
        .collect();

    let mut out = format!(
        "Tutti policy constraints (best-effort for {runtime_label}):\n\
         Only execute Bash commands matching one of these allow rules:\n"
    );
    for rule in &rules {
        out.push_str("- ");
        out.push_str(rule);
        out.push('\n');
    }
    out.push_str(
        "If a required command is outside policy, do not run it. Report the blocked command and why.",
    );
    out
}

fn append_policy_prompt(base_prompt: Option<&str>, appendix: &str) -> Option<String> {
    match base_prompt.map(str::trim).filter(|s| !s.is_empty()) {
        Some(base) => Some(format!("{base}\n\n{appendix}")),
        None => Some(appendix.to_string()),
    }
}

fn launch_mode_label(settings: LaunchSettings) -> &'static str {
    match settings.mode {
        LaunchMode::Safe => "safe",
        LaunchMode::Auto => "auto",
        LaunchMode::Unattended => "unattended",
    }
}

fn launch_policy_label(settings: LaunchSettings) -> &'static str {
    match settings.effective_policy() {
        LaunchPolicyMode::Constrained => "constrained",
        LaunchPolicyMode::Bypass => "bypass",
    }
}

fn launch_policy_record(
    workspace: &str,
    agent: &str,
    runtime_name: &str,
    settings: LaunchSettings,
    permissions_policy: Option<&PermissionsConfig>,
    warnings: LaunchCommandWarnings,
    command: &str,
) -> PolicyDecisionRecord {
    let (enforcement, decision, reason) = if settings.mode == LaunchMode::Safe {
        (
            "prompt".to_string(),
            "allow".to_string(),
            Some("interactive approval mode".to_string()),
        )
    } else if settings.is_bypass() {
        (
            "bypass".to_string(),
            "allow".to_string(),
            Some("unattended bypass policy".to_string()),
        )
    } else if warnings.unsupported_constrained_runtime {
        (
            "unsupported".to_string(),
            "allow".to_string(),
            Some("runtime does not support constrained no-prompt policy".to_string()),
        )
    } else if warnings.constrained_best_effort {
        (
            "best_effort".to_string(),
            "allow".to_string(),
            Some("constrained policy guidance only (not hard-enforced)".to_string()),
        )
    } else {
        (
            "hard".to_string(),
            "allow".to_string(),
            Some("constrained policy is hard-enforced by runtime".to_string()),
        )
    };

    PolicyDecisionRecord {
        timestamp: Utc::now(),
        workspace: workspace.to_string(),
        agent: Some(agent.to_string()),
        runtime: Some(runtime_name.to_string()),
        action: "launch".to_string(),
        mode: launch_mode_label(settings).to_string(),
        policy: launch_policy_label(settings).to_string(),
        enforcement,
        decision,
        reason,
        data: Some(serde_json::json!({
            "policy_rules": permissions_policy.map_or(0, |p| p.allow.len()),
            "runtime": runtime_name,
            "command": command
        })),
    }
}

/// Resolve a runtime-compatible command override from the workspace profile.
fn resolve_profile_command_for_runtime(
    config: &TuttiConfig,
    global: Option<&GlobalConfig>,
    runtime_name: &str,
) -> Option<String> {
    let profile_name = config.workspace.auth.as_ref()?.default_profile.as_ref()?;
    let profile = global?.get_profile(profile_name)?;
    runtime::compatible_command_override(
        runtime_name,
        Some(profile.provider.as_str()),
        Some(profile.command.as_str()),
    )
    .map(ToString::to_string)
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

fn print_budget_warnings(outcome: &BudgetGuardOutcome) {
    for warning in &outcome.warnings {
        eprintln!("  {} {}", "warn".yellow(), warning);
    }
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
fn run_all(
    fresh_worktree: bool,
    mode_override: Option<super::UpLaunchMode>,
    policy_override: Option<super::UpLaunchPolicy>,
) -> Result<()> {
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
                let launch_settings =
                    resolve_launch_settings(&config, mode_override, policy_override);
                let project_root = config_path.parent().unwrap();
                state::ensure_tutti_dir(project_root)?;

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
                let permissions_policy =
                    resolve_launch_permissions(Some(&global), &config, &sorted, launch_settings)?;
                let mut warned_best_effort = false;
                let mut warned_unsupported_constrained = false;
                let mut warned_bypass = false;

                for agent in sorted {
                    match budget::enforce_pre_exec(&config, project_root, "up", Some(&agent.name)) {
                        Ok(outcome) => print_budget_warnings(&outcome),
                        Err(e) => {
                            eprintln!("  Skipping {} ({e})", agent.name);
                            continue;
                        }
                    }

                    let runtime_name = match agent.resolved_runtime(&config.defaults) {
                        Some(rt) => rt,
                        None => {
                            eprintln!("  Skipping {} (no runtime)", agent.name);
                            continue;
                        }
                    };
                    let command_override =
                        resolve_profile_command_for_runtime(&config, Some(&global), &runtime_name);
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

                    let (working_dir, worktree_path, branch) = prepare_agent_working_dir(
                        project_root,
                        &config.defaults,
                        agent,
                        fresh_worktree,
                    );
                    let (cmd, warnings) = match build_launch_command(
                        adapter.as_ref(),
                        &runtime_name,
                        launch_settings,
                        permissions_policy,
                        project_root,
                        &agent.name,
                        agent.prompt.as_deref(),
                    ) {
                        Ok(v) => v,
                        Err(e) => {
                            eprintln!("  Failed to prepare launch for {}: {e}", agent.name);
                            continue;
                        }
                    };
                    if warnings.constrained_best_effort && !warned_best_effort {
                        eprintln!(
                            "  {} constrained mode is best-effort for {}; hard allowlist enforcement is currently Claude-only",
                            "warn".yellow(),
                            runtime_name
                        );
                        warned_best_effort = true;
                    }
                    if warnings.unsupported_constrained_runtime && !warned_unsupported_constrained {
                        eprintln!(
                            "  {} constrained no-prompt policy is not supported for runtime {}; falling back to runtime defaults",
                            "warn".yellow(),
                            runtime_name
                        );
                        warned_unsupported_constrained = true;
                    }
                    if warnings.bypass_mode && !warned_bypass {
                        eprintln!(
                            "  {} launch policy is bypass; commands may run without permission prompts",
                            "warn".yellow()
                        );
                        warned_bypass = true;
                    }
                    let _ = state::append_policy_decision(
                        project_root,
                        &launch_policy_record(
                            &config.workspace.name,
                            &agent.name,
                            &runtime_name,
                            launch_settings,
                            permissions_policy,
                            warnings,
                            &cmd,
                        ),
                    );
                    if let Err(e) = TmuxSession::create_session(&session, &working_dir, &cmd, &env)
                    {
                        eprintln!("  Failed to launch {}: {e}", agent.name);
                        continue;
                    }

                    let agent_state = state::AgentState {
                        name: agent.name.clone(),
                        runtime: runtime_name,
                        session_name: session,
                        worktree_path,
                        branch,
                        status: "Working".to_string(),
                        started_at: Utc::now(),
                        stopped_at: None,
                    };
                    let _ = state::save_agent_state(project_root, &agent_state);
                    let _ = state::append_control_event(
                        project_root,
                        &ControlEvent {
                            event: "agent.started".to_string(),
                            workspace: config.workspace.name.clone(),
                            agent: Some(agent.name.clone()),
                            timestamp: Utc::now(),
                            correlation_id: format!(
                                "launch-{}-{}",
                                Utc::now().timestamp_millis(),
                                agent.name
                            ),
                            data: Some(serde_json::json!({
                                "runtime": agent_state.runtime,
                                "session_name": agent_state.session_name
                            })),
                        },
                    );
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
    use crate::config::{
        AgentConfig, DefaultsConfig, GlobalConfig, LaunchConfig, LaunchMode, LaunchPolicyMode,
        PermissionsConfig, ProfileConfig, WorkspaceAuth, WorkspaceConfig,
    };

    fn make_agent(name: &str, deps: Vec<&str>) -> AgentConfig {
        AgentConfig {
            name: name.to_string(),
            runtime: Some("claude-code".to_string()),
            scope: None,
            prompt: None,
            depends_on: deps.into_iter().map(|s| s.to_string()).collect(),
            worktree: None,
            fresh_worktree: None,
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
            launch: None,
            agents: vec![],
            tool_packs: vec![],
            workflows: vec![],
            hooks: vec![],
            handoff: None,
            observe: None,
            budget: None,
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
            launch: None,
            agents: vec![],
            tool_packs: vec![],
            workflows: vec![],
            hooks: vec![],
            handoff: None,
            observe: None,
            budget: None,
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
            launch: None,
            agents: vec![],
            tool_packs: vec![],
            workflows: vec![],
            hooks: vec![],
            handoff: None,
            observe: None,
            budget: None,
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
            permissions: None,
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
            launch: None,
            agents: vec![],
            tool_packs: vec![],
            workflows: vec![],
            hooks: vec![],
            handoff: None,
            observe: None,
            budget: None,
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
            permissions: None,
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

    #[test]
    fn resolve_profile_command_for_runtime_ignores_mismatched_profile_command() {
        let config = TuttiConfig {
            workspace: crate::config::WorkspaceConfig {
                name: "test".to_string(),
                description: None,
                env: None,
                auth: Some(WorkspaceAuth {
                    default_profile: Some("claude-profile".to_string()),
                }),
            },
            defaults: DefaultsConfig {
                worktree: true,
                runtime: Some("codex".to_string()),
            },
            launch: None,
            agents: vec![],
            tool_packs: vec![],
            workflows: vec![],
            hooks: vec![],
            handoff: None,
            observe: None,
            budget: None,
        };
        let global = GlobalConfig {
            user: None,
            profiles: vec![ProfileConfig {
                name: "claude-profile".to_string(),
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
            permissions: None,
        };

        assert_eq!(
            resolve_profile_command_for_runtime(&config, Some(&global), "codex"),
            None
        );
    }

    #[test]
    fn resolve_profile_command_for_runtime_applies_matching_profile_command() {
        let config = TuttiConfig {
            workspace: crate::config::WorkspaceConfig {
                name: "test".to_string(),
                description: None,
                env: None,
                auth: Some(WorkspaceAuth {
                    default_profile: Some("codex-profile".to_string()),
                }),
            },
            defaults: DefaultsConfig {
                worktree: true,
                runtime: Some("codex".to_string()),
            },
            launch: None,
            agents: vec![],
            tool_packs: vec![],
            workflows: vec![],
            hooks: vec![],
            handoff: None,
            observe: None,
            budget: None,
        };
        let global = GlobalConfig {
            user: None,
            profiles: vec![ProfileConfig {
                name: "codex-profile".to_string(),
                provider: "openai".to_string(),
                command: "/opt/bin/codex-prod".to_string(),
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
        };

        assert_eq!(
            resolve_profile_command_for_runtime(&config, Some(&global), "codex").as_deref(),
            Some("/opt/bin/codex-prod")
        );
    }

    #[test]
    fn resolve_launch_settings_prefers_cli_over_workspace_config() {
        let config = TuttiConfig {
            workspace: WorkspaceConfig {
                name: "test".to_string(),
                description: None,
                env: None,
                auth: None,
            },
            defaults: DefaultsConfig {
                worktree: true,
                runtime: Some("claude-code".to_string()),
            },
            launch: Some(LaunchConfig {
                mode: LaunchMode::Safe,
                policy: LaunchPolicyMode::Bypass,
            }),
            agents: vec![],
            tool_packs: vec![],
            workflows: vec![],
            hooks: vec![],
            handoff: None,
            observe: None,
            budget: None,
        };

        let resolved = resolve_launch_settings(
            &config,
            Some(super::super::UpLaunchMode::Unattended),
            Some(super::super::UpLaunchPolicy::Constrained),
        );
        assert_eq!(resolved.mode, LaunchMode::Unattended);
        assert_eq!(resolved.policy, LaunchPolicyMode::Constrained);
    }

    #[test]
    fn resolve_launch_permissions_requires_policy_for_constrained_supported_runtimes() {
        let config = TuttiConfig {
            workspace: WorkspaceConfig {
                name: "test".to_string(),
                description: None,
                env: None,
                auth: None,
            },
            defaults: DefaultsConfig {
                worktree: true,
                runtime: Some("claude-code".to_string()),
            },
            launch: None,
            agents: vec![make_agent("backend", vec![])],
            tool_packs: vec![],
            workflows: vec![],
            hooks: vec![],
            handoff: None,
            observe: None,
            budget: None,
        };
        let launch_settings = LaunchSettings {
            mode: LaunchMode::Auto,
            policy: LaunchPolicyMode::Constrained,
        };
        let agents = topological_sort(&config.agents).expect("agents should sort");
        let err = resolve_launch_permissions(
            Some(&GlobalConfig::default()),
            &config,
            &agents,
            launch_settings,
        )
        .expect_err("constrained launch should require policy");
        assert!(err.to_string().contains("[permissions]"));
    }

    #[test]
    fn build_launch_command_codex_constrained_adds_flags_and_policy_prompt() {
        let adapter = runtime::get_adapter("codex", None).expect("codex adapter");
        let dir = std::env::temp_dir().join(format!("tutti-test-up-codex-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let policy = PermissionsConfig {
            allow: vec!["git status".to_string(), "cargo test".to_string()],
        };

        let (cmd, warnings) = build_launch_command(
            adapter.as_ref(),
            "codex",
            LaunchSettings {
                mode: LaunchMode::Auto,
                policy: LaunchPolicyMode::Constrained,
            },
            Some(&policy),
            &dir,
            "backend",
            Some("You own tests."),
        )
        .expect("command should build");

        assert!(cmd.contains("'-a' 'never' '-s' 'workspace-write'"));
        assert!(cmd.contains("--prompt"));
        assert!(cmd.contains("Tutti policy constraints"));
        assert!(warnings.constrained_best_effort);
        assert!(!warnings.unsupported_constrained_runtime);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_launch_command_openclaw_constrained_adds_policy_prompt() {
        let adapter = runtime::get_adapter("openclaw", None).expect("openclaw adapter");
        let dir =
            std::env::temp_dir().join(format!("tutti-test-up-openclaw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let policy = PermissionsConfig {
            allow: vec!["git status".to_string(), "cargo test".to_string()],
        };

        let (cmd, warnings) = build_launch_command(
            adapter.as_ref(),
            "openclaw",
            LaunchSettings {
                mode: LaunchMode::Auto,
                policy: LaunchPolicyMode::Constrained,
            },
            Some(&policy),
            &dir,
            "backend",
            Some("You own tests."),
        )
        .expect("command should build");

        assert!(cmd.contains("--prompt"));
        assert!(cmd.contains("Tutti policy constraints"));
        assert!(warnings.constrained_best_effort);
        assert!(!warnings.unsupported_constrained_runtime);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_launch_command_aider_constrained_adds_policy_prompt() {
        let adapter = runtime::get_adapter("aider", None).expect("aider adapter");
        let dir = std::env::temp_dir().join(format!("tutti-test-up-aider-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let policy = PermissionsConfig {
            allow: vec!["git status".to_string(), "cargo test".to_string()],
        };

        let (cmd, warnings) = build_launch_command(
            adapter.as_ref(),
            "aider",
            LaunchSettings {
                mode: LaunchMode::Auto,
                policy: LaunchPolicyMode::Constrained,
            },
            Some(&policy),
            &dir,
            "backend",
            Some("You own tests."),
        )
        .expect("command should build");

        assert!(cmd.contains("--message"));
        assert!(cmd.contains("Tutti policy constraints"));
        assert!(warnings.constrained_best_effort);
        assert!(!warnings.unsupported_constrained_runtime);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_launch_command_bypass_warns_even_without_runtime_specific_flags() {
        let adapter = runtime::get_adapter("openclaw", None).expect("openclaw adapter");
        let dir = std::env::temp_dir().join(format!(
            "tutti-test-up-bypass-openclaw-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");

        let (_cmd, warnings) = build_launch_command(
            adapter.as_ref(),
            "openclaw",
            LaunchSettings {
                mode: LaunchMode::Unattended,
                policy: LaunchPolicyMode::Bypass,
            },
            None,
            &dir,
            "backend",
            Some("You own tests."),
        )
        .expect("command should build");

        assert!(warnings.bypass_mode);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
