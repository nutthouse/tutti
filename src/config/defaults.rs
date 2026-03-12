/// Default tutti.toml template written by `tt init`.
pub const DEFAULT_CONFIG: &str = r#"# tutti.toml — your agent team configuration
# Docs: https://github.com/adamnutt/tutti

[workspace]
name = "my-project"
description = "My project workspace"

# Workspace-specific environment
# [workspace.env]
# git_name = "Your Name"
# git_email = "you@example.com"
# GITHUB_USER = "yourusername"

# Auth profile to use in this workspace (defined in ~/.config/tutti/config.toml)
# [workspace.auth]
# default_profile = "claude-personal"

# Default settings applied to all agents unless overridden
[defaults]
worktree = true           # git worktree isolation per agent
runtime = "claude-code"   # default runtime for agents

# Define your agents — each gets its own terminal session
[[agent]]
name = "backend"
scope = "src/api/**"
prompt = "You own the API layer. Follow existing patterns."

[[agent]]
name = "frontend"
scope = "src/app/**"
prompt = "You own the UI. Follow existing component patterns."

# [[agent]]
# name = "tests"
# runtime = "codex"
# scope = "tests/**"
# prompt = "Write and maintain tests."
# depends_on = ["backend", "frontend"]

# [[agent]]
# name = "pr-monitor"
# runtime = "claude-code"
# prompt = "Monitor open PRs. Check CI status."
# persistent = true          # keeps running, doesn't "finish"

# Handoff settings (Phase 2)
# [handoff]
# auto = true
# threshold = 0.2
# include = ["active_task", "file_changes", "decisions", "blockers"]

# Dashboard settings (Phase 3)
# [observe]
# dashboard = true
# port = 4040
# track_cost = true
"#;

/// Default global config template written to ~/.config/tutti/config.toml.
pub const DEFAULT_GLOBAL_CONFIG: &str = r#"# Tutti global config — applies across all workspaces
# This file is auto-created by `tt init` and updated as you register workspaces.

# [user]
# name = "Your Name"

# Subscription profiles (shared across all workspaces)
# [[profile]]
# name = "claude-personal"
# provider = "anthropic"
# command = "claude"
# max_concurrent = 5
# monthly_budget = 100.00

# [[profile]]
# name = "claude-work"
# provider = "anthropic"
# command = "claude"
# max_concurrent = 10
# priority = 2                     # fallback when personal hits limits

# Dashboard settings
# [dashboard]
# port = 4040
# show_all_workspaces = true       # dashboard shows ALL registered workspaces

# Global resilience defaults
# [resilience]
# provider_down_strategy = "pause"
# save_state_on_failure = true
"#;
