# Changelog

## [0.7.0] - 2026-03-22

### Added
- **Artifact Pipeline**: Prompt steps can now capture and pass artifacts between
  workflow stages. New `artifact_glob` and `artifact_name` fields on prompt steps
  enable glob-based artifact discovery after a prompt step completes, with pre-step
  snapshot to prevent race conditions with concurrent runs.
- **inject_files template expansion**: `inject_files` now supports
  `{{output.artifact_name.path}}` template references, allowing artifacts from prior
  steps to be automatically injected into downstream agents' worktrees.
- **Dashboard artifact labels**: Flow connectors on the factory floor show
  artifact names flowing between pipeline stages during active workflow runs.
- **Dry-run artifact validation**: `tt run --dry-run` validates gstack-slug
  availability when artifact_glob uses `{slug}` interpolation, catching
  configuration errors before workflow execution.
- **Variable interpolation in globs**: `{slug}`, `{workspace}`, `{agent}`, and
  `~` are expanded in artifact_glob patterns at runtime.

## Unreleased

### Added
- **Agent Focus Mode**: click any stage card on the factory floor to zoom
  into a full-screen agent view with live terminal output, token usage
  stats, git diff of changes, context health %, and a prompt input bar.
  The Factorio zoom-in — see the gears turning inside each machine.
- **`GET /v1/agents/{ws}/{agent}/focus`**: combined endpoint returning
  terminal capture, usage stats, diff, and context % in a single fast
  response (~35ms). Usage scan separated to a slower cadence to keep
  terminal updates instant.
- **Workflow step events**: `workflow.step.started`, `.completed`, and
  `.failed` SSE events with step index, agent, type, and duration.
- **Event file rotation**: `events.jsonl` auto-rotates at 5,000 events,
  archiving old entries to prevent unbounded growth.
- **Run tracking on factory floor**: animated work-item dots flow through
  pipeline stages during workflow runs, with marching-dash connectors.
- **Step timeline in detail drawer**: click a run dot to see per-step
  progress with status badges, durations, and failure messages.
- **Dispatch panel**: trigger workflow runs directly from the dashboard
  with workflow selector and optional issue number input.
- **Historical run reconstruction**: page load replays `/v1/events` to
  rebuild in-progress runs. Orphan runs pruned after 30 minutes.

### Fixed
- Dispatch panel checked `json.status` instead of `json.ok` (response
  shape mismatch).
- `implement_code` startup grace increased to 120s to prevent premature
  idle detection during agent exploration phase.

## 0.5.0 - 2026-03-20

Remote access, factory-floor dashboard, and operational diagnostics release.
Operators can now monitor agents through a real-time web dashboard, expose
`tt serve` to remote networks, manage SSH tunnels with `tt remote`, and
triage run failures via stable, categorised root causes.

