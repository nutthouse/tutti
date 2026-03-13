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
3. `.tutti/state/health/*.json` plus automation state files for machine-oriented control loops.
4. `.tutti/logs/*.log` (when enabled) for historical output analysis.
5. `tt permissions check ... --json` when integrations want to preflight local command safety policy.
6. `tt doctor --json` (or `tt doctor --strict` for CI gates) before long-running automation to validate workspace prerequisites, including running-agent auth health checks.

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
  - read `.tutti/state/health/{agent}.json` or call `tt health --json`

Recommended machine pattern:
- Read all state files from `.tutti/state/`.
- Use `status`, `started_at`, `stopped_at`, and `session_name` for orchestration decisions.

### 4) Inspect output

- Snapshot:
  - `tt peek <agent> --lines 100`
- Interactive:
  - `tt attach <agent>`
- One-off prompt with completion wait:
  - `tt send <agent> --wait --timeout-secs 900 "..."` (optionally tune `--idle-stable-secs`)
  - `tt send <agent> --auto_up --wait --output "..."` for auto-start + captured pane delta

Use `peek` for automation. Use `attach` for operator handoff.

### 4.5) Inspect and land code changes

- Inspect worktree + branch changes:
  - `tt diff <agent>`
- Land agent commits into current branch:
  - `tt land <agent>`
- Force land even when local branch has tracked changes:
  - `tt land <agent> --force`
  - Force mode performs a temporary stash/pop around landing to avoid tracked-dirty blocks.
- Push and open PR from agent branch:
  - `tt land <agent> --pr`
- Send review packet to reviewer agent:
  - `tt review <agent> [--reviewer <agent>]`

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
- Resume a failed workflow run:
  - `tt run --resume <run-id>`
- Dry-run resolution:
  - `tt run <workflow> --dry-run`
- Run verification workflow:
  - `tt verify`
  - `tt verify --workflow <name>`
- Daemon-backed scheduling + health endpoint:
  - `tt serve --port 4040`
  - `GET /v1/health`
  - `GET /v1/health/{workspace}/{agent}`
  - `GET /v1/status`, `GET /v1/voices`, `GET /v1/workflows`, `GET /v1/runs`, `GET /v1/logs`, `GET /v1/handoffs`, `GET /v1/policy-decisions`, `GET /v1/events`
  - Events cursor/filter: `GET /v1/events?cursor=<RFC3339 timestamp>&workspace=<name>`
  - Event stream (SSE): `GET /v1/events/stream?cursor=<RFC3339 timestamp>&workspace=<name>`
  - Stream event types include: `agent.started`, `agent.stopped`, `agent.working`, `agent.idle`, `agent.auth_failed`, `workflow.started`, `workflow.completed`, `workflow.failed`, `handoff.generated`, `handoff.applied`
  - Budget control events may be emitted when configured: `budget.threshold`, `budget.blocked`
  - `POST /v1/actions/up|down|send|run|verify|review|land`
  - API envelope: `ok/action/error/data`
  - `send` action includes structured completion payload in `data.send` (`waited`, `completion_source`, `captured_output`)
  - Mutating idempotency: `Idempotency-Key` header (or `idempotency_key` request field)
- Generate handoff packet:
  - `tt handoff generate <agent>`
- Apply latest handoff packet:
  - `tt handoff apply <agent>`

Hook behavior in v1:
- `agent_stop` hooks fire from explicit stop paths (`tt down`, `tt down --all`).
- `workflow_complete` hooks fire for all workflow executions (`run`, `verify`, `hook_agent_stop`, `observe_cycle`) with source/name filters.
- Hook defaults are fail-open unless configured fail-closed.
- Workflow step types:
  - `prompt`, `command`, `ensure_running`, `workflow` (nested), `review`, `land`
  - `command` supports `cwd` plus optional `subdir` (workspace-relative) for deterministic per-service execution paths.
  - `depends_on = [<step-number>, ...]` enables dependency-aware execution; independent `ensure_running`/`review`/`land` steps run in parallel waves.
  - `review`/`land` steps auto-start required sessions when missing.
  - `prompt` supports `inject_files` to copy workspace-relative files into the target agent worktree before send.
- Workflow auto-reclaim:
  - Agents with `persistent = false` that were not running at workflow start are auto-stopped at workflow end.

### 7) Command permission policy (optional)

- Check if a command is currently allowed by global policy:
  - `tt permissions check <command...> --json`
- Export Claude-compatible settings scaffold from policy:
  - `tt permissions export --runtime claude`

Policy notes:
- Policy is opt-in via `[permissions]` in `~/.config/tutti/config.toml`.
- When policy is absent, Tutti allows commands and reports that policy is not configured.
- Policy entries can include shell command prefixes and Claude tool names (`Read`, `Edit`, `Write`, etc.).
- `tt up` launch integration:
  - Claude constrained mode: auto-generates a runtime settings file and launches with `--permission-mode dontAsk`.
  - Codex/OpenClaw constrained mode: best-effort policy guidance (Codex also uses `-a never -s workspace-write`).

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
- `.tutti/state/health/{agent}.json`: latest probe-based health snapshot for each agent.
- `.tutti/state/automation-runs.jsonl`: append-only execution records (workflow/hook runs).
- `.tutti/state/events.jsonl`: append-only control-plane events (`agent.*`, `workflow.*`, `handoff.*`, `budget.*`).
- `.tutti/state/policy-decisions.jsonl`: append-only launch policy decisions (`action`, `mode`, `policy`, `enforcement`, `decision`).
- `.tutti/state/verify-last.json`: last verification summary (`workflow_name`, `success`, `failed_steps`, `strict`, `agent_scope`).
- `.tutti/state/scheduler-last-runs.json`: scheduler fire timestamps per workspace/workflow key.
- `.tutti/state/workflow-outputs/<run-id>/<step-id>.json`: canonical structured step outputs.
- `.tutti/state/workflow-checkpoints/<run-id>.json`: execution checkpoint used by `tt run --resume`.

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
  - `inspect_agent_diff` -> `tt diff <agent>`
  - `land_agent_changes` -> `tt land <agent>`
  - `force_land_agent_changes` -> `tt land <agent> --force`
  - `open_agent_pr` -> `tt land <agent> --pr`
  - `review_agent_changes` -> `tt review <agent>`
  - `run_workflow` -> `tt run <workflow> --json`
  - `verify_team` -> `tt verify --json`
  - `team_status` -> `tt status` or `tt health --json`
  - `agent_output` -> `tt peek <agent> --lines N`
  - `send_and_wait` -> `tt send <agent> --wait --timeout-secs <N> "..."`
  - `serve_control_plane` -> `tt serve --port <N>`
  - `health_http` -> `GET /v1/health`
  - `read_verify_status` -> `tt verify --last --json` (or read `.tutti/state/verify-last.json`)
  - `stop_agent` -> `tt down <agent>`
  - `stop_team` -> `tt down`
- Prefer reading `.tutti/state/` for control logic over parsing command tables.
- Keep command execution local to the workspace root.

## Versioning

This is an evolving contract. Backward-compatible additions are expected.
If a future change requires a breaking integration update, it should be called out in release notes and this document.
