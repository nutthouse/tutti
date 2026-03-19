# Changelog

## 0.3.1 - 2026-03-19

Highlights:
- Added `output_files` field for prompt workflow steps: declares expected output artifacts that must exist and be non-empty after step completion. Steps fail with a clear error if declared outputs are missing, enabling deterministic handoff validation between workflow agents.
- Updated SDLC example workflow (`tutti-codex-sdlc.toml`) to use `output_files` for planner→implementer handoff via `plan_issue.json`.

Release impact:
- New optional `output_files` array on prompt steps. Existing workflows without `output_files` are unaffected.
- `output_files` paths must be workspace-relative (validated at config load time).
- Steps declaring `output_files` will fail if any declared file is missing or empty after the step completes.

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
