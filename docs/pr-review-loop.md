# PR Review Loop ("Choir" Reproducibility)

This is the exact merge loop for Tutti automation.

## Objective

Make PR handling deterministic:
1. Resolve CodeRabbit threads.
2. Let CodeRabbit auto re-review after push.
3. Wait for required checks to be green.
4. Confirm approvals are present.
5. Merge/land only after gates pass.

## Canonical loop

1. Open or update PR from the issue branch.
2. Wait for CodeRabbit feedback (`wait_and_collect_coderabbit.py`).
3. Apply actionable feedback and push.
4. Repeat until no unresolved feedback remains.
5. Run final validation (`cargo test --quiet` + reviewer packet).
6. **Before `land`/merge, enforce merge gate**:
   - Required checks must be green.
   - PR review threads must all be resolved (including CodeRabbit threads).
7. Merge/land.

## Enforcement

Automation land steps now run with:

- `TT_ENFORCE_MERGE_GATE=1`

When enabled, `tt land` fails closed if:
- no open PR exists for the landed branch,
- any required check is not green,
- any PR review thread remains unresolved.

## Notes

- CodeRabbit re-review is push-triggered; do not manually force review unless required.
- Keep this gate in automation even if humans occasionally merge manually.
- If a gate fails, fix the PR state and rerun the land step.
