# tutti

Your agents, all together.

Tutti is an open-source orchestration layer for multi-agent development. It sits above whatever agent CLIs you already use — Claude Code, Codex, Aider, Gemini CLI, or anything else — and turns independently-running agents into a coordinated team.

No new subscriptions. No API keys. No vendor lock-in. Bring your own agents.

```
tt up                    # launch your agent team from a tutti.toml
tt up --mode safe        # keep interactive permission prompts
tt up --mode unattended --policy bypass
                         # max autonomy (highest risk)
tt status                # see what every agent is doing right now
tt diff frontend         # inspect agent branch + worktree changes
tt land frontend         # cherry-pick agent commits into current branch
tt land frontend --force # ignore local tracked-dirty check
tt land frontend --pr    # push agent branch + open PR via gh
tt review frontend       # send review packet to reviewer agent
tt usage --by-workspace  # inspect API-profile token usage + plan % (Claude/Codex logs)
tt watch                 # interactive terminal status dashboard
tt switch                # fuzzy-pick a running agent and attach
tt send frontend "Analyze the review page UX issues"
                         # send ad-hoc prompt to a running session
tt send frontend --wait --timeout-secs 900 "Analyze the review page UX issues"
                         # wait for activity->idle completion
tt send frontend --auto_up --wait --output "Analyze the review page UX issues"
                         # auto-start if needed and print captured pane delta
tt health --json         # probe + print machine-readable health
tt serve --port 4040     # scheduler + probes + local /v1 control API
tt handoff generate backend
                         # write a handoff packet to .tutti/handoffs
tt handoff apply backend # send latest packet into running backend session
tt run verify-app        # run reusable workflow steps (prompt + commands)
tt run --list            # show configured workflows
tt run --list --json     # machine-readable workflow discovery
tt run verify-app --dry-run --json
                         # machine-readable resolved execution plan
tt run verify-app --json # machine-readable workflow execution result
tt verify                # run verification workflow + persist summary
tt verify --json         # machine-readable verify execution result
tt verify --last         # show latest persisted verification summary
tt verify --last --json  # machine-readable verify status
tt doctor                # preflight runtime/profile/tool-pack prerequisites
tt doctor --strict       # CI gate (warnings become failures)
tt doctor --json         # machine-readable preflight report
tt permissions check git status
                         # evaluate team command policy
tt permissions check git status --json
                         # machine-readable policy decision
tt logs backend -f       # follow captured output for an agent
```

## The Problem

You're running 5+ agent sessions across terminals and monitors. Each one is powerful on its own. But *you* are the orchestration layer — manually tracking what each agent is working on, writing handoff packets when context runs thin, eyeballing token spend, and copy-pasting state between sessions.

That doesn't scale. Tutti does.

## What Tutti Is

**An orchestration layer, not another agent.** Tutti doesn't replace Claude Code or Codex or Aider. It wraps around them. It spawns terminal sessions using whatever agent CLI you already have installed and authenticated. Your existing subscriptions, your existing workflows.

**Org code.** Your agent team topology — who does what, how they communicate, what context they share — is defined in a `tutti.toml` file. Version it. Share it. Fork someone else's.

**Observable by default (today: terminal UI, planned: web UI).** Today Tutti ships a live terminal watch mode plus status and usage commands. A web dashboard is planned.

**Automated handoffs (planned).** Context packet generation and one-command session replacement are on the roadmap.

**Resilience (partially built).** Tutti detects auth failures and captures emergency state. Provider-wide outage handling, pause/resume, and profile failover are planned.

**Multi-subscription aware (partially built).** Profile configuration and capacity tracking are built. Automatic rate-limit rotation/failover is planned.

## What Tutti Is Not

- Not an IDE. Your IDE is already terminals.
- Not tied to any model provider. Claude, OpenAI, local models — whatever.
- Not a framework that requires buy-in. Start with `tt up` and one agent. Add complexity when you need it.
- Not a replacement for your agent's capabilities. Tutti orchestrates. Your agents execute.

## Quick Start

```bash
# Install (from source)
git clone https://github.com/nutthouse/tutti.git
cd tutti
cargo install --path . --locked

# Initialize in your project
cd your-project
tt init

# Edit your team config
$EDITOR tutti.toml

# Launch
tt up
```

If `tt` is not found after install, add Cargo bin to your shell PATH:

```bash
echo 'export PATH="$HOME/.cargo/bin:$PATH"' >> ~/.zshrc
source ~/.zshrc
```

## Project Status (March 2026)

