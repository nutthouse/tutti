# X ARTICLE: Why I Stopped Building Agent Swarms
# Editor notes for Adam — read this before pasting into X article composer

## FORMATTING GUIDE FOR X ARTICLES:
## - X articles support: bold, italic, headers (H1/H2), blockquotes, bullet lists, numbered lists, images, links
## - X articles DO NOT support: code blocks, tables, syntax highlighting, inline code backticks
## - Replace all code blocks with either: screenshots, images, or plain text descriptions
## - Replace all tables with either: bullet lists or images of tables
## - Em dashes (—) replaced throughout with periods, commas, or sentence restructuring
## - Each [IMAGE] tag below needs a generated image. Prompts are provided.

## IMAGE PROMPTS (generate these before pasting):
##
## 1. HERO IMAGE: "A minimal dark terminal showing 5 named AI agents in a pipeline diagram.
##    Clean monospace font, dark background (#1a1b26), green/blue accent colors. The agents
##    are labeled: planner, implementer, tester, reviewer, docs. Arrows flow left to right
##    between them. Small document icons between each arrow represent artifacts. No chrome,
##    no window decorations. Just the pipeline. 16:9 aspect ratio."
##
## 2. CONTEXT POLLUTION: "Split screen comparison. Left side: a messy desk overflowing with
##    papers, sticky notes, coffee cups, old drafts, crumpled rejects. Labeled 'Single agent:
##    200K tokens of everything.' Right side: a clean desk with one crisp document on it.
##    Labeled 'Scoped agent: 20K tokens of exactly what it needs.' Minimal illustration style,
##    muted colors, no people. 16:9."
##
## 3. ARTIFACT FUNNEL: "A funnel diagram. At the top: a wide cloud labeled '50K tokens of
##    planning debate' with messy scribbles. In the middle: the funnel narrows, labeled
##    'Artifact boundary.' At the bottom: a clean rectangle labeled '2K token design doc.'
##    Arrows show noise being filtered out on the sides. Clean, minimal, dark background. 16:9."
##
## 4. COORDINATION SCALING: "A simple graph. X-axis: 'Number of agents' (2, 3, 5, 10, 30).
##    Y-axis: 'Coordination channels needed.' The line curves upward steeply (quadratic).
##    Key data points labeled: 2 agents = 1 channel, 5 agents = 10 channels, 30 agents = 435
##    channels. Clean, minimal chart style, dark background, single accent color. 16:9."
##
## 5. RAMP PIPELINE: "A horizontal pipeline diagram showing: Monitor fires > Triage agent >
##    Fix agent > PR created. Each step is a rounded rectangle. Between each step, a small
##    document icon labeled 'alert context', 'reproduction', 'fix.' Below the pipeline:
##    'Ramp Sheets: 1,000 monitors, 40 bugs caught in week 1.' Dark background, clean style. 16:9."
##
## 6. POSITIONING MAP: "A 2x2 quadrant chart. X-axis: 'Automation level' (low to high).
##    Y-axis: 'Coordination overhead' (low to high). Top-left quadrant: 'Swarms' with a
##    cluster icon. Bottom-right quadrant: 'Sequential pipelines' with an arrow icon.
##    Bottom-left: 'Manual' with a person icon. Clean, minimal, dark background. 16:9."

---
START OF ARTICLE (paste everything below this line into X)
---

# Why I Stopped Building Agent Swarms

Everyone's building agent swarms. Multiple AI agents talking to each other, voting on decisions, passing messages through SQLite mailboxes, coordinating via shared memory. It sounds amazing in a demo. It falls apart in production.

I know because I tried it. Then I built the opposite.

[IMAGE 1: Hero - pipeline diagram]

## The question nobody asks

Before comparing swarms vs sequential, there's a more fundamental question. Why run multiple agents at all?

Claude has a 1M token context window. That's roughly 750,000 words, more than the entire Lord of the Rings trilogy twice over. You could paste your entire codebase into a single conversation. So why split the work across separate agents, each with their own context?

Because a bigger window doesn't mean a better one.

## The context pollution problem

Here's what happens when you ask a single agent to plan, implement, test, review, and document a feature.

By step 3, the context is polluted. The agent is carrying the entire planning conversation. The rejected approaches, the back-and-forth on scope, the design alternatives you didn't pick. When it starts implementing, all of that planning debris is sitting in context, competing for attention with the actual code it needs to write.

By step 4, the context contains the implementation discussion too. The debugging tangents, the "actually let me try a different approach" pivots, the dead-end explorations. The agent isn't just reading your code anymore. It's reading through a history of everything it tried, including the things that failed.

