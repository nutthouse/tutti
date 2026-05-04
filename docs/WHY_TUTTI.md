# Why Tutti Exists

Every big AI platform is shipping an agent SDK. That is good. It also means Tutti should not pretend to be another generic agent framework.

Tutti exists for the layer above agents: agent operations.

## The Problem

AI coding work now spans more than one chat window:

- Work starts in GitHub, Linear, Jira, Slack, or a local queue.
- Agents run in Claude Code, Codex, Aider, OpenClaw, or direct model APIs.
- Changes land through branches, PRs, review tools, human approvals, CI, and merge gates.
- Useful output is scattered across terminals, files, logs, artifacts, comments, and dashboards.

Without an operations layer, the human becomes the scheduler, reviewer, debugger, historian, and merge coordinator.

That does not scale.

## What Tutti Owns

Tutti turns agent work into versioned operations:

- **Topology**: which agents exist, what they own, and how they are launched.
- **Workflow**: what steps happen, in what order, with which dependencies.
- **Isolation**: worktrees and runtime boundaries so agents do not trample each other.
- **Artifacts**: plans, test reports, review packets, run outputs, and handoff files.
- **Gates**: checks, reviews, policies, approvals, and merge requirements.
- **State**: ledgers, checkpoints, telemetry, events, and replayable history.
- **Observability**: dashboards, logs, run status, failure categories, and next actions.

This is the Terraform analogy in practice. The important object is not a prompt. It is the versioned arrangement of work.

## What Tutti Does Not Need To Own

Tutti does not need to own every agent brain.

OpenAI, Anthropic, Google, Microsoft, AWS, CrewAI, LangGraph, and others will keep building model-native harnesses, agent SDKs, tool protocols, and hosted runtimes. Tutti should integrate with them where useful, but the product boundary is not "better reasoning."

The product boundary is operational control.

## The Adapter Shape

The first Tutti-for-Tutti path uses GitHub issue intake and CodeRabbit review because those are concrete and useful today. They should be treated as adapters, not assumptions.

The durable shape is:

1. **Intake adapter**: GitHub, Linear, Jira, webhook, local file, or manual queue.
2. **Execution adapter**: Claude Code, Codex, Aider, OpenClaw, API-direct provider, or future agent tool.
3. **Review adapter**: CodeRabbit, Claude/Codex reviewer, human reviewer, CI, static analysis, or policy engine.
4. **Gate adapter**: required checks, approval state, resolved threads, budget, policy, or release rule.
5. **Record adapter**: run ledger, event log, artifacts, replay output, and dashboard state.

That shape is the product.

## The Near-Term Wedge

The next user-facing goal is simple:

> A stranger installs Tutti and reaches a first successful workflow run in minutes.

That means the first experience should optimize for activation:

- Clear positioning: agent operations, not another agent framework.
- One obvious quick start.
- A small workflow that succeeds before a user commits to a full agent team.
- A canonical SDLC example that names its adapter boundaries.
- A dashboard and logs that explain what happened.

Traffic and stars are useful signals. The real signal is a second run.

## Why This Can Win

Frameworks compete on how agents think.

Tutti competes on how agent work runs.

That is a smaller lane, but a better one. Developers and teams will keep changing models, SDKs, CLIs, review tools, and issue trackers. They still need a repeatable way to operate the work.

Tutti should be that layer.
