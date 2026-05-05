# Tutti Activation Telemetry Plan — 2026-05-05

## Goal
Define the minimum activation signal for Tutti without adding analytics bloat.

## Recommendation
Use the existing control-plane event stream and run ledger.

Do not build a separate telemetry system for this sprint.

## Activation definition
A workspace is **activated** when it reaches its **first successful orchestrated workflow run**.

Recommended exact signal:
- first `workflow.completed` event where:
  - the run belongs to a real workspace
  - the workflow is user-triggered or first-run guided
  - the run result is successful

## Activation funnel for this sprint
Track only these milestones:
1. `tt init` completed
2. `tt up` succeeded
3. first workflow started (`workflow.started`)
4. first workflow completed successfully (`workflow.completed`)

That is enough to answer:
- are installs reaching first value?
- where are they dropping?
- is onboarding improving activation?

## What already exists
Current Tutti already has:
- persisted control events in `.tutti/state/events.jsonl`
- workflow lifecycle events including `workflow.started` and `workflow.completed`
- run records and local API endpoints for runs/events
- SSE/event stream exposure via `/v1/events` and `/v1/events/stream`

## Sprint scope for #25
For this sprint, keep it narrow:
- document the activation definition
- ensure the docs point to the existing event sources
- if needed later, add one lightweight helper command or doc example for querying first successful run

## Explicit non-goals
Not this sprint:
- third-party product analytics
- cohort dashboards
- hosted telemetry backend
- deep retention instrumentation
- broad event schema redesign

## Suggested next implementation slice
If we want one concrete engineering follow-up after this doc:
- add a small operator-facing command or example script that answers:
  - "Has this workspace activated yet?"
  - "When was the first successful workflow run?"

## Why this is the right cut
Tutti already emits the important events.
The problem is not missing telemetry plumbing.
The problem is missing activation definition and operator-facing usage of the data already there.
