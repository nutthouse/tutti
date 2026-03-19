# Changelog

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
