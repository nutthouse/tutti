# Changelog

## 0.2.4 - 2026-03-18

Features:
- Added `tt permissions suggest <workflow>` subcommand for batch pre-approval of workflow command policies.
- `tt permissions check` now includes actionable allow-rule hints when a command is blocked by policy.
- `tt run --dry-run --json` now includes a literal `command` field in resolved execution plan output.
- Workflow `land` steps now enforce a GitHub merge gate — required checks must be green and all PR review threads resolved before landing.

Fixes:
- Fixed Codex runtime prompt dispatch to pass prompts positionally instead of the unsupported `--prompt` flag.
- Bumped ratatui 0.29 → 0.30 to resolve lru Stacked Borrows memory-safety vulnerability.
- Made wildcard hint helper internal (no user-facing change).

Notes:
- Release impact: `tt permissions check` output now includes hint text for blocked commands (additive, non-breaking). Workflow `land` steps that previously landed without checking GitHub status will now fail if checks are red or reviews are unresolved — operators relying on force-land behavior in automation should verify their CI pipelines pass before landing.

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
