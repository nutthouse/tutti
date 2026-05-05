# Tutti Issue Triage — 2026-05-05

## Summary
Tutti has crossed the line from interesting internal tool to emerging real product. The issue queue should now optimize for **trust, activation, and core orchestration value** — in that order.

## Now

### 1. #122 — wait_for_prompt_activity spinner detection bug
**Why now:** This is the highest-leverage issue because it damages operator trust in the orchestration layer itself. If Tutti falsely thinks agents are idle or failed while they are actively thinking, the whole product feels flaky.

**Impact:** Reliability, confidence, dogfooding quality, outside adoption.

### 2. #24 — Positioning and onboarding narrative
**Why now:** Traction is starting to appear. People need to understand what Tutti is for, when to use it, and why it beats ad hoc multi-agent chaos.

**Impact:** Activation, category clarity, forks/stars conversion.

### 3. #25 — Telemetry and activation instrumentation
**Why now:** Traction without instrumentation is vibes. This issue turns outside interest and internal dogfooding into measurable insight.

**Impact:** Learning speed, activation tuning, prioritization quality.

## Next

### 4. #56 — agent mailbox for inter-agent coordination
**Why next:** This is a true core product primitive. It deepens the orchestration moat, but it should land after trust and activation basics are stronger.

**Impact:** Product depth, reduced human relay burden, stronger coordination story.

### 5. #83 — context bridge / file-image upload to remote agents
**Why next:** High user value, especially for remote-first/VPS workflows. But it is less important than operator trust and clear proof of value.

**Impact:** Remote workflow usability, differentiated value.

### 6. #36 — `tt run --auto-allow` bootstrap mode
**Why next:** Useful for smoother autonomous workflow adoption, but only once core operator trust is stronger.

### 7. #27 — post-merge release finalizer
**Why next:** Good automation hygiene, but not the primary adoption bottleneck.

## Later
- #20 — cost escalation and autonomy guardrails
- #18 — approval gates and guardrail workflows
- #17 — unattended runtime recovery and provider auto-switch
- #23 — trust and compliance pack
- #21 — deployment and operations story
- #15 — identity, auth, and RBAC foundations

These matter, but they are later-stage multipliers. They should not outrank immediate trust, onboarding, and activation.

## Top 5 ranked issues
1. **#122** — reliability/trust bug in prompt activity detection
2. **#24** — onboarding and positioning narrative
3. **#25** — telemetry and activation instrumentation
4. **#56** — agent mailbox
5. **#83** — context bridge uploads

## Missing issue that should exist
A focused issue for **"first 10 minutes activation proof"** should probably exist if it does not already. Positioning (#24) is about narrative, but Tutti also needs an explicit product issue aimed at making the first-run experience undeniably useful.

Suggested issue:
- Improve time-to-value in first session
- Make one workflow/demo path obviously magical
- Reduce setup friction and ambiguity
- Define the exact signal: first successful orchestrated workflow within 10–15 minutes

## Recommendation
If only one engineering issue gets worked next, it should be **#122**.
If one product issue gets worked in parallel, it should be **#24**.
