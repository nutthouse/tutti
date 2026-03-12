# tutti

Your agents, all together.

Tutti is an open-source orchestration layer for multi-agent development. It sits above whatever agent CLIs you already use — Claude Code, Codex, Aider, Gemini CLI, or anything else — and turns independently-running agents into a coordinated team.

No new subscriptions. No API keys. No vendor lock-in. Bring your own agents.

```
tt up                    # launch your agent team from a tutti.toml
tt status                # see what every agent is doing right now
tt handoff agent-3       # auto-generate a context packet for a fresh session
tt dash                  # open the web dashboard
```

## The Problem

You're running 5+ agent sessions across terminals and monitors. Each one is powerful on its own. But *you* are the orchestration layer — manually tracking what each agent is working on, writing handoff packets when context runs thin, eyeballing token spend, and copy-pasting state between sessions.

That doesn't scale. Tutti does.

## What Tutti Is

**An orchestration layer, not another agent.** Tutti doesn't replace Claude Code or Codex or Aider. It wraps around them. It spawns terminal sessions using whatever agent CLI you already have installed and authenticated. Your existing subscriptions, your existing workflows.

**Org code.** Your agent team topology — who does what, how they communicate, what context they share — is defined in a `tutti.toml` file. Version it. Share it. Fork someone else's.

**Observable by default.** Live dashboard showing every agent's status (working / idle / blocked / errored), token usage, cost attribution, elapsed time, and context health — across all providers.

**Automated handoffs.** When an agent's context gets thin, Tutti serializes the working state into a handoff packet — the current task, relevant file paths, decisions made, what's left — and can spin up a fresh session pre-loaded with that context. No more writing handoff prompts by hand.

**Resilient by default.** OAuth will break. Providers will go down. Rate limits will hit. Tutti detects auth failures, distinguishes individual agent errors from provider-wide outages, preserves state before anything is lost, and can pause/resume or failover to alternate subscriptions. Your work survives provider chaos.

**Multi-subscription aware.** Run Claude Pro for some agents and Claude Max for heavy workloads. Rotate to a backup subscription when rate limits hit. Set budget caps per profile. Tutti manages the complexity of multiple subscriptions across multiple providers so you don't have to.

## What Tutti Is Not

- Not an IDE. Your IDE is already terminals.
- Not tied to any model provider. Claude, OpenAI, local models — whatever.
- Not a framework that requires buy-in. Start with `tt up` and one agent. Add complexity when you need it.
- Not a replacement for your agent's capabilities. Tutti orchestrates. Your agents execute.

## Quick Start

```bash
# Install
curl -fsSL https://tutti.dev/install.sh | bash

# Initialize in your project
cd your-project
tt init

# Edit your team config
$EDITOR tutti.toml

# Launch
tt up
```

## tutti.toml

The team topology file. This is the "org code" — it defines your agent team as a versionable, forkable configuration.

```toml
[team]
name = "my-project"

[[agent]]
name = "backend"
runtime = "claude-code"          # or "codex", "aider", "gemini-cli", etc.
scope = "src/api/**"
prompt = "You own the API layer. Use existing patterns. Track work in bd."

[[agent]]
name = "frontend"
runtime = "claude-code"
scope = "src/app/**"
prompt = "You own the UI. Follow existing component patterns."

[[agent]]
name = "tests"
runtime = "codex"
scope = "tests/**"
prompt = "Write and maintain tests. Run the test suite after changes."
depends_on = ["backend", "frontend"]

[handoff]
auto = true                      # auto-generate handoff packets at 20% context
threshold = 0.2
include = ["active_task", "file_changes", "decisions", "blockers"]

[observe]
dashboard = true                 # serve web dashboard on localhost
port = 4040
track_cost = true

# Subscription profiles (for multi-account setups)
[[profile]]
name = "claude-personal"
provider = "anthropic"
command = "claude"
max_concurrent = 5
monthly_budget = 100.00

[[profile]]
name = "claude-work"
provider = "anthropic"
command = "claude"
max_concurrent = 10
priority = 2                     # fallback when personal hits limits

# Resilience
[resilience]
provider_down_strategy = "pause" # pause agents, preserve state, resume when back
save_state_on_failure = true
rate_limit_strategy = "rotate"   # auto-switch to fallback profile on rate limit
```

## Core Concepts

### Voices
Each running agent instance is a **voice** — the musical term for an individual part in an ensemble. `tt voices` lists what's playing.

### Arrangements
A `tutti.toml` file is an **arrangement** — the configuration that tells each voice what to play and when. Share arrangements, fork them, adapt them to your project.

