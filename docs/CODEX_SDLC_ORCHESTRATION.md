# Codex SDLC Orchestration (Tutti-for-Tutti)

This framework automates the SDLC loop for Tutti using Codex agents:

1. Select issue from GitHub
2. Create issue branch
3. Implement with specialized agents (implementation, testing, docs/release)
4. Validate locally
5. Open PR
6. Wait for CodeRabbit review
7. Apply review fixes
8. Re-validate and update PR

## Specialized agent topology (recommended)

Use 6 focused agents:
- `planner` (Claude) — issue decomposition, risk/test/release planning
- `conductor` (Codex) — orchestration/handoffs only
- `implementer` (Codex) — code changes in `src/**`
- `tester` (Codex) — tests and validation ownership
- `docs-release` (Codex) — docs/changelog/version responsibilities
- `reviewer` (Codex) — strict release-readiness review

This keeps each agent independent and accountable to one concern while preserving Codex-heavy execution.

## Prerequisites

- `gh` authenticated with repo access
- `codex` CLI authenticated and available in PATH
- `git`, `python3` available
- Repo has labels for issue intake (default `agent-ops`)

## Test run first (required)

Before unattended automation, run a smoke workflow that:

- selects issue (`select_issue.sh`)
- creates branch (`create_issue_branch.sh`)
- runs validation (`cargo test --quiet`)
- does **not** open PR or push

Only after successful smoke, run full cycle.

## Example workflow file

Use `docs/examples/tutti-codex-sdlc.toml` as a starting point.

## Core scripts

- `scripts/automation/select_issue.sh`
- `scripts/automation/create_issue_branch.sh`
- `scripts/automation/wait_coderabbit.sh`
- `scripts/automation/collect_coderabbit_feedback.sh`

## Operational notes

- Keep branch naming deterministic: `auto/issue-<num>-<timestamp>`
- Run branch-creation commands in the implementer `agent_worktree` (not workspace root) to avoid dirtying the wrong checkout
- Treat `.tutti/state/auto/branch.json` as the source of truth and instruct every SDLC prompting step to commit/push to that branch explicitly
- Always include issue reference in commit and PR body
- Enforce docs/version updates in implementation prompt
- Require test pass before PR open and before merge/land
- If CodeRabbit fails, gather feedback and route to Codex fix step
- Follow the canonical PR reproducibility loop in `docs/pr-review-loop.md`
- Automation `land` steps enforce merge gate checks (required checks + resolved review threads)

## Suggested runbook

1. `tt run sdlc-smoke --strict`
2. Inspect logs/output artifacts under `.tutti/state/auto/`
3. `tt run sdlc-auto --strict`
4. Monitor with `tt watch` / `tt logs`
5. Land only after checks + review pass

## Safety

- Start with `fail_mode = "closed"` on command steps
- Keep PR creation and land as explicit steps (no hidden auto-merge)
- Keep approval checks in your reviewer/final summary prompts
- Keep merge gate enabled before `land` (required checks green + no unresolved review threads)