### Built and usable now
- Core CLI commands: `init`, `up`, `down`, `status`, `voices`, `watch`, `switch`, `diff`, `land`, `review`, `send`, `handoff`, `attach`, `peek`, `logs`, `usage`, `run`, `verify`, `doctor`, `permissions`, `workspaces`
- Runtime adapters: Claude Code, Codex CLI, Aider
- Dependency-aware startup order (`depends_on`)
- Per-agent git worktree isolation
- Cross-workspace registry (`tt workspaces`, `tt up --all`, `tt down --all`)
- Token/capacity reporting via `tt usage` for API profiles (`plan = "api"`) from local Claude Code + Codex session logs
- `max_concurrent` launch guardrails per profile (`tt up` refuses launches above limit)
- Workspace `[[tool_pack]]` declarations + `tt doctor` prerequisite checks (commands/env/profile/runtime)

### Planned / in progress
- Session replacement flow (`tt handoff apply`) hardening and richer packet templates
- Web dashboard and API/WebSocket UI
- Provider-level failover/rate-limit rotation
- Richer cost attribution and context-health telemetry
- OpenClaw integration hardening (packaging templates + external registry examples)
- Community phrase/arrangement registry

### Integration docs
- External agent/orchestrator contract: `docs/AGENT_INTEGRATION_CONTRACT.md`
- OpenClaw skill contract: `docs/OPENCLAW_SKILL_CONTRACT.md`
- OpenClaw skill starter: `skills/openclaw/SKILL.md`
- OpenClaw integration bundle: `integrations/openclaw/README.md`

## tutti.toml

The team topology file. This is the "org code" — it defines your agent team as a versionable, forkable configuration.

```toml
[workspace]
name = "my-project"
description = "My project workspace"

[workspace.auth]
default_profile = "claude-personal" # profile from ~/.config/tutti/config.toml

[defaults]
worktree = true
runtime = "claude-code"

[launch]
mode = "auto"            # safe | auto | unattended
policy = "constrained"   # constrained | bypass

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

[[workflow]]
name = "verify-app"
schedule = "*/30 * * * *"

[[workflow.step]]
id = "verify"
type = "command"
run = "cargo test --quiet"
cwd = "workspace"
fail_mode = "closed"
output_json = ".tutti/state/verify.json"

[[workflow.step]]
type = "ensure_running"
agent = "backend"

[[workflow.step]]
type = "prompt"
agent = "conductor"
text = "Summarize anomalies from latest snapshot and propose dispatch actions."
inject_files = [".tutti/state/snapshot.json"]

[[workflow.step]]
type = "workflow"
workflow = "verify-app"
strict = true
fail_mode = "closed"

[[workflow.step]]
type = "review"
agent = "backend"
reviewer = "reviewer"

[[workflow.step]]
type = "land"
agent = "backend"
force = true

[[hook]]
event = "workflow_complete"
workflow_source = "observe_cycle"
workflow_name = "verify-app"
run = "echo scheduled verify completed"
```

Profiles are configured globally in `~/.config/tutti/config.toml`:

```toml
[[profile]]
name = "claude-personal"
provider = "anthropic"
command = "claude"
max_concurrent = 5
plan = "max"
reset_day = "monday"
weekly_hours = 45.0
```

`tt usage` scans and aggregates usage only for profiles with `plan = "api"`.
`tt permissions` is opt-in and reads `[permissions]` from `~/.config/tutti/config.toml`.
With default launch mode (`auto`), constrained non-interactive runs require `[permissions]` allow rules.
For prompt steps that need workspace artifacts, use `inject_files = ["relative/path.json"]` to copy files into the target agent's working tree before the prompt is sent.

Optional tool packs can be declared per workspace and validated with `tt doctor`:

