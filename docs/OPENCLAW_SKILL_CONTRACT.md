# OpenClaw Skill Contract (Tutti)

This document defines a stable first-pass contract for building an OpenClaw skill that orchestrates Tutti workspaces.

## Goals

- Keep the skill simple and deterministic.
- Use Tutti CLI for actions and `.tutti/state/*` for machine-readable reads.
- Avoid parsing terminal table output.

## Required Commands

- `tt up`
- `tt down`
- `tt status`
- `tt run`
- `tt verify`
- `tt peek`
- `tt doctor`

## Intent Mapping

- `launch_team`
  - Command: `tt up`
  - Success criteria: exit code `0`.
- `launch_agent`
  - Command: `tt up <agent>`
  - Success criteria: exit code `0`.
- `run_workflow`
  - Command: `tt run <workflow> [--agent <agent>] [--strict] [--json]`
  - Success criteria: exit code `0`.
- `verify_team`
  - Command: `tt verify [--workflow <name>] [--agent <agent>] [--strict] [--json]`
  - Success criteria:
    - strict mode: exit code `0`.
    - non-strict mode: exit code `0` (warnings allowed).
- `team_status`
  - Command: `tt status`
  - Machine read should prefer `.tutti/state/*.json`.
- `agent_output`
  - Command: `tt peek <agent> --lines <n>`
- `stop_agent`
  - Command: `tt down <agent>`
- `stop_team`
  - Command: `tt down`
- `read_verify_status`
  - Command: `tt verify --last --json`
  - Fallback file read: `.tutti/state/verify-last.json`

## Preflight Flow (Recommended)

Before launch/verify loops:

1. `tt doctor`
2. If non-zero, surface failing checks and stop.
3. If zero, continue with workflow execution.

For machine reads, prefer `tt doctor --json`.

## Machine State Reads

- Agent state directory: `.tutti/state/`
- Per-agent file: `.tutti/state/{agent}.json`
- Verify summary: `.tutti/state/verify-last.json`
- Automation history: `.tutti/state/automation-runs.jsonl`

## Failure Handling

- Non-zero from `tt run`, `tt verify --strict`, `tt down`, `tt up`: treat as action failure.
- For non-strict `tt verify`, inspect output + `verify-last.json` to detect warnings.
- If `tt doctor` fails, treat as environment/config precondition failure.

## Versioning

This contract is additive-first. New intents can be introduced without breaking existing mappings.