### Added
- **Factory-floor dashboard**: embedded SPA served at the control API root
  when `observe.dashboard` is enabled. Renders agents as pipeline stages
  with flow connectors, state-driven visual treatments
  (working/idle/stopped/blocked/auth-fail), pulse animations for active
  stages, and a throughput HUD. Connects via `/v1/health` snapshot and
  `/v1/events/stream` SSE for live updates. Mobile-responsive (#84).
- **`tt remote attach <host>`**: open an SSH port-forward tunnel to a remote
  tutti host, persist the entry in global config, and print connection
  instructions (#85).
- **`tt remote status`**: list registered remotes with live reachability probes.
- **`tt serve --remote`**: bind to all interfaces (`0.0.0.0`) with
  auto-generated bearer-token authentication for remote agent access (#82, #88).
- **`tt serve --bind <addr>`**: custom bind address override for the control
  API.
- **Stable failure taxonomy** (`FailureCategory`): operator-facing enum
  (`Routing`, `Runtime`, `Permission`, `Review`, `Provider`, `Timeout`,
  `Orchestration`, `Unknown`) for actionable run summaries (#76, #87).
- **Step timeline persistence** (`StepTimeline`): records `started_at`,
  `finished_at`, `duration_secs`, and `retry_count` per workflow step with
  cursor-based pagination hardening (#75, #81).
- `GlobalConfig` gains optional `[serve]` table and `[[remote]]` array.

### Fixed
- Config load failures now emit a warning instead of hard-erroring, preventing
  crashes when `~/.config/tutti/config.toml` is absent or malformed.
- Health endpoint path corrected (was returning 404).

## 0.4.0 - 2026-03-20

First fully unattended dogfood milestone. `tt run sdlc-auto --strict` completes
all 23 steps end-to-end without operator intervention.

### Added
- **PR review loop + merge gate** (`tt land`): enforce CI-green + CodeRabbit-approved
  before landing agent branches. Configurable via `TT_ENFORCE_MERGE_GATE` env var.
- **`tt runs`** subcommand for listing workflow run history from the SDLC ledger.
- **Health-state classification** in `status`/`watch`: unified `HealthState` enum
  (Working, Idle, Stalled, AuthFailed, RateLimited, ProviderDown, Stopped, Unknown)
  derived from health probe data (#64, #19).
- **`verify_clean_baseline.sh`**: asserts HEAD == base_sha and clean porcelain after
  branch creation, catching worktree contamination before implementation starts.
- **`--milestone` flag** on `tt issue-claim acquire` for filtering by GitHub milestone.
- **Worktree preservation guard**: agent worktrees on `auto/issue-*` branches are
  preserved across `tt up` instead of being reset, preventing mid-workflow state loss.
- Config validation rejects `wait_timeout_secs`/`startup_grace_secs` when
  `wait_for_idle` is false.

### Fixed
- **False idle detection between Claude Code tool calls**: completion signal now
  requires idle stability window before firing, preventing premature completion
  when the prompt bar flashes briefly between tool invocations.
- **Uninitialized agent sessions**: `ensure_running` and all auto-start paths now
  wait for the agent to reach idle/ready state via `start_and_wait_ready()` instead
  of a fixed 3-second sleep.
- **Dirty worktree contamination**: `create_issue_branch.sh` pre-cleans, post-cleans
  with `git clean -ffdx`, and asserts clean baseline before proceeding.
- **CodeRabbit timeout handling**: `wait_coderabbit.sh` treats timeout as soft exit,
  detects rate-limit and kickoff-only comments, and passes JSON via env var instead
  of argv (fixing arg-length and stdin-conflict issues).
- **Stale output_json**: prompt steps now delete leftover output files before sending
  to prevent wait helpers from short-circuiting on previous attempt artifacts.
- **Merge gate ordering**: gate now runs after `commit_wip_if_needed` and push so it
  evaluates the final branch state.
- **Hardcoded "codex" in retry**: implement_code retry path uses the agent's actual
  runtime and configured timeout instead of fixed values.
- Permission suggestions no longer append wildcard `*`; single-token commands now
  return a suggestion instead of `None`.
- `git_current_branch` handles detached HEAD by returning `None`.
- Git subprocess calls in automation scripts include 60-second timeouts.
- Explicit UTF-8 encoding on all automation script file I/O.

### Changed
- `SdlcRunLedgerRecord` gains `issue_title`, `branch`, and `failure_message` fields.
- `wait_for_agent_idle` accepts `startup_grace` parameter across all call sites.
- `stop_idle_agents` step added before `final_review` to free `max_concurrent` slots.
- Claude Code runtime detection expanded: `Searching for`, `Unravelling`, `(thinking)`
  working patterns; `don't ask on`, `shift+tab to cycle` idle/completion patterns.

## 0.3.0 - 2026-03-19

Highlights:
- Added startup grace window for `wait_for_agent_idle` to prevent false
  idle/completion detection on brand-new prompt steps (#67). Fresh `sdlc-auto`
  runs no longer fail at early steps because the agent is still visibly thinking.
- Added per-agent persistent memory (`memory` config field) for long-lived
  context across sessions (#62, #63).
- Added merge gate enforcement for `tt land` steps: when `TT_ENFORCE_MERGE_GATE=1`,
  land verifies required CI checks are green and all PR review threads are resolved.
  Automation land steps enable this by default (#59).
- Added `tt permissions suggest <workflow>` for batch pre-approval of blocked
  commands across an entire workflow (#53).
- Added permission fix hints in blocked-command error messages (#57).
- Added orchestration state machine and run ledger for deterministic workflow
  resume and PR-summary formatting (#54, #55).
- Added explicit resume baseline from SDLC ledger (#58).
- Added issue claim lease with auto-release on failed/aborted runs (#33).
- Added Codex SDLC orchestration framework with helper scripts, CI dispatch,
  and multi-agent topology docs (#26, #31, #32).
- Hardened runtime detection with diagnostics and fixtures (#14).
- Added resume intent log and compensator preflight (#13).
- Fixed automation branch alignment with agent worktrees (#61).
- Fixed ratatui 0.29 → 0.30 to resolve lru Stacked Borrows vulnerability (#43).
- Fixed Codex runtime prompt passing (positional instead of `--prompt`) (#32).
- Added `docs/pr-review-loop.md` documenting the canonical PR review/merge loop.
- Added VERSIONING.md policy and PR versioning checklist (#42).

Release impact:
- **Minor version bump** (0.2.0 → 0.3.0) per VERSIONING.md: new user-visible
  capabilities (persistent memory, permissions suggest, merge gate, orchestration
  state machine) and autonomy milestone (deterministic fresh-run startup).
- Existing workflows remain backward-compatible. Merge gate is opt-in via env var
  for manual use; automation land steps enable it automatically.
- Permissions `--audit` output now emits tighter suggested rules (exact prefix
  instead of trailing wildcard).

## 0.2.0 - 2026-03-14

Highlights:
- Added native OpenClaw runtime support across launch/status/send flows.
- Added deterministic `tt send --wait` completion with structured outcomes and runtime signal preference.
- Added local control API under `/v1/*` with read endpoints, action endpoints, idempotency keys, and SSE event stream.
- Added workflow scheduling, workflow-complete hooks, structured step outputs, and workflow resume checkpoints.
- Added launch-policy parity hardening with constrained-mode shell shims and persisted policy decision logs.
- Added resilience automation:
  - launch-time profile rotation,
  - `tt serve` runtime recovery on auth/rate-limit/provider-down triggers,
  - `tt watch` runtime recovery on auth/rate-limit/provider-down triggers,
  - recovery control events (`agent.recovery_*`).
- Added API-only budget guardrails with threshold/block events and pre-exec enforcement on `up/send/run/verify`.
- Expanded `tt doctor --strict` and CI smoke coverage (workflow smoke plus recovery-trigger event smoke).

Notes:
- This release is focused on autonomous multi-agent orchestration hardening and control-plane completeness.
- Existing workflows remain backward-compatible; new behavior is primarily additive and opt-in via config.
