# Why I Stopped Building Agent Swarms

Everyone's building agent swarms. Multiple AI agents talking to each other, voting on decisions, passing messages through SQLite mailboxes, coordinating via shared memory. It sounds amazing in a demo. It falls apart in production.

I know because I tried it. Then I built the opposite.

## The question nobody asks

Before comparing swarms vs sequential, there's a more fundamental question: **why run multiple agents at all?**

Claude has a 1M token context window. That's roughly 750,000 words — more than the entire Lord of the Rings trilogy, twice. You could paste your entire codebase into a single conversation. So why would you split the work across separate agents, each with their own context?

Because a bigger window doesn't mean a better one.

## The context pollution problem

Here's what happens when you ask a single agent to plan, implement, test, review, and document a feature:

By step 3, the context is polluted. The agent is carrying the entire planning conversation — the rejected approaches, the back-and-forth on scope, the design alternatives you didn't pick. When it starts implementing, all of that planning debris is sitting in context, competing for attention with the actual code it needs to write.

By step 4 (testing), the context contains the implementation discussion too — the debugging tangents, the "actually let me try a different approach" pivots, the dead-end explorations. The agent isn't just reading your code anymore. It's reading through a history of everything it tried, including the things that failed.

This is the dirty secret of large context windows: **more context doesn't mean better context.** A 200K token conversation where 150K is planning history and debugging tangents is worse than a 20K token conversation that contains exactly the artifact the agent needs to do its job.

LLMs have attention. Attention is finite. When the context is full of noise, the signal gets diluted — even if the window is technically big enough to hold everything. Research on "lost in the middle" shows that models struggle to attend to information buried in long contexts, even when it's there.

## Why separate agents win: context specialization

Each tutti agent starts with a **clean context** optimized for its specific job:

**The planner** gets:
- The project's CLAUDE.md (conventions, architecture)
- The design prompt
- The human's answers to design questions
- Nothing else. No implementation details. No test output. No debugging history.

When the planner produces a design doc, that document is a **distilled artifact** — the planning conversation compressed into its conclusions. The rejected approaches, the back-and-forth, the "what about..." tangents are gone. What remains is the decision.

**The implementer** gets:
- The design doc (conclusions only, not the process)
- The test plan (what to build toward)
- A clean git worktree with the current codebase
- Its system prompt: "You own implementation. Commit and push when done."

The implementer never sees the planning debate. It doesn't know there were three approaches considered. It doesn't carry the 50K tokens of design conversation. It sees the artifact — "build this" — and builds it in a clean context where 100% of its attention is on the code.

**The tester** gets:
- The test plan
- The implementer's diff
- A clean context with zero implementation history

The tester has never seen the implementer struggle with a tricky type error for 30 minutes. It has no sympathy for the implementation. It reviews the code cold, the way a real reviewer would — with fresh eyes and no sunk-cost bias.

This is the core insight: **separate agents aren't about parallelism. They're about context hygiene.**

## The artifact handoff is the key mechanism

The thing that makes this work isn't agents. It's the artifact boundary between them.

When the planner finishes, it produces a design doc. That doc is a lossy compression of the planning conversation — and the lossiness is the feature. All the noise, tangents, and rejected approaches are discarded. Only the signal survives the boundary.

```
Planning conversation    →  Design doc artifact    →  Implementation context
(50K tokens of debate)      (2K tokens of decisions)   (clean, focused, no debris)
```

Each artifact boundary is a noise filter. The implementer doesn't need to know why you chose approach B over approach A. It needs to know what approach B is. The design doc tells it exactly that and nothing more.

This is why swarms fail: **they skip the artifact boundary.** When agents share a message bus or a SQLite mailbox, they're sharing raw conversation — unfiltered, uncompressed, full of noise. Agent A's debugging tangent becomes Agent B's context pollution. The shared memory that's supposed to enable coordination becomes a vector for attention dilution.

## Git worktrees: physical isolation, not just logical

Each tutti agent runs in its own git worktree. This isn't just about preventing file conflicts (though it does that too). It's about giving each agent a **physically separate view of the codebase**.

The implementer's worktree diverges from main as it works. The tester's worktree stays on main until the implementer's changes are ready for review. The docs agent never sees half-finished implementation code.

```
main branch  ──────────────────────────────────────────
                \                    \
                 └── tutti/implementer  (diverges during implementation)
                                      \
                                       └── tutti/tester  (created when implementation is done)
```

