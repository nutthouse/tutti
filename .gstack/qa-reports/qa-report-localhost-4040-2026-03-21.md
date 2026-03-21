# QA Report — tutti dashboard (Phase 1b wiring)

**URL:** http://localhost:4040
**Date:** 2026-03-21
**Branch:** feat/phase-1b-dashboard-wiring
**Tier:** Standard (diff-aware)
**Duration:** ~5 min
**Pages tested:** 1 (dashboard SPA)
**Framework:** Vanilla JS SPA served by Rust (rust-embed)

## Health Score

| Category | Score | Weight | Weighted |
|----------|-------|--------|----------|
| Console | 100 | 15% | 15.0 |
| Links | 100 | 10% | 10.0 |
| Visual | 100 | 10% | 10.0 |
| Functional | 92 | 20% | 18.4 |
| UX | 100 | 15% | 15.0 |
| Performance | 100 | 10% | 10.0 |
| Content | 100 | 5% | 5.0 |
| Accessibility | 100 | 15% | 15.0 |
| **Total** | | | **98.4** |

## Issues Found: 1

### ISSUE-001 — HUD shows phantom run count from orphan reconstructed runs
- **Severity:** Medium
- **Category:** Functional
- **Fix Status:** verified
- **Commit:** 12260d0

**Description:** On page load, the `reconstructRuns()` function fetches `/v1/events` and replays workflow events. Since the API only returns the last ~50 events, `workflow.started` events for old runs get reconstructed without their matching `workflow.completed`/`workflow.failed` events, creating zombie runs with status "running". The HUD then displays "3 runs" (or more) even though no runs are active.

**Root cause:** The `/v1/events` pagination window is smaller than the full event history. Runs that started and completed outside the window get their `started` event replayed but not their terminal event.

**Fix:** After reconstruction completes, prune any runs still marked "running" that are older than 30 minutes (well beyond any reasonable active run).

**Evidence:**
- Before: HUD showed "3 runs" with all agents stopped
- After: HUD shows no run count, clean state

## Summary

- Total issues found: 1
- Fixes applied: 1 (verified)
- Health score: 98.4 → 98.4 (fix was functional, not a regression)

**Tested features:**
- Pipeline stage cards render correctly with all 5 stages
- Detail drawer opens/closes on stage click with agent metadata and filtered events
- Dispatch panel opens on "+ RUN" click, loads workflows from API
- Mobile responsive layout (375x812) — vertical pipeline stack, all elements accessible
- SSE connection indicator (green dot)
- Historical event reconstruction populates timeline
- No console errors

**PR summary:** QA found 1 issue (orphan run reconstruction), fixed it, health score 98.4.
