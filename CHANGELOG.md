# Changelog

## 0.4.0 - 2026-03-18

Highlights:
- Added deterministic SDLC planner-to-implementer handoff via prompt-step `output_json` artifacts such as `.tutti/state/auto/plan_issue.json`.
- Workflow prompt outputs written inside agent worktrees are now mirrored back into workspace state so downstream steps can inject them reliably.
- Idle completion detection now requires a fresh runtime completion signal, avoiding false advances caused by stale footer UI or redraw noise.

Notes:
- This release strengthens unattended SDLC workflow execution for the built-in Codex/Claude release-train example.
- Existing command-step `output_json` behavior is unchanged; prompt steps now participate in the same artifact-driven workflow pattern.

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
