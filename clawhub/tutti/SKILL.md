---
name: tutti
description: Orchestrate multiple AI coding agents (Claude Code, Codex, Aider) from a single config — launch teams, run workflows, track capacity, and manage handoffs.
version: 1.0.0
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

## Execution pattern

Follow this sequence for orchestrating a workspace:

1. **Preflight** — `doctor_check`. Stop and report if non-zero.
2. **Launch** — `launch_team` or `launch_agent <name>`.
3. **Monitor** — `team_status` and `agent_output <name>` to observe progress.
4. **Workflow** — `list_workflows` to discover, then `run_workflow <name>`.
5. **Verify** — `verify_team --strict` for gate-style quality checks.
6. **Handoff** — `generate_handoff <agent>` when context is high, `apply_handoff <agent>` to resume.
7. **Stop** — `stop_team` or `stop_agent <name>` when done.

## Failure handling

- **Non-zero exit**: Surface the `action`, `command`, and `stderr` from the JSON envelope. Do not retry blindly.
- **Verify warnings (non-strict)**: Report as warning. Include data from `read_verify_status`.
- **Missing state files**: Treat as transient — retry up to 3 times with short delays. If still missing, the workspace may not have been launched.
- **Auth failures**: If `stderr` contains auth errors, stop and escalate to the user. Do not retry auth failures.

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
- Use `--json` output from `tt` commands when you need structured data (the wrapper handles this automatically).
- Do not parse `stdout` text output — always use the `data` field from the JSON envelope.