```toml
[[tool_pack]]
name = "analytics"
required_commands = ["bq", "jq"]
required_env = ["GCP_PROJECT"]
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

### Agent Management (Built)
- Spawn and manage agents from any supported runtime
- Git worktree isolation per agent (configurable)
- Session persistence across restarts
- Start and terminate individual agents (`tt up` / `tt down`)
- Inspect worktree + branch changes (`tt diff <agent>`)
- Land agent commits into current branch (`tt land <agent>`)
- Override local cleanliness guard when needed (`tt land <agent> --force`)
- Push/open PRs from agent branches (`tt land <agent> --pr`)
- Dispatch review packets to reviewer agent (`tt review <agent>`)
- Ad-hoc prompt dispatch with optional auto-start + wait + captured output (`tt send --auto_up --wait --output`)

### Automation (Built)
- `tt run` / `tt verify` reusable workflow execution with persisted run records
- Workflow step types: `prompt`, `command`, `ensure_running`, `workflow` (nested), `review`, `land`
- `workflow_complete` hooks for deterministic chaining
- Auto-reclaim of newly-started `persistent = false` sessions at workflow end
- `tt serve` local control API endpoints:
  - Reads: `/v1/health`, `/v1/status`, `/v1/voices`, `/v1/workflows`, `/v1/runs`, `/v1/logs`, `/v1/handoffs`, `/v1/policy-decisions`, `/v1/events`
  - Event cursor/list filter: `/v1/events?cursor=<RFC3339 timestamp>&workspace=<name>`
  - SSE stream: `/v1/events/stream?cursor=<RFC3339 timestamp>&workspace=<name>`
  - Stream emits lifecycle/control events (`agent.started`, `agent.stopped`, `agent.working`, `agent.idle`, `agent.auth_failed`, `workflow.started`, `workflow.completed`, `workflow.failed`, handoff events)
  - Actions (POST): `/v1/actions/up|down|send|run|verify|review|land`
  - Envelope shape: `ok/action/error/data`
  - `send` action returns structured send result (`waited`, `completion_source`, `captured_output`)
  - Mutating actions support `Idempotency-Key` header (or `idempotency_key` request field)

### Observability (Built)
- Real-time status for all running agents
- Profile/workspace token usage and capacity estimates (`tt usage`, API profiles only)
- Interactive terminal watch mode with `PLAN` + live `CTX` plus quick attach/peek flow
- Per-agent log capture and tailing (`tt logs`)

### Handoffs (Built + Planned)
- `tt handoff generate <agent>` creates markdown packets in `.tutti/handoffs/`
- `tt handoff apply <agent>` injects latest packet into a running agent session
- `tt handoff list [--agent ...] [--json]` for packet discovery
- Auto packet generation in `tt watch` (and post-`tt up`) when `CTX` crosses configured handoff threshold
- Configurable packet templates and richer handoff content (planned)
- One-command session replacement polish/hardening (planned)

### Dashboard (Planned)
- Web-based dashboard at localhost (optional)
- Click into any agent to see live output
- Cost breakdown by agent, by provider, by time period
- Provider health panel (auth status, rate limit state)
- Team topology visualization

### Resilience (Partially Built)
- Auth failure detection (OAuth expiry, provider outages)
- Emergency state capture on auth failures
- Correlated failure detection (provider-level vs individual agent) (planned)
- Pause/resume + automatic failover (planned)

### Subscription Management (Partially Built)
- Multiple profiles per provider (personal, work, team accounts)
- Per-profile capacity settings (`plan`, `reset_day`, `weekly_hours`)
- Per-profile concurrency limits (`max_concurrent`) enforced by `tt up`
- Automatic profile rotation and `tt profiles` command (planned)

### Permissions Policy (Built, Opt-in)
- Team-shared command allowlist in `~/.config/tutti/config.toml` under `[permissions]`
- Policy entries may be shell command prefixes (`git status`, `cargo test`) and/or Claude tool names (`Read`, `Edit`, `Write`)
- `tt permissions check <command...>` evaluates command prefixes against policy
- `tt permissions export --runtime claude` emits a Claude settings scaffold
- `tt up` auto-wires constrained non-interactive policy for Claude sessions
- Codex/OpenClaw constrained mode is best-effort (prompt-level policy guidance; Codex also uses non-interactive flags)
- If constrained non-interactive launch is selected without policy, `tt up` fails with guidance
- Launch policy decisions are persisted to `.tutti/state/policy-decisions.jsonl` and exposed via `/v1/policy-decisions`

### Tool Packs (Built, Opt-in)
- Declarative `[[tool_pack]]` blocks in `tutti.toml` (`required_commands`, `required_env`)
- `tt doctor` reports pass/warn/fail for tmux, profile wiring, runtime binaries, and tool-pack prerequisites

### Community (Planned)
- Share and discover arrangements (team configs)
- Publish and install phrases (reusable prompts/skills)
- `tt browse` to explore what others are running

## Architecture

```
┌─────────────────────────────────────┐
│           tt (CLI)                  │
│  init · up · status · watch · usage │
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
| Codex CLI | ✅ Supported | Token tracking via local Codex session logs |
| Aider | ✅ Supported | Model-agnostic |
| OpenClaw | ✅ Supported | Native runtime adapter (`runtime = "openclaw"`) |
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

- [x] Core CLI (`tt init`, `tt up`, `tt down`, `tt status`, `tt voices`, `tt watch`, `tt switch`, `tt diff`, `tt land`, `tt review`, `tt send`, `tt handoff`, `tt attach`, `tt peek`, `tt logs`, `tt usage`, `tt run`, `tt verify`, `tt doctor`, `tt permissions`, `tt workspaces`)
- [x] Claude Code runtime adapter
- [x] Codex runtime adapter  
- [x] Aider runtime adapter
- [x] `tt usage` profile/workspace capacity reporting
- [ ] Context health monitoring
- [x] Automatic handoff packet generation
- [ ] Web dashboard
- [ ] Cost tracking and attribution (provider-accurate)
- [x] OpenClaw skill for Tutti orchestration workflows
- [ ] Phrase registry (community prompts/skills)
- [ ] Arrangement sharing (community team configs)
- [ ] Agent-to-agent communication protocol

## License

MIT

---

*In music, tutti means "all together" — the moment every voice in the ensemble plays as one. That's what your agents should feel like.*
