# Issue #28 kickoff — stale `CHANGES_REQUESTED` gate handling

Issue: https://github.com/nutthouse/tutti/issues/28

## Problem framing
Merge orchestration can deadlock when GitHub keeps `CHANGES_REQUESTED` from an older review even after follow-up commits and green checks.

## Focused implementation slices
1. **Readiness evaluator**
   - Add a helper that compares:
     - latest actionable review timestamp
     - latest head commit timestamp
     - required-checks all-green state
2. **Stale-review rule**
   - Mark stale when:
     - checks are green
     - latest commit is newer than latest actionable review
     - no newer actionable comments/reviews exist
3. **Orchestrator handling**
   - Trigger one re-review request/ping path
   - Add bounded wait + poll loop
   - Terminal fallback: `needs-human-unblock`
4. **Operator-visible output**
   - PR summary line includes stale-review diagnosis and chosen action.

## Acceptance mapping
- Distinguish unresolved feedback vs stale review lock ✅
- Terminal status includes merged/needs-human-unblock ✅
- Deterministic summary comment path ✅

## Next code change in this branch
- Implement readiness decision enum + tests for stale/non-stale scenarios.
