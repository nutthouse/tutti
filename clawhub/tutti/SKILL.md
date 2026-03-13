---
name: tutti
description: Orchestrate multiple AI coding agents (Claude Code, Codex, Aider) from a single config — launch teams, run workflows, track capacity, and manage handoffs.
version: 1.1.0
metadata:
  openclaw:
    requires:
      bins:
        - tt
        - tmux
        - python3
    emoji: "\U0001F3B6"
    homepage: https://github.com/nutthouse/tutti
---

# Tutti — Multi-Agent Orchestration

Orchestrate a team of AI coding agents from a declarative `tutti.toml` config. Launch agents in isolated git worktrees, run verification workflows, track token usage, and manage context handoffs — all through a single CLI.

## When to use this skill

Use when the user asks you to:
- Launch, monitor, or stop a team of AI coding agents
- Run or verify automated workflows across agents
- Dispatch prompts to agents with auto-start and output capture
- Land agent work back to the main branch or open PRs
- Check agent status, health, or capacity usage
- Generate or apply context handoff packets
- Coordinate multi-agent development workflows

## Prerequisites

1. `tt` binary installed and on PATH (install from https://github.com/nutthouse/tutti)
2. `tmux` installed
3. `python3` available
4. A `tutti.toml` config file in the workspace root

Always run preflight checks before starting a workflow:
```bash
python3 tutti_openclaw.py doctor_check
```

## Actions

All actions go through the wrapper script. Every action returns a consistent JSON envelope:

```json
{
  "ok": true,
  "action": "action_name",
  "command": ["tt", "..."],
  "exit_code": 0,
  "data": {},
  "stdout": "",
  "stderr": ""
}
```

### Lifecycle

| Action | Command | Purpose |
|--------|---------|---------|
| `doctor_check` | `python3 tutti_openclaw.py doctor_check` | Preflight: verify tools, config, and environment |
| `launch_team` | `python3 tutti_openclaw.py launch_team` | Launch all agents defined in tutti.toml |
| `launch_agent` | `python3 tutti_openclaw.py launch_agent <name>` | Launch a single agent |
| `send_prompt` | `python3 tutti_openclaw.py send_prompt <agent> <prompt...> [--auto-up] [--wait] [--output]` | Send a prompt to an agent with optional auto-start, wait-for-idle, and output capture |
| `team_status` | `python3 tutti_openclaw.py team_status` | Read agent states from .tutti/state/ |
| `agent_output` | `python3 tutti_openclaw.py agent_output <name> --lines 50` | Peek at an agent's terminal output |
| `stop_agent` | `python3 tutti_openclaw.py stop_agent <name>` | Stop a single agent |
| `stop_team` | `python3 tutti_openclaw.py stop_team` | Stop all agents |

### Workflows

| Action | Command | Purpose |
|--------|---------|---------|
| `list_workflows` | `python3 tutti_openclaw.py list_workflows` | Discover available workflows |
| `plan_workflow` | `python3 tutti_openclaw.py plan_workflow <name> [--strict]` | Dry-run a workflow |
| `run_workflow` | `python3 tutti_openclaw.py run_workflow <name> [--agent <a>] [--strict]` | Execute a workflow |
| `verify_team` | `python3 tutti_openclaw.py verify_team [--workflow <w>] [--strict]` | Run verification workflow |
| `read_verify_status` | `python3 tutti_openclaw.py read_verify_status` | Read last verification result |

### Git Operations

| Action | Command | Purpose |
|--------|---------|---------|
| `land_agent` | `python3 tutti_openclaw.py land_agent <agent> [--pr] [--force]` | Land an agent's branch back to current branch, or open a PR |

### Handoffs

| Action | Command | Purpose |
|--------|---------|---------|
| `generate_handoff` | `python3 tutti_openclaw.py generate_handoff <agent> [--reason <r>]` | Capture agent context to a packet |
| `apply_handoff` | `python3 tutti_openclaw.py apply_handoff <agent> [--packet <path>]` | Inject a handoff packet into an agent |
| `list_handoffs` | `python3 tutti_openclaw.py list_handoffs [--agent <a>] [--limit 20]` | List available handoff packets |

### Permissions

| Action | Command | Purpose |
|--------|---------|---------|
| `permissions_check` | `python3 tutti_openclaw.py permissions_check <cmd...>` | Check if a command is allowed by policy |

## Workflow step types

Workflows in `tutti.toml` support these step types:

| Type | Purpose | Key fields |
|------|---------|------------|
| `prompt` | Send text to an agent session | `agent`, `text`, `inject_files`, `wait_for_idle`, `wait_timeout_secs` |
| `command` | Execute a shell command | `run`, `cwd`, `timeout_secs`, `fail_mode` |
| `ensure_running` | Start an agent if not already running | `agent`, `fail_mode` |
| `workflow` | Execute another workflow as a nested step | `workflow`, `agent`, `strict`, `fail_mode` |
| `land` | Land an agent's branch | `agent`, `pr`, `force`, `fail_mode` |
| `review` | Send an agent's diff to a reviewer | `agent`, `reviewer`, `fail_mode` |

Prompt steps support `inject_files` — an array of workspace-relative file paths that are copied into the agent's worktree before the prompt is sent. This enables stateful context passing between agents (e.g., injecting a snapshot JSON produced by another agent).

Nested `workflow` steps enable composition: observe → dispatch → fix → verify → land as a chain of workflow invocations.

## Execution pattern

Follow this sequence for orchestrating a workspace:

1. **Preflight** — `doctor_check`. Stop and report if non-zero.
2. **Launch** — `launch_team` or `launch_agent <name>`.
3. **Monitor** — `team_status` and `agent_output <name>` to observe progress.
4. **Dispatch** — `send_prompt <agent> "do something" --auto-up --wait --output` to dispatch work and capture results.
5. **Workflow** — `list_workflows` to discover, then `run_workflow <name>`.
6. **Verify** — `verify_team --strict` for gate-style quality checks.
7. **Land** — `land_agent <agent>` to cherry-pick work, or `land_agent <agent> --pr` to open a PR.
8. **Handoff** — `generate_handoff <agent>` when context is high, `apply_handoff <agent>` to resume.
9. **Stop** — `stop_team` or `stop_agent <name>` when done.

## Failure handling

- **Non-zero exit**: Surface the `action`, `command`, and `stderr` from the JSON envelope. Do not retry blindly.
- **Verify warnings (non-strict)**: Report as warning. Include data from `read_verify_status`.
- **Missing state files**: Treat as transient — retry up to 3 times with short delays. If still missing, the workspace may not have been launched.
- **Auth failures**: If `stderr` contains auth errors, stop and escalate to the user. Do not retry auth failures.
- **Agent not running**: Use `--auto-up` on `send_prompt` to automatically start agents on demand rather than failing.

## Configuration override

If `tt` is not on PATH or you need a specific version:

```bash
python3 tutti_openclaw.py --tt-bin /path/to/tt doctor_check
# or via environment variable
TUTTI_BIN=/path/to/tt python3 tutti_openclaw.py doctor_check
```

## Rules

- Always run `doctor_check` before any launch or workflow operation.
- Never retry auth failures — escalate to the user immediately.
- Prefer `team_status` (reads state files directly) over `agent_output` for status checks.
- Use `--strict` flag on `verify_team` and `run_workflow` when results gate further actions.
- Use `--auto-up` on `send_prompt` when the target agent may not be running.
- Use `--output` on `send_prompt` to capture the agent's response for programmatic verification.
- Use `--json` output from `tt` commands when you need structured data (the wrapper handles this automatically).
- Do not parse `stdout` text output — always use the `data` field from the JSON envelope.
