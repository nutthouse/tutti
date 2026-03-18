# Changelog

## 0.2.4 - 2026-03-18

- Added actionable allow-rule hints in blocked-command error output so operators can quickly fix permission configs.
- Added PR merge gate for `tt land` steps: when `TT_ENFORCE_MERGE_GATE=1` is set, land fails closed unless required CI checks are green and all PR review threads are resolved.
- Added `docs/pr-review-loop.md` documenting the canonical PR review and merge loop for automation.

Release impact: PATCH — both features are opt-in and non-breaking. Existing `tt land` behavior is unchanged unless the merge gate env var is explicitly enabled. Permission hints are purely additive CLI output.

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
