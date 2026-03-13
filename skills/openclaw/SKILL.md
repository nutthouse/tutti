# Tutti OpenClaw Skill (Starter)

Use this skill to orchestrate a Tutti workspace from another agent loop.

Reference wrapper:
- `integrations/openclaw/tutti_openclaw.py`
- `integrations/openclaw/action-contract.json`

If your `tt` binary is older than repo `main`, invoke the wrapper with:
- `--tt-bin "cargo run --quiet --"`

## Trigger

Use when asked to launch/monitor/verify/stop a multi-agent workspace managed by Tutti.

## Preconditions

1. Run from workspace root (`tutti.toml` present).
2. Run `tt doctor` before launching long workflows.
3. Prefer `.tutti/state/*.json` and `tt verify --last --json` for verification reads.

## Intent Mapping

- `launch_team`: `tt up`
- `launch_agent`: `tt up <agent>`
- `run_workflow`: `tt run <workflow> [--agent <agent>] [--strict] [--json]`
- `list_workflows`: `tt run --list --json`
- `plan_workflow`: `tt run <workflow> --dry-run --json`
- `verify_team`: `tt verify [--workflow <name>] [--agent <agent>] [--strict] [--json]`
- `generate_handoff`: `tt handoff generate <agent> --json`
- `list_handoffs`: `tt handoff list --json [--agent <agent>]`
- `apply_handoff`: `tt handoff apply <agent> [--packet <path>]`
- `team_status`: `tt status`
- `agent_output`: `tt peek <agent> --lines <n>`
- `stop_agent`: `tt down <agent>`
- `stop_team`: `tt down`
- `read_verify_status`: `tt verify --last --json` (fallback: `.tutti/state/verify-last.json`)

## Execution Pattern

1. Preflight:
   - `python3 integrations/openclaw/tutti_openclaw.py doctor_check`
   - stop if non-zero and report failures.
2. Launch:
   - `... launch_team` or `... launch_agent <agent>`.
3. Work:
   - `... list_workflows`, then `... run_workflow <workflow>`.
4. Verify:
   - `... verify_team --strict` for gate-style checks.
5. Stop:
   - `... stop_team` or `... stop_agent <agent>`.

## Failure Policy

- Non-zero `tt` exit: surface command + stderr summary.
- `tt verify` non-strict warnings: report as warning, include `tt verify --last --json` fields.
- Missing state file: treat as transient for up to 3 short retries.

## Output Contract (Recommended)

Return:
- action performed
- command used
- exit status
- brief evidence (one or two key lines)
- next recommended action

For full integration details, see:
- `docs/AGENT_INTEGRATION_CONTRACT.md`
- `docs/OPENCLAW_SKILL_CONTRACT.md`