### Movements
A **movement** is a phase of work — a logical grouping of tasks across agents. "Build the auth system" might be one movement containing work across backend, frontend, and test voices.

### Phrases
Reusable prompt components and skills are **phrases**. A phrase might be a CLAUDE.md snippet, a testing methodology, a code style guide, or an architectural pattern. Publish and share phrases through the community registry.

## Features

### Agent Management
- Spawn and manage agents from any supported runtime
- Git worktree isolation per agent (configurable)
- Session persistence across restarts
- Start, pause, resume, and terminate individual agents

### Observability
- Real-time status for all running agents
- Per-agent token usage and cost tracking (multi-provider)
- Context window health monitoring
- Activity timeline and decision log

### Handoffs
- Automatic context serialization when context runs low
- Configurable handoff packet contents
- One-command session replacement with context transfer
- Handoff history for audit and replay

### Dashboard
- Web-based dashboard at localhost (optional)
- Click into any agent to see live output
- Cost breakdown by agent, by provider, by time period
- Provider health panel (auth status, rate limit state)
- Team topology visualization

### Resilience
- Auth failure detection (OAuth expiry, provider outages)
- Correlated failure detection (provider-level vs individual agent)
- Automatic state preservation on any failure
- Pause/resume agents when providers recover
- Failover to alternate runtimes or subscription profiles
- Rate limit detection and automatic profile rotation

### Subscription Management
- Multiple profiles per provider (personal, work, team accounts)
- Per-profile rate limit tracking and budget caps
- Automatic rotation when limits are hit
- `tt profiles` to see subscription health across all accounts

### Community
- Share and discover arrangements (team configs)
- Publish and install phrases (reusable prompts/skills)
- `tt browse` to explore what others are running

## Architecture

```
┌─────────────────────────────────────┐
│           tt (CLI)                  │
│  init · up · status · handoff · dash│
├─────────────────────────────────────┤
│        Orchestration Core           │
│  Team topology · Agent lifecycle    │
│  Context monitoring · Cost tracking │
├──────────┬──────────┬───────────────┤
│ Runtime  │ Runtime  │ Runtime       │
│ Adapter: │ Adapter: │ Adapter:      │
│ Claude   │ Codex    │ Aider/Custom  │
├──────────┴──────────┴───────────────┤
│       Terminal Session Layer        │
│  tmux/zellij · git worktrees       │
│  PTY capture · ANSI parsing        │
├─────────────────────────────────────┤
│        Observation Layer            │
│  Token counting · Cost attribution  │
│  Status detection · Context health  │
├─────────────────────────────────────┤
│         Dashboard (optional)        │
│  Web UI · REST API · WebSocket feed │
└─────────────────────────────────────┘
```

## Supported Runtimes

| Runtime | Status | Notes |
|---------|--------|-------|
| Claude Code | ✅ Primary | Full support including context monitoring |
| Codex CLI | ✅ Supported | Token tracking via CLI output |
| Aider | ✅ Supported | Model-agnostic |
| Gemini CLI | 🔜 Planned | |
| Custom | 🔜 Planned | Any CLI agent via adapter interface |

## Philosophy

**BYOS: Bring Your Own Subscription.** Tutti never asks for your API keys. It spawns agents using whatever CLI tools you already have installed and authenticated. If you can run `claude` in your terminal, Tutti can orchestrate it.

**Org code is real code.** How you structure your agent team is as important as the code they write. It should be versioned, reviewed, and iterable — just like infrastructure-as-code or CI/CD pipelines.

**Observe everything, control nothing.** Tutti watches what your agents do but doesn't intercept or modify their behavior. It's a coordination and visibility layer, not a proxy.

**Start simple, scale up.** One agent in a tutti.toml is fine. You don't need five agents and a complex topology on day one. Tutti should make even a single agent session better through observability and handoff support.

## Contributing

Tutti is early. If this resonates with how you work, we want to hear from you.

- **Issues**: Bug reports, feature requests, questions
- **Discussions**: Share your arrangements, talk about workflows
- **PRs**: See CONTRIBUTING.md for guidelines

## Roadmap

- [ ] Core CLI (`tt init`, `tt up`, `tt status`, `tt voices`)
- [ ] Claude Code runtime adapter
- [ ] Codex runtime adapter  
- [ ] Aider runtime adapter
- [ ] Context health monitoring
- [ ] Automatic handoff packet generation
- [ ] Web dashboard
- [ ] Cost tracking and attribution
- [ ] Phrase registry (community prompts/skills)
- [ ] Arrangement sharing (community team configs)
- [ ] Agent-to-agent communication protocol

## License

MIT

---

*In music, tutti means "all together" — the moment every voice in the ensemble plays as one. That's what your agents should feel like.*
