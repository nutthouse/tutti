# Tutti Local Repo Audit — 2026-05-05

## Executive summary
Tutti looks like a real, fast-moving product repo rather than a half-formed experiment. The combination of healthy README positioning, recent merged PR velocity, green Actions, and a clear architecture/story suggests genuine momentum.

The biggest risk is not technical collapse. It is **scope sprawl outrunning operator trust and activation clarity**.

## Current repo signals
- Repo: `nutthouse/tutti`
- Public repo
- Default branch: `main`
- GitHub signals checked separately: 32 stars, 2 forks, recent merged PR activity, green Actions, no open PR backlog
- Local checkout branch: `main` tracking `origin/main`
- Two local untracked empty files exist: `...` and `merged`
  - Both appear benign junk residue, not structural risk

## Architecture shape
Based on the README and manifest, Tutti is a Rust CLI centered around:
- orchestration core
- runtime adapters for coding agents
- terminal/tmux session layer
- observation layer
- optional dashboard / local control API

This is a credible shape for the product category. It matches the stated wedge: orchestrate existing AI coding agents instead of becoming one.

## Product maturity signals
Strong signals:
- multi-runtime support is already framed clearly
- web dashboard and operator-console concepts are tangible
- automated SDLC language is concrete, not hand-wavy
- recent PR stream suggests active iteration, not stagnation
- issue taxonomy is coherent enough to read like a real product roadmap

Less mature / risk signals:
- versioning/docs may lag reality in places (`Cargo.toml` shows `0.6.0` while README references later project status/history)
- roadmap breadth is large enough to dilute focus
- several later-stage platform concerns are open before the trust/activation loop feels fully locked

## Verification notes
I was able to inspect:
- `README.md`
- `Cargo.toml`
- recent merged PRs
- open issues
- repo state / branch state

A fuller `cargo test` pass was not completed here because approval-gated shell execution interrupted the deeper audit. So this is a grounded inspection report, not a build-verification report.

## Technical risks
1. **Operator trust bugs**
   - #122 is the best example. Reliability papercuts undermine the product more than missing advanced features.

2. **Mismatch between story and first-run experience**
   - The README promise is ambitious. If first-run value is weaker than the pitch, traction leaks.

3. **Scope spread across too many frontier features**
   - Remote uploads, mailbox, approvals, recovery, RBAC, deployment, trust pack: all sensible, but too many “important” fronts at once.

4. **Potential docs/version drift**
   - Signals of fast movement are good, but product status and package version should stay crisp to preserve trust.

## Best next sprint from an engineering perspective
### Theme: make the operator trust Tutti
Recommended sprint contents:
- Fix **#122** spinner/activity detection bug
- Tighten one adjacent reliability edge if found during implementation
- Make sure the visible operator story matches current reality
- Avoid starting a deep new systems feature in the same sprint

## What to avoid this week
- starting mailbox + uploads + guardrails all at once
- deep enterprise work (RBAC, compliance pack) before activation is sharper
- adding product surface area faster than reliability

## Recommendation
Tutti is healthy enough to treat seriously. The next sprint should **narrow hard around reliability + activation clarity**, not chase breadth.