This is the dirty secret of large context windows. More context doesn't mean better context. A 200K token conversation where 150K is planning history and debugging tangents is worse than a 20K token conversation that contains exactly the artifact the agent needs to do its job.

[IMAGE 2: Context pollution - messy desk vs clean desk]

LLMs have attention. Attention is finite. When the context is full of noise, the signal gets diluted, even if the window is technically big enough to hold everything. Research on "lost in the middle" shows that models struggle to attend to information buried in long contexts, even when it's there.

## Why separate agents win: context specialization

Each agent in our system starts with a clean context optimized for its specific job.

**The planner** gets the project's conventions file, the design prompt, and the human's answers to design questions. Nothing else. No implementation details. No test output. No debugging history.

When the planner produces a design doc, that document is a distilled artifact. The planning conversation compressed into its conclusions. The rejected approaches, the back-and-forth, the tangents are gone. What remains is the decision.

**The implementer** gets the design doc (conclusions only, not the process), the test plan, and a clean git worktree with the current codebase. It never sees the planning debate. It doesn't know there were three approaches considered. It doesn't carry the 50K tokens of design conversation. It sees the artifact ("build this") and builds it in a clean context where 100% of its attention is on the code.

**The tester** gets the test plan and the implementer's diff. A clean context with zero implementation history. The tester has never seen the implementer struggle with a tricky type error for 30 minutes. It has no sympathy for the implementation. It reviews the code cold, the way a real reviewer would, with fresh eyes and no sunk-cost bias.

This is the core insight. Separate agents aren't about parallelism. They're about context hygiene.

## The artifact handoff is the key mechanism

The thing that makes this work isn't agents. It's the artifact boundary between them.

When the planner finishes, it produces a design doc. That doc is a lossy compression of the planning conversation. And the lossiness is the feature. All the noise, tangents, and rejected approaches are discarded. Only the signal survives the boundary.

[IMAGE 3: Artifact funnel - 50K tokens narrowing to 2K]

Each artifact boundary is a noise filter. The implementer doesn't need to know why you chose approach B over approach A. It needs to know what approach B is. The design doc tells it exactly that and nothing more.

This is why swarms fail. They skip the artifact boundary. When agents share a message bus or a SQLite mailbox, they're sharing raw conversation: unfiltered, uncompressed, full of noise. Agent A's debugging tangent becomes Agent B's context pollution. The shared memory that's supposed to enable coordination becomes a vector for attention dilution.

## Git worktrees: physical isolation, not just logical

Each agent runs in its own git worktree. This isn't just about preventing file conflicts (though it does that too). It's about giving each agent a physically separate view of the codebase.

The implementer's worktree diverges from main as it works. The tester's worktree stays on main until the implementer's changes are ready for review. The docs agent never sees half-finished implementation code.

Each agent's file tools see exactly the files relevant to their job. The implementer sees the code it's changing. The tester sees the finished diff. The reviewer sees the PR-ready state. Nobody sees work-in-progress files from another agent's half-finished task.

Compare this to swarms: 5 agents sharing one checkout, each reading files the others are actively editing. Agent A reads a file, starts planning changes. Agent B edits that file while A is thinking. A's plan is now based on a stale file. This isn't a theoretical problem. It's the primary failure mode of parallel agent systems.

## Scoped prompts: telling agents what NOT to think about

Each agent gets a system prompt that defines its role and, critically, its scope. The implementer is scoped to source code. The tester is scoped to source code and tests. The planner doesn't touch code at all. It produces documents.

This is the opposite of a single-agent approach where one context does everything. A single agent with a big context window has no natural boundary between "planning mode" and "implementation mode" and "testing mode." It drifts. It starts fixing a test while it was supposed to be implementing a feature. It refactors code while it was supposed to be reviewing a PR.

Separate agents can't drift because they don't have access to drift into. The implementer can't accidentally start writing docs. It doesn't see the docs directory. The tester can't accidentally start implementing. It receives the diff as a read-only artifact.

## The coordination trap

Here's what happens when you run 5 agents on the same codebase simultaneously.

They stomp on each other's files. Agent A edits a file while Agent B is reading it. Agent C commits a change that breaks Agent B's work. You spend more tokens on coordination than on actual coding.

One competitor's own internal docs admit this honestly: "swarms often perform worse than a single focused agent." The agents spend tokens polling a mail bus, resolving conflicts, and re-reading context that changed since they last looked. I call it coordination theater. It looks like productivity, but it's overhead pretending to be work.

[IMAGE 4: Coordination scaling graph]

The coordination tax scales quadratically. 2 agents need 1 coordination channel. 3 need 3. 5 need 10. 30 agents (as one tool claims to run) need 435 coordination channels. Every token spent resolving a merge conflict or polling a message bus is a token not spent writing code.