This means each agent's `Read` and `Grep` tools see exactly the files relevant to their job. The implementer sees the code it's changing. The tester sees the finished diff. The reviewer sees the PR-ready state. Nobody sees work-in-progress files from another agent's half-finished task.

Compare this to swarms: 5 agents sharing one checkout, each reading files the others are actively editing. Agent A reads `main.rs`, starts planning changes. Agent B edits `main.rs` while A is thinking. A's plan is now based on a stale file. This isn't a theoretical problem — it's the primary failure mode of parallel agent systems.

## Scoped prompts: telling agents what NOT to think about

Each agent gets a system prompt that defines its role and — critically — its scope:

```toml
[[agent]]
name = "implementer"
scope = "src/**"
prompt = "You own implementation. Commit and push when done."

[[agent]]
name = "tester"
scope = "src/**"
prompt = "You write and run tests."
```

The `scope` field isn't just documentation. It tells the agent where to focus. The implementer doesn't explore `docs/` or `templates/` — it stays in `src/**`. The planner doesn't touch code at all — it produces documents.

This is the opposite of a single-agent approach where one context does everything. A single agent with a big context window has no natural boundary between "planning mode" and "implementation mode" and "testing mode." It drifts. It starts fixing a test while it was supposed to be implementing a feature. It refactors code while it was supposed to be reviewing a PR.

Separate agents can't drift because they don't have access to drift into. The implementer can't accidentally start writing docs — it doesn't see the docs directory. The tester can't accidentally start implementing — it receives the diff as a read-only artifact.

## The coordination trap

Here's what happens when you run 5 agents on the same codebase simultaneously:

They stomp on each other's files. Agent A edits `main.rs` while Agent B is reading it. Agent C commits a change that breaks Agent B's work. You spend more tokens on coordination than on actual coding.

One competitor's own internal docs admit this honestly: "swarms often perform worse than a single focused agent." The agents spend tokens polling a mail bus, resolving conflicts, and re-reading context that changed since they last looked. I call it coordination theater — it looks like productivity, but it's overhead pretending to be work.

The coordination tax scales quadratically. 2 agents need 1 coordination channel. 3 need 3. 5 need 10. 30 agents (as one tool claims to run) need 435 coordination channels. Every token spent resolving a merge conflict or polling a message bus is a token not spent writing code.

Sequential workflows have zero coordination channels. Agent A finishes, produces an artifact, Agent B starts. The coordination cost is constant: one artifact handoff per step.

## The idle detection problem

The one hard problem with sequential workflows isn't coordination — it's knowing when an agent is done.

When a skill like `/office-hours` asks the human a question, the agent goes idle. The terminal stops changing. The health monitor sees "idle" and thinks "task complete" — moves to the next step. But the skill isn't done. It's waiting for the human.

This tripped us up during dogfooding. The planner ran `/office-hours`, the skill asked the human a design question, and tutti immediately advanced to the next step because the pane hash stopped changing.

We solved this with artifact-polling. Instead of watching for idle, tutti watches for the output file. The agent can sit there waiting for human input for an hour. When the design doc file appears on disk, tutti captures it and continues.

```toml
[[workflow.step]]
id = "design"
agent = "planner"
text = "/office-hours"
artifact_glob = "~/.gstack/projects/{slug}/*-design-*.md"
artifact_name = "design_doc"
# No wait_for_idle — polls for the artifact file instead
```

Simple. No agent-to-agent communication protocol. No message bus. Just: did the file appear? Yes → next step.

## When you actually want parallelism

Sequential doesn't mean slow. The bottleneck in AI-assisted development isn't compute — it's review.

The real parallelism in software development isn't multiple agents editing the same file simultaneously. It's multiple agents working on **different concerns** — implementation, testing, documentation, review — with clear boundaries and typed handoff.

You can run the implementer and docs agent in parallel because they touch different files. You can run the tester while the human reviews the design doc. The parallelism comes from the natural independence of concerns, not from forcing agents to share a codebase.

The tutti workflow engine supports this — `ensure_running` steps can launch agents that work independently, and the artifact pipeline merges their outputs at the right point in the workflow.

## The role mapping insight

One more thing that falls out of the multi-agent model: you can **swap the brain behind each role** without changing the workflow.

```toml
[roles]
planner = "claude-code"
implementer = "claude-code"
reviewer = "codex"
tester = "claude-code"
```

The reviewer runs on Codex (OpenAI). Everything else runs on Claude. Why? Because code review benefits from a different model's perspective — the same way a human code review benefits from fresh eyes. The reviewer literally has a different neural network evaluating the code.

Change one line and the implementer runs on Codex too:

```toml
implementer = "codex"
```

