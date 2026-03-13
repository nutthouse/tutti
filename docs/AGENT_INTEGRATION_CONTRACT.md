# Tutti Agent Integration Contract

This document defines a practical contract for integrating external agents/orchestrators (for example OpenClaw) with Tutti.

Scope of this contract:
- How an external agent should launch, monitor, and control Tutti-managed sessions.
- Which interfaces are stable enough to consume now.
- What to avoid (fragile parsing patterns).

## Goals

- Let an external agent treat Tutti as a local control plane.
- Keep integrations robust even when terminal UI output changes.
- Standardize lifecycle behavior across tools.

## Non-goals

- This is not a network API spec.
- This is not a replacement for `tutti.toml` ownership/scope design.

## Integration Surfaces

Use these surfaces, in this order of preference:

1. Tutti CLI commands for lifecycle actions.
2. `.tutti/state/*.json` files for machine-readable per-agent runtime state.
3. `.tutti/state/automation-runs.jsonl` and `.tutti/state/verify-last.json` for automation outcomes.
4. `.tutti/logs/*.log` (when enabled) for historical output analysis.
5. `tt permissions check ... --json` when integrations want to preflight local command safety policy.
6. `tt doctor --json` before long-running automation to validate workspace prerequisites.

Avoid:
- Parsing pretty tables from `tt status` as your primary machine interface.
- Depending on ANSI color text.

## Required Environment Assumptions

- `tt` is installed and on `PATH`.
- `tmux` is installed and available.
- Workspace has a valid `tutti.toml`.
- Global config exists (or `tt init` has created defaults).
- The integration process has read/write access to workspace `.tutti/`.

## Canonical Lifecycle

### 1) Workspace resolution

- Preferred: run inside the target workspace directory and use local commands (`tt up`, `tt status`).
- Cross-workspace operations are supported via `--workspace` and `workspace/agent` references.

### 2) Launch

- Start all configured agents:
  - `tt up`
- Override launch autonomy mode:
  - `tt up --mode safe`
  - `tt up --mode auto`
  - `tt up --mode unattended --policy constrained|bypass`
- Start a single agent:
  - `tt up <agent>`

Expected behavior:
- Idempotent for already-running sessions (skips instead of hard-failing).
- Non-zero exit when runtime/config constraints fail.
- In constrained non-interactive mode, `tt up` fails fast when `[permissions]` is missing.

### 3) Observe

- Human-oriented status:
  - `tt status`
  - `tt watch`
- Machine-oriented state:
  - read `.tutti/state/{agent}.json`

Recommended machine pattern:
- Read all state files from `.tutti/state/`.
- Use `status`, `started_at`, `stopped_at`, and `session_name` for orchestration decisions.

### 4) Inspect output

- Snapshot:
  - `tt peek <agent> --lines 100`
- Interactive:
  - `tt attach <agent>`

Use `peek` for automation. Use `attach` for operator handoff.

### 5) Stop

- Stop one:
  - `tt down <agent>`
- Stop all in workspace:
  - `tt down`
- Stop all registered workspaces:
  - `tt down --all`

### 6) Workflow automation

- Run a named workflow:
  - `tt run <workflow>`
- Dry-run resolution:
  - `tt run <workflow> --dry-run`
- Run verification workflow:
  - `tt verify`
  - `tt verify --workflow <name>`
- Generate handoff packet:
  - `tt handoff generate <agent>`
- Apply latest handoff packet:
  - `tt handoff apply <agent>`

Hook behavior in v1:
- `agent_stop` hooks fire from explicit stop paths (`tt down`, `tt down --all`).
- Hook defaults are fail-open unless configured fail-closed.

### 7) Command permission policy (optional)

- Check if a command is currently allowed by global policy:
  - `tt permissions check <command...> --json`
- Export Claude-compatible settings scaffold from policy:
  - `tt permissions export --runtime claude`

Policy notes:
- Policy is opt-in via `[permissions]` in `~/.config/tutti/config.toml`.
- When policy is absent, Tutti allows commands and reports that policy is not configured.
- `tt up` launch integration:
  - Claude constrained mode: auto-generates a runtime settings file and launches with `--permission-mode dontAsk`.
  - Codex constrained mode: uses non-interactive flags (`-a never -s workspace-write`) plus policy guidance prompt (best-effort).

## Machine-Readable State Contract

State files are written under:
- `.tutti/state/{agent}.json`

Current JSON shape:

```json
{
  "name": "backend",
  "runtime": "claude-code",
  "session_name": "tutti-my-project-backend",
  "worktree_path": "/abs/path/or/null",
  "branch": "tutti/backend/or/null",
  "status": "Working|Idle|Stopped|Auth Failed: ...|Unknown|Errored",
  "started_at": "RFC3339 timestamp",
  "stopped_at": "RFC3339 timestamp or null"
}
```

Integration guidance:
- Treat unknown `status` values as non-fatal; fallback to `Unknown`.
- Do not assume strict enum-only values (auth failure strings may include details).
- Use timestamps to detect stale/crashed behavior heuristically.

Automation state files:
- `.tutti/state/automation-runs.jsonl`: append-only execution records (workflow/hook runs).
- `.tutti/state/verify-last.json`: last verification summary (`workflow_name`, `success`, `failed_steps`, `strict`, `agent_scope`).

## Failure Handling Contract

External agents should apply this retry policy:

1. Config/runtime validation failures:
   - Do not retry immediately.
   - Surface actionable error and require operator/config fix.
2. Transient session/pane read errors:
   - Retry with short backoff (for example 250ms, 500ms, 1s).
3. Auth failures:
   - Treat as blocked state; prompt re-authentication flow.
   - Check `.tutti/handoffs/*-emergency-*.md` for captured context when present.

## Concurrency and Safety

- Prefer one orchestrator process per workspace.
- If multiple tools interact with the same workspace, coordinate through state files and command idempotency.
- Never mutate `.tutti/state/*.json` directly; use Tutti commands to drive state transitions.

## Recommended Agent Loop (Pseudo-flow)

1. Validate workspace (`tutti.toml` present, `tt status` callable).
2. Launch required agents (`tt up` or targeted `tt up <agent>`).
3. Poll `.tutti/state/*.json` on interval.
4. For blocked/unknown agents, inspect via `tt peek`.
5. Escalate to operator or attach handoff (`tt attach`) when intervention is needed.
6. Stop with `tt down` when done.

## OpenClaw-Specific Notes

When building an OpenClaw skill/plugin:

- Canonical mapping reference: `docs/OPENCLAW_SKILL_CONTRACT.md`
- Reference wrapper implementation: `integrations/openclaw/README.md`

- Expose high-level intents mapped to Tutti commands:
  - `launch_team` -> `tt up`
  - `launch_agent` -> `tt up <agent>`
  - `run_workflow` -> `tt run <workflow> --json`
  - `verify_team` -> `tt verify --json`
  - `team_status` -> `tt status`
  - `agent_output` -> `tt peek <agent> --lines N`
  - `read_verify_status` -> `tt verify --last --json` (or read `.tutti/state/verify-last.json`)
  - `stop_agent` -> `tt down <agent>`
  - `stop_team` -> `tt down`
- Prefer reading `.tutti/state/` for control logic over parsing command tables.
- Keep command execution local to the workspace root.

## Versioning

This is an evolving contract. Backward-compatible additions are expected.
If a future change requires a breaking integration update, it should be called out in release notes and this document.