Sequential workflows have zero coordination channels. Agent A finishes, produces an artifact, Agent B starts. The coordination cost is constant: one artifact handoff per step.

## The idle detection problem

The one hard problem with sequential workflows isn't coordination. It's knowing when an agent is done.

When an interactive skill asks the human a question, the agent goes idle. The terminal stops changing. The health monitor sees "idle" and thinks "task complete," then moves to the next step. But the skill isn't done. It's waiting for the human.

This tripped us up during dogfooding. The planner ran an interactive design session, the skill asked the human a design question, and the orchestrator immediately advanced to the next step because the terminal output stopped changing.

We solved this with artifact-polling. Instead of watching for idle, the system watches for the output file. The agent can sit there waiting for human input for an hour. When the design doc file appears on disk, the orchestrator captures it and continues.

Simple. No agent-to-agent communication protocol. No message bus. Just: did the file appear? Yes, next step.

## The role mapping insight

One more thing that falls out of the multi-agent model: you can swap the brain behind each role without changing the workflow.

The reviewer runs on Codex (OpenAI). Everything else runs on Claude. Why? Because code review benefits from a different model's perspective, the same way a human code review benefits from fresh eyes. The reviewer literally has a different neural network evaluating the code.

Change one line in the config and the implementer runs on Codex too. Every agent using that role switches runtime. No per-agent edits. This is impossible with a single-agent approach. You can't run part of a conversation on Claude and part on GPT within one context window.

## What we learned building this: 7 lessons from dogfooding

Our framework is built by itself. The orchestrator orchestrates its own development. We call them "choir runs." Every lesson below came from a real failure during dogfooding, not from theory.

**1. Agents that review their own code are worse than agents that review cold.**

During our first end-to-end run, we had the implementer agent review its own PR. It approved everything. Of course it did. It wrote the code. It had 100K tokens of context explaining why every decision was the right one.

We split review into a separate agent with a different model. The tester received only the diff and the test plan. No implementation history. No debugging tangents.

