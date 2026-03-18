# Changelog

## 0.3.1 - 2026-03-19

Highlights:
- Added `PlanArtifact` struct for deterministic planner-to-implementer handoff, with typed save/load helpers and validation.

Notes:
- Internal workflow improvement: the `plan_issue` step now produces a structured JSON artifact (issue number, scope summary, target files, implementation tasks, first slice) that downstream steps consume directly. No CLI or config changes.

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