Every agent using the `implementer` role switches runtime. No per-agent edits. This is impossible with a single-agent approach — you can't run part of a conversation on Claude and part on GPT within one context window.

## The numbers

Last week we ran tutti's full SDLC pipeline end-to-end:
- 50 commits to main
- 17,200 lines of code
- 7 releases (v0.3.0 → v0.9.0)
- 8 consecutive active days
- 84% of commits AI-assisted

Zero agent coordination conflicts. Zero merge resolution. Zero tokens spent on agents talking to each other.

The artifact pipeline handled the handoff: design doc → test plan → implementation → review → ship. Each agent received exactly the context it needed, nothing more.

## The positioning map

```
              HIGH COORDINATION
                    |
         Swarms     |
                    |
                    |
LOW AUTO ---+-------+-------+--- HIGH AUTO
            |       |       |
            | squad |       | Sequential
            |       |       | (tutti)
                    |
              LOW COORDINATION
```

Different quadrants, different bets. Swarms bet that coordination complexity is worth the overhead. Sequential workflows bet that structured repeatability ships more code.

I'm betting on sequential. Not because swarms can't work — but because the marginal cost of coordination is higher than the marginal cost of waiting for the previous step to finish. And because every artifact boundary is a free noise filter that makes the next agent smarter.

The best agent isn't the one with the biggest context. It's the one with the cleanest context.

## What we learned building this: 7 lessons from dogfooding

Tutti is built by tutti. The framework orchestrates its own development — we call them "choir runs." Every lesson below came from a real failure during dogfooding, not from theory.

### 1. Agents that review their own code are worse than agents that review cold

During our first end-to-end SDLC run, we had the implementer agent review its own PR. It approved everything. Of course it did — it wrote the code. It had 100K tokens of context explaining why every decision was the right one. It couldn't see its own mistakes because it had already rationalized them during implementation.

We split review into a separate agent with a different model (Codex instead of Claude). The tester received only the diff and the test plan — no implementation history, no debugging tangents, no "I tried this first but then..." context.

