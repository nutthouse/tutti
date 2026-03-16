# Changelog

## 0.2.4 - 2026-03-16

### Added
- Codex SDLC orchestration framework with specialized 6-agent topology (planner, conductor, implementer, tester, docs-release, reviewer).
- Automation helper scripts: `select_issue.sh`, `create_issue_branch.sh`, `wait_coderabbit.sh`, `collect_coderabbit_feedback.sh`.
- `workflow_dispatch` CI orchestrator for SDLC smoke and auto runs.
- Agent operations roadmap and telemetry plan (`docs/AGENT_OPS_ROADMAP.md`).
- Claude planner lane and no-commit guard to SDLC choir workflow.

### Fixed
- Codex runtime now passes prompts positionally instead of unsupported `--prompt` flag.
- Automation includes issue body in implementation prompts and fails fast when no changes are produced.
- CodeRabbit wait/collect scripts hardened for error handling and inline comment collection.
- SDLC orchestration flow hardened for reruns and worktree alignment.
- Duplicate issue preselection removed; `issue_label` wired through workflows.
- Missing plan-file dependency removed from SDLC cycle.
- GitHub Actions workflow permissions added for code scanning compliance.

### Changed
- Extracted SDLC inline Python blocks into standalone automation scripts.
- Implementer prompt now requires evidence of changes in output.

## 0.2.3 - 2026-03-15

### Added
- Runtime detection hardening with diagnostics and transcript fixtures (Issue #11).
- `tt detect` diagnostics output includes confidence scores and matched pattern labels.

## 0.2.2 - 2026-03-14

### Added
- Resume intent log and compensator preflight for workflow recovery (Issue #10).

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
