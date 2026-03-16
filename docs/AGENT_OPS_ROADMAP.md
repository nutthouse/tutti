# Tutti Agent Operations Roadmap (Execution Plan)

## Goal
Position Tutti as the agent operations layer (not just a coding assistant) by closing reliability, governance, observability, and adoption gaps.

## Priorities

### P0 — Foundation (must ship first)
1. **Identity + Auth + RBAC**
   - User/org/workspace identities
   - API keys and role-based permissions
   - Audit trail for sensitive actions

2. **Production Control Plane Hardening**
   - Stable API contracts
   - Durable event stream semantics
   - Idempotency for mutating endpoints
   - Queryable run/event history

3. **Reliability Primitives**
   - Retry policies + backoff
   - Checkpoint/resume guarantees
   - Dead-letter handling for failed actions
   - Circuit breakers / provider failover

4. **Approval and Guardrail Workflows**
   - Human approval gates for risky actions
   - Policy-as-code enforcement
   - Exception handling with traceability

5. **Operator Observability**
   - Per-run timeline and failure stages
   - Root cause attribution (routing/tool/model/policy)
   - SLO dashboards and alerting hooks

### P1 — Scale and Governance
6. **Cost Governance**
   - Workspace/agent budgets
   - Forecasting and anomaly detection
   - Hard-stop + escalation behavior

7. **Deployment and Operations Story**
   - Managed and self-hosted deployment paths
   - Secrets/backup/migration tooling
   - Upgrade safety and rollback docs

8. **Ecosystem + Templates**
   - Connector starter set (CRM, ticketing, docs, messaging)
   - Template arrangements/workflows by use case
   - Under 30 min time-to-first-value path

9. **Trust and Compliance Pack**
   - Security posture docs
   - Data retention/export controls
   - Auditability and incident response basics

10. **Positioning + Onboarding Narrative**
   - Replace generic “agent framework” messaging
   - “Run reliable agent operations in one day” onboarding flow
   - Product-led proof of value in first session

## Telemetry Plan (parallel to all milestones)

### North Star
- **Activation rate:** installs that reach first successful workflow run within 24h

### Event instrumentation
- Install: `tutti_install_started`, `tutti_install_completed`
- Activation: `first_workspace_created`, `first_up_success`, `first_send`, `first_workflow_run`
- Habit: DAW (daily active workspaces), returning workspace cohorts (D1/D7/D30), runs per workspace
- Outcome: run success/failure, failure type (`routing/tool/model/policy`), time-to-success
- Commercial: conversion funnel install → first up → first successful run → D7 return

**PII boundaries (mandatory):**
- Allowed fields: timestamps, boolean flags, counters, duration buckets, workflow/run status, failure category.
- Identifier handling: workspace/user identifiers must be one-way pseudonymized (salted hash).
- Explicitly excluded: workspace names, user emails, full prompts/messages, raw payload bodies.
- Retention: raw event payloads 30 days; aggregated cohort and DAW metrics 12 months.
- Enforcement: apply and verify via the "Redaction/privacy defaults" deliverable before dashboard rollout.

### Telemetry deliverables
- Event schema + versioning
- Redaction/privacy defaults
- Dashboard for activation, reliability, retention
- Alerting on activation regression and failure spikes

## Suggested sequencing (8-week sketch)
- **Weeks 1-2:** P0.1 + P0.2 + telemetry schema foundation
- **Weeks 3-4:** P0.3 + P0.4 + failure attribution dashboards
- **Weeks 5-6:** P0.5 + P1.6 (observability + cost governance)
- **Weeks 7-8:** P1.7 + P1.8 + onboarding narrative improvements
- Ongoing: P1.9 + P1.10 hardening and GTM tuning

## Success criteria
- Activation: >= 60% of installs reach first successful workflow run within 24h
- Reliability: run failure rate < 5% and MTTR < 2 hours
- Operator trust: approval/audit/cost controls are production-usable and exercised in at least one documented runbook
- Adoption: >= 3 production-ready templates with documented use cases
- Retention: D7 returning-workspace cohort >= 40%
