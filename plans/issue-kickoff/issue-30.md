# Issue #30 kickoff — orchestration state machine + run ledger

Issue: https://github.com/nutthouse/tutti/issues/30

## Problem framing
Recovery is currently ad-hoc. We need one canonical SDLC state record so interrupted runs can resume deterministically.

## Focused implementation slices
1. **State model**
   - Add explicit states:
     `selected -> branched -> implemented -> tested -> docs -> pr_open -> reviewed -> ready_to_merge -> merged`
   - Define transition guards + failure reasons.
2. **Persistent run ledger**
   - Store `run_id`, issue/PR linkage, timestamps, actor, from_state/to_state, reason.
   - Append-only transitions with current-state projection.
3. **Recovery command path**
   - Resume from last valid transition rather than inferred filesystem state.
   - Detect and report invalid transition gaps.
4. **Reporting integration**
   - Generate PR summary/status directly from ledger transitions.

## Acceptance mapping
- Single canonical run state record per SDLC run ✅
- Recovery resumes from validated last state ✅
- PR summary derives from ledger ✅

## Next code change in this branch
- Land state enum + transition validator + initial persistence schema tests.
