# Changelog

## 0.3.1 - 2026-03-18

Highlights:
- Preserved persistent agent memory for `claude-code` launches without a dedicated worktree by falling back to prompt injection instead of mutating the workspace root `CLAUDE.md`.
- Hardened memory path handling by rejecting symlinked memory sources and `CLAUDE.md` destinations that resolve outside the allowed workspace/worktree boundary.
- Fixed managed memory-section replacement so relaunches only remove the section after the injected start marker, avoiding accidental matches against unrelated end markers in `CLAUDE.md`.

Notes:
- This patch release is focused on launch reliability and filesystem safety for persistent agent memory.

## 0.3.0 - 2026-03-14

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
