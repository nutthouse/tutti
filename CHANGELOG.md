# Changelog

## 0.3.1 - 2026-03-18

Highlights:
- Added structured workflow handoff artifacts for SDLC automation via step `id` plus `output_json`, including prompt-step outputs that resolve inside the target agent worktree.
- `tt run --dry-run --json` now exposes prompt-step `output_json` paths so planner-to-implementer handoff wiring can be validated before execution.
- Added regression coverage and updated orchestration examples/docs for planner artifact handoffs and implementer result artifacts.

Notes:
- This is a patch release focused on deterministic SDLC automation and release-ops ergonomics.
- No breaking CLI or config contract changes.

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