The result was dramatic. In the v0.8.0 dogfood run, the tester agent (running cold on the implementer's output) found 5 real issues: a missing workflow in the gstack-startup template, non-deterministic HashMap iteration in the role display, incomplete test coverage for detect_all partial matches, a formatting inconsistency, and a misplaced cfg(test) import. The implementer would never have caught these — it wrote the code knowing the intent, so the gaps were invisible to it.

**Lesson: context isolation isn't a limitation. It's an audit mechanism.** The tester's ignorance of the implementation is a feature, not a bug. Fresh eyes find bugs that sunk-cost eyes can't.

### 2. The planner produces better designs when it can't see the code

This surprised us. Early on, the planner agent had full access to the source code. It would read through the implementation, form opinions about what was hard to change, and produce conservative designs that worked around existing code rather than improving it.

When we scoped the planner to `docs/**` and only gave it CLAUDE.md (project conventions) plus the design prompt, the designs got bolder. The planner stopped thinking about implementation difficulty — that wasn't its problem. It started thinking about what the user actually needs.

The v0.8.0 template packs design doc proposed role mapping, a template format with detection scoring, and artifact-polling mode. None of these were obvious extensions of the existing code. They required rethinking how `tt init` works, how config validation works, and how the workflow engine detects completion. A planner that had read through 17K lines of Rust would have proposed something incremental. A planner working from first principles proposed something better.

**Lesson: agents with less context make bolder decisions.** When you can't see the constraints, you don't design around them. Sometimes that's exactly what you need.

### 3. Worktree isolation caught a real shipping bug

The implementer agent for v0.8.0 committed its work on branch `auto/issue-15-*` in its own worktree. The worktree was based on `main` at `v0.6.0`, but the artifact-polling feature we'd just built was on the `feat/template-packs` branch. When we ran the builtin template validation tests in the implementer's worktree, they failed — the gstack-startup template used `wait_timeout_secs` without `wait_for_idle`, which was only valid in our unreleased config validation changes.

If the implementer had been working in the same checkout as us, this mismatch would have been invisible — the tests would have passed against our local code, and the PR would have broken CI when merged to main.

The worktree isolation surfaced the incompatibility at development time, not at merge time. We cherry-picked the commit onto the right branch and the tests passed.

**Lesson: worktree isolation doesn't just prevent file conflicts. It prevents environmental contamination.** Each agent tests against a known state, not against whatever happens to be on disk.

### 4. Permission scoping reveals hidden dependencies

When we first ran `/office-hours` through tutti's orchestration, it failed. The gstack skill's preamble runs `source <(~/.claude/skills/gstack/bin/gstack-slug ...)` as a Bash command. The planner agent ran in `dontAsk` permission mode with a scoped allowlist — and `source <(...)` wasn't on the list.

Then `/plan-eng-review` failed for a different reason: the `Skill` tool wasn't in our `CLAUDE_TOOL_NAMES` list. Then `/office-hours` failed again because `AskUserQuestion` wasn't allowed.

Each permission failure revealed a hidden dependency: gstack skills need Bash for their preamble, the Skill tool to invoke themselves, and AskUserQuestion to interact with the human. A single agent running in `--dangerously-skip-permissions` mode would never have discovered these dependencies — everything would "just work" until you tried to run it securely.

We added `Skill`, `AskUserQuestion`, and `Bash(source *)` to the allowlist. Now every agent runs with scoped permissions — full autonomy within clear boundaries.

**Lesson: separate agents with scoped permissions act as a dependency scanner.** Every agent is a test of "what does this workflow actually need to run?" Running in bypass mode hides these dependencies. Running with allowlists reveals them.

### 5. The artifact boundary caught a data race we didn't design for

The dashboard focus sidebar had a bug: usage stats would flash with real values then disappear, replaced by zeros. The root cause: two polling loops writing to the same DOM element. The fast 2-second terminal poll included `usage: { input_tokens: 0, ... }` in the response (because it didn't request usage data). The slow 30-second usage poll set the real values. The fast poll overwrote them every 2 seconds.

This is a coordination race — two processes sharing mutable state with no synchronization. Exactly the kind of bug that swarms amplify across 30 agents sharing a message bus.

The fix was an artifact boundary: the server now omits `usage` from the response unless `?usage=1` is explicitly requested. The fast poll can't overwrite what it doesn't send. The slow poll is the single source of truth for usage data.

**Lesson: the artifact boundary pattern scales down to UI state management.** Anywhere two processes share mutable state, the fix is the same: don't share. Give each producer its own output channel. Let the consumer merge at render time.

### 6. The "agent went idle" problem is fundamentally unsolvable with observation alone

We tried three approaches to detect when an agent is done:

1. **Pane hash stability** — watch the tmux pane content, detect when it stops changing. Fails when the agent is waiting for human input (idle but not done).
2. **Output file detection** — watch for a specific output JSON file to appear. Works for non-interactive steps but doesn't work for skills that produce artifacts at unpredictable paths.
3. **Artifact-glob polling** — watch for new files matching a glob pattern. The agent can take as long as it needs; tutti checks every 5 seconds for the artifact.

Approach 3 was the breakthrough. It decouples "agent is working" from "step is complete." The signal isn't the agent's behavior — it's the artifact's existence.

This is why swarms with shared observation are fragile. When 5 agents are all watching each other's pane output to coordinate, they're building a distributed system on an unreliable signal. Agent A "looks done" but is actually waiting for human input. Agent B proceeds because A looks idle. The workflow breaks.

The artifact boundary makes this problem disappear. You never ask "is the agent done?" You ask "did the artifact appear?" One is subjective and error-prone. The other is a file system check.

**Lesson: observe artifacts, not agents.** An agent's terminal state is unreliable. The presence of its output file is binary and unambiguous.

### 7. Tutti developing tutti surfaced 5 framework bugs in one session

During the v0.8.0 dogfood run, we discovered and fixed:

1. **Bash permission denied for `source <(...)`** — parentheses in the command conflicted with Claude Code's `Bash(...)` permission syntax
2. **Skill tool not in CLAUDE_TOOL_NAMES** — gstack skills couldn't invoke themselves in dontAsk mode
3. **AskUserQuestion not in allowlist** — interactive skills couldn't ask the human questions
4. **Idle detection false positive** — `/office-hours` asking the human a question looked like "task complete"
5. **Dashboard usage race condition** — fast poll overwriting slow poll data

None of these would have been found by running the agents manually. They only surfaced because tutti orchestrated the agents through the full workflow with real permissions, real artifact handoff, and real health monitoring.

This is the meta-lesson: **the orchestrator is a test harness for itself.** When your framework runs your framework's development, every deficiency in the framework shows up as a deficiency in the development process. You can't work around it because the workaround IS the framework.

We fixed all 5 bugs, re-ran the pipeline, and the implementer shipped 976 lines across 22 files. The tester reviewed it cold and found 5 more issues (different kind — code quality, not framework bugs). That's the system working: framework bugs surface during dogfooding, code quality bugs surface during review. Different agents, different contexts, different failure modes caught.

## Production evidence: Ramp's self-maintaining system

In March 2026, Ramp published how they built a self-maintaining system for Ramp Sheets. Their findings are the strongest production-scale evidence for the sequential, scoped-agent thesis — and they arrived at the same architecture independently, at a scale we can't claim yet.

### Their unfocused agent failed for the same reason swarms fail

Ramp's first attempt was a nightly QA agent that ran a broad pass over the entire codebase. It found real bugs — but it kept reading the same files, investigating the same features, running the same tests. Their diagnosis:

> "Current frontier models are very capable at a wide range of software engineering tasks, but they cannot synthesize a large codebase with a large observability surface and determine what needs attention. Prioritization at production scale requires a level of intelligence surpassing that of any model available today."

This is the context pollution problem, stated by an engineering team running it against a production codebase with real users. A single agent with a broad mandate and a big context window can't prioritize. The signal-to-noise ratio in a large codebase overwhelms the model's attention.

Their solution was the same as ours: **don't give the agent the whole codebase.** Give it a specific alert, a specific diff, a specific mission.

### Monitor → webhook → agent → fix → PR is a sequential pipeline

Ramp's production architecture:

1. On PR merge, an agent reads the diff and generates monitors for the new code
2. When a monitor fires, a Datadog webhook kicks off a new agent with the alert context
3. The agent reproduces the issue in a sandbox
4. The agent pushes a fix and notifies Slack

This is sequential workflow with artifact handoff. The monitor is the artifact from step 1. The alert context is the artifact from step 2. The sandbox reproduction is the verification gate. The PR is the output artifact. Each agent gets exactly the context it needs for its specific job — not a dump of the entire codebase.

No swarm coordination. No agents polling each other. No shared memory. Just: trigger → scoped context → action → output.

### Their noise problem is the coordination problem

Ramp's biggest challenge was noise. Auto-generated monitors had bad thresholds. Routine user activity triggered cascades of false positives. Duplicate alerts for the same issue flooded Slack.

Their fix: a triage agent that runs before the fix agent. If it's noise, the triage agent tunes or deletes the monitor. If it's real, the fix agent proceeds.

This is another artifact boundary — the triage step filters noise before it reaches the agent that does expensive work. The same pattern as tutti's artifact-polling: don't let the downstream agent see raw signal. Filter it first.

And critically: the triage agent and the fix agent are separate. The triage agent is optimized for classification (is this real?). The fix agent is optimized for reproduction and remediation. Ramp even found that different models are better at different jobs: "GPT-5 model series are very thorough debuggers, [but] Opus 4.6 was a more accurate triage evaluator." Different brains for different roles — exactly the role mapping pattern.

### 1,000 narrow monitors beat 10 broad ones

Ramp scaled from 10 hand-written monitors to over 1,000 AI-generated ones — one for every 75 lines of code. Their hand-written monitors were broad: alert when the frontend crashes, or when an API call times out. The generated ones are granular, "acting like a tight mesh over the exact shape of the code."

This is the scoped-context principle at the monitoring layer. A single broad monitor watching everything is the swarm approach — it sees all signals but can't prioritize. A thousand narrow monitors, each watching exactly its scope, is the sequential approach — each one is specialized, and the triage layer handles coordination.

The result: 40 real bugs caught in the first week. Issues detected within minutes of a user triggering them. In one case, their system flagged a bug before the user's Slack message about it even landed.

### What Ramp's numbers tell us

| Metric | Ramp (production) | Tutti (dogfooding) |
|--------|-------------------|-------------------|
| Architecture | Sequential: monitor → triage → fix → PR | Sequential: plan → review → implement → test → ship |
| Context strategy | Scoped per alert | Scoped per role |
| Model diversity | GPT-5 for debugging, Opus for triage | Claude for implementation, Codex for review |
| Noise handling | Triage agent filters before fix agent | Artifact boundary filters between stages |
| Coordination overhead | Zero (webhook-driven) | Zero (artifact-driven) |

Two teams, different scales, different domains (production monitoring vs SDLC automation), same architecture. Not because we copied each other — because the constraints push you here.

When you try the swarm approach, you hit context pollution and coordination overhead. When you try the single-agent approach, you hit the attention and prioritization limit. The architecture that survives is: scoped agents, sequential handoff, typed artifacts, and different models for different jobs.

## Try it

```bash
cargo install tutti
cd your-project
tt init
```

60 seconds to a working agent team. No swarms required.

github.com/nutthouse/tutti