The result was dramatic. The tester agent (running cold on the implementer's output) found 5 real issues: a missing workflow in a template, non-deterministic iteration in a display function, incomplete test coverage, a formatting inconsistency, and a misplaced import. The implementer would never have caught these. It wrote the code knowing the intent, so the gaps were invisible to it.

Context isolation isn't a limitation. It's an audit mechanism.

**2. The planner produces better designs when it can't see the code.**

This surprised us. Early on, the planner agent had full access to the source code. It would read through the implementation, form opinions about what was hard to change, and produce conservative designs that worked around existing code rather than improving it.

When we scoped the planner to docs only and gave it project conventions plus the design prompt, the designs got bolder. The planner stopped thinking about implementation difficulty. That wasn't its problem. It started thinking about what the user actually needs.

Agents with less context make bolder decisions. When you can't see the constraints, you don't design around them.

**3. Worktree isolation caught a real shipping bug.**

The implementer agent committed its work in its own worktree, based on the main branch at an older version. But the feature we'd just built was on a different branch. When we ran template validation tests in the implementer's worktree, they failed. The template used a config option that was only valid in our unreleased code.

If the implementer had been working in the same checkout as us, this mismatch would have been invisible. The tests would have passed against our local code, and the PR would have broken CI when merged to main.

Worktree isolation doesn't just prevent file conflicts. It prevents environmental contamination.

**4. Permission scoping reveals hidden dependencies.**

When we first ran an interactive design session through the orchestrator, it failed. The skill's preamble needed Bash access. Then the next skill failed because it needed the Skill tool. Then it failed again because it needed the AskUserQuestion tool.

Each permission failure revealed a hidden dependency. A single agent running with all permissions bypassed would never have discovered these requirements. Everything would "just work" until you tried to run it securely.

Separate agents with scoped permissions act as a dependency scanner.

**5. The artifact boundary caught a data race we didn't design for.**

The dashboard had a bug: usage stats would flash with real values then disappear, replaced by zeros. Two polling loops were writing to the same DOM element. The fast 2-second poll included zeros (because it didn't request usage data). The slow 30-second poll set the real values. The fast poll overwrote them every 2 seconds.

This is a coordination race. Two processes sharing mutable state with no synchronization. Exactly the kind of bug that swarms amplify across 30 agents sharing a message bus.

The fix was an artifact boundary: the server now omits usage data from the response unless explicitly requested. The fast poll can't overwrite what it doesn't send.

**6. "Agent went idle" is fundamentally unsolvable with observation alone.**

We tried three approaches to detect when an agent is done:

- Pane hash stability: watch the terminal, detect when it stops changing. Fails when the agent is waiting for human input (idle but not done).
- Output file detection: watch for a specific file to appear. Works for non-interactive steps but not for skills that produce artifacts at unpredictable paths.
- Artifact-glob polling: watch for new files matching a pattern. The agent can take as long as it needs. The orchestrator checks every 5 seconds.

Approach 3 was the breakthrough. It decouples "agent is working" from "step is complete." The signal isn't the agent's behavior. It's the artifact's existence.

Observe artifacts, not agents. An agent's terminal state is unreliable. The presence of its output file is binary and unambiguous.

**7. The orchestrator developing itself surfaced 5 framework bugs in one session.**

During one dogfood run, we discovered and fixed five bugs: a permission syntax conflict, a missing tool in the allowlist, another missing tool, an idle detection false positive, and a dashboard race condition.

None of these would have been found by running the agents manually. They only surfaced because the orchestrator ran the agents through the full workflow with real permissions, real artifact handoff, and real health monitoring.

The meta-lesson: the orchestrator is a test harness for itself. When your framework runs your framework's development, every deficiency shows up as a deficiency in the development process. You can't work around it because the workaround IS the framework.

## Production evidence: Ramp's self-maintaining system

In March 2026, Ramp published how they built a self-maintaining system for Ramp Sheets. Their findings are the strongest production-scale evidence for the sequential, scoped-agent thesis. They arrived at the same architecture independently, at a scale we can't claim yet.

[IMAGE 5: Ramp pipeline diagram]

**Their unfocused agent failed for the same reason swarms fail.**

Ramp's first attempt was a nightly QA agent that ran a broad pass over the entire codebase. It found real bugs. But it kept reading the same files, investigating the same features, running the same tests. Their diagnosis:

> "Current frontier models are very capable at a wide range of software engineering tasks, but they cannot synthesize a large codebase with a large observability surface and determine what needs attention. Prioritization at production scale requires a level of intelligence surpassing that of any model available today."

This is the context pollution problem, stated by an engineering team running it against a production codebase with real users. A single agent with a broad mandate and a big context window can't prioritize. The signal-to-noise ratio overwhelms the model's attention.

Their solution was the same as ours. Don't give the agent the whole codebase. Give it a specific alert, a specific diff, a specific mission.

**Their architecture is a sequential pipeline.**

On PR merge, an agent reads the diff and generates monitors. When a monitor fires, a webhook kicks off a new agent with the alert context. The agent reproduces the issue in a sandbox. Then it pushes a fix and notifies the team.

Sequential workflow with artifact handoff. The monitor is the artifact from step 1. The alert context is the artifact from step 2. The sandbox reproduction is the verification gate. The PR is the output. Each agent gets exactly the context it needs. Not a dump of the entire codebase.

No swarm coordination. No agents polling each other. No shared memory.

**They use different models for different jobs.**

Ramp found that "GPT-5 model series are very thorough debuggers, [but] Opus 4.6 was a more accurate triage evaluator." Different brains for different roles. Exactly the pattern we use: Claude for implementation, Codex for review.

**1,000 narrow monitors beat 10 broad ones.**

Ramp scaled from 10 hand-written monitors to over 1,000 AI-generated ones. One for every 75 lines of code. Their hand-written monitors were broad: alert when the frontend crashes, or when an API times out. The generated ones are granular, "acting like a tight mesh over the exact shape of the code."

The result: 40 real bugs caught in the first week. Issues detected within minutes of a user triggering them. In one case, their system flagged a bug before the user's Slack message about it even landed.

## The positioning map

[IMAGE 6: Positioning quadrant]

Different quadrants, different bets. Swarms bet that coordination complexity is worth the overhead. Sequential workflows bet that structured repeatability ships more code.

I'm betting on sequential. Not because swarms can't work, but because the marginal cost of coordination is higher than the marginal cost of waiting for the previous step to finish. And because every artifact boundary is a free noise filter that makes the next agent smarter.

**The best agent isn't the one with the biggest context. It's the one with the cleanest context.**

## The numbers

Last week we ran the full SDLC pipeline end-to-end:

- 50 commits to main
- 17,200 lines of code
- 7 releases (v0.3.0 to v0.9.0)
- 8 consecutive active days
- 84% of commits AI-assisted

Zero agent coordination conflicts. Zero merge resolution. Zero tokens spent on agents talking to each other.

The artifact pipeline handled the handoff: design doc to test plan to implementation to review to ship. Each agent received exactly the context it needed, nothing more.

60 seconds to a working agent team. No swarms required.

github.com/nutthouse/tutti
