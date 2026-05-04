# PR Review Loop ("Choir" Reproducibility)

This is the exact merge loop for the current GitHub + CodeRabbit Tutti automation. Treat CodeRabbit as the first review adapter, not the only possible reviewer.

## Objective

Make PR handling deterministic:
1. Resolve configured review threads.
2. Let the review adapter re-check after push.
3. Wait for required checks to be green.
4. Confirm approvals are present.
5. Merge/land only after gates pass.

## Canonical loop

1. Open or update PR from the issue branch.
2. Wait for configured review-adapter feedback. For CodeRabbit today, use `wait_coderabbit.sh` plus `collect_coderabbit_feedback.sh`.
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

## Adapter model

The durable gate is "review feedback resolved and required checks green." CodeRabbit is one implementation:

- CodeRabbit thread resolution -> review feedback resolved
- Human approval -> review feedback resolved
- Claude/Codex reviewer packet -> review feedback resolved when the reviewer signs off
- CI/static analysis -> required checks green

Keep adapter-specific polling and comment resolution in scripts. Keep the merge gate generic at the workflow level.

## Notes

- CodeRabbit re-review is push-triggered; do not manually force review unless required.
- Keep this gate in automation even if humans occasionally merge manually.
- If a gate fails, fix the PR state and rerun the land step.
