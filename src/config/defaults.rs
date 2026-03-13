/// Default tutti.toml template written by `tt init`.
pub const DEFAULT_CONFIG: &str = r#"# tutti.toml — your agent team configuration
# Docs: https://github.com/nutthouse/tutti

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

# Launch autonomy settings (optional)
# [launch]
# mode = "auto"            # safe | auto | unattended
# policy = "constrained"   # constrained | bypass
#
# Notes:
# - mode = "auto" runs non-interactive and policy-constrained (best default for agent autonomy).
# - constrained mode requires [permissions] in ~/.config/tutti/config.toml.
# - mode = "unattended" + policy = "bypass" is highest autonomy and highest risk.

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
# [agent.env]
# CUSTOM_VAR = "value"           # agent-level env vars (override workspace env)

# [[agent]]
# name = "conductor"
# runtime = "openclaw"
# prompt = "Coordinate other agents and keep workflows moving."

# [[agent]]
# name = "pr-monitor"
# runtime = "claude-code"
# prompt = "Monitor open PRs. Check CI status."
# persistent = true          # keeps running, doesn't "finish"

# Reusable automation workflows (opt-in)
# [[workflow]]
# name = "verify-app"
# description = "Run deterministic checks before merge."
# schedule = "*/30 * * * *"       # optional 5-field local-time cron
# [[workflow.step]]
# id = "tests"
# type = "command"
# run = "cargo test --quiet"
# cwd = "workspace"
# fail_mode = "closed"
# timeout_secs = 1200
# output_json = ".tutti/state/verify.json"
#
# [[workflow]]
# name = "code-simplifier"
# [[workflow.step]]
# type = "ensure_running"
# agent = "backend"
# fail_mode = "closed"
# [[workflow.step]]
# type = "prompt"
# id = "simplify"
# agent = "backend"
# text = "Simplify/refactor recent changes and keep behavior identical."
# inject_files = [".tutti/state/snapshot.json"]  # copy workspace file into agent worktree before prompt
# wait_for_idle = true
# wait_timeout_secs = 900
# [[workflow.step]]
# type = "command"
# id = "fmt"
# run = "cargo fmt"
# cwd = "workspace"
# fail_mode = "open"
#
# [[workflow]]
# name = "autofix-loop"
# [[workflow.step]]
# type = "workflow"
# workflow = "verify-app"
# strict = true
# fail_mode = "closed"
# [[workflow.step]]
# type = "review"
# agent = "backend"
# reviewer = "reviewer"
# fail_mode = "open"
# [[workflow.step]]
# type = "land"
# agent = "backend"
# force = true
# fail_mode = "closed"
#
# [[hook]]
# event = "agent_stop"
# agent = "backend"
# workflow = "verify-app"
# fail_mode = "open"
#
# [[hook]]
# event = "workflow_complete"
# workflow_source = "observe_cycle"
# workflow_name = "verify-app"
# run = "echo workflow complete"
#
# Optional tool-pack prerequisites for `tt doctor` (Milestone 3)
# [[tool_pack]]
# name = "analytics"
# description = "BigQuery + jq based analysis workflows"
# required_commands = ["bq", "jq"]
# required_env = ["GCP_PROJECT"]

# Handoff settings (Phase 2)
# [handoff]
# auto = true
# threshold = 0.2
# include = ["active_task", "file_changes", "decisions", "blockers"]

# Dashboard settings (reserved for future web UI; currently ignored by CLI runtime paths)
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
# plan = "max"                     # "free", "pro", "max", "team", "api"
# reset_day = "monday"             # weekly reset day for capacity tracking
# weekly_hours = 45.0              # capacity ceiling in compute-hours

# Dashboard settings
# [dashboard]
# port = 4040
# show_all_workspaces = true       # dashboard shows ALL registered workspaces

# Global resilience defaults
# [resilience]
# provider_down_strategy = "pause"
# save_state_on_failure = true
#
# Team-shared command allowlist policy (opt-in)
# [permissions]
# allow = [
#   "git status",
#   "git diff",
#   "git log",
#   "cargo test",
#   "Read",      # optional Claude tool names
#   "Edit",
# ]
"#;
