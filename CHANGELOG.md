# Changelog

## 0.3.0 - 2026-03-17

Highlights:
- Added issue-claim lease workflows via `tt issue-claim acquire|heartbeat|release|sweep` for autonomous SDLC loops.
- Added `tt permissions suggest <workflow>` to pre-compute command allowlists before unattended runs.
- Added orchestration state machine and run-ledger foundations for deterministic recovery and resume visibility.
- Published `tutti` on crates.io and documented `cargo install tutti` in Quick Start.
- Updated the OpenClaw integration bundle to the v1.1.0 action contract, including `send_prompt` and `land_agent`.

Notes:
- This release focuses on operator-facing SDLC automation and release-channel packaging without changing existing workflow contracts.
- Follow-on work for first-class run and work-unit UX continues separately under the `v0.4.0` milestone.

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
