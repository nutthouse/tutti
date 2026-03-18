# Changelog

## 0.4.0 - 2026-03-18

Highlights:
- Added per-agent persistent memory via `[[agent]].memory`, with workspace-relative validation and safe path checks.
- Claude Code agents now inject managed memory into agent-worktree `CLAUDE.md`; other runtimes receive the same memory prepended to the launch prompt.
- Hardened prompt delivery by sending multiline session input atomically through tmux bracketed paste and warning when `tt send` detects an exited or auth-failed runtime before dispatch.
- Expanded worktree lifecycle and tmux usability test coverage for unattended SDLC automation.

Notes:
- This release adds a new operator-facing configuration surface and long-lived agent behavior, so it is versioned as a minor release.
- Existing workspaces remain compatible; agents opt into memory explicitly via `memory = "..."`

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
