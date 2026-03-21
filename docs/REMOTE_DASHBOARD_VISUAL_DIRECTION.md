# Remote Dashboard Visual Direction

## Goal

`#84` should not ship as a generic admin panel. The dashboard should feel like a live factory floor for autonomous software work: practical first, but with enough motion and systems legibility that it is satisfying to watch.

This is the visual/product direction for the remote demo path:

1. Phone or browser opens the dashboard
2. User sees a live multi-agent software factory
3. Work enters the system and visibly moves through planner -> implementer -> tester -> reviewer -> docs/release
4. The operator can immediately spot bottlenecks, failures, and throughput

## Core Product Metaphor

Treat agents as machines in a production line, not chat avatars.

- Planner, implementer, tester, reviewer, and docs-release are "stations"
- Runs are "work items" moving through the line
- Retries loop back visibly
- Failures jam a lane
- Healthy systems feel like a humming factory

The user should feel:
"I am operating a software factory."

Not:
"I am reading another monitoring dashboard."

## Default View

The primary landing view should be a **Factory Floor**.

Layout:

- Top bar: workspace, remote host, health, active runs, PR yield, token/cost burn
- Center canvas: stage-to-stage flow map
- Right rail or lower sheet: selected run details / latest event timeline
- Bottom strip: agent health and queue depth summary

The first screen should answer these questions in under 3 seconds:

- Which agents are alive?
- What is currently moving?
- Where is the bottleneck?
- Is anything jammed or waiting on humans?

## Visual Language

### 1. Flow-first

The center of the UI should show movement:

- work items travel along lanes between stages
- stage occupancy is visible
- queue buildup is visible
- retries visibly route backward or into a retry loop

### 2. Machine states

Each agent/stage should have a strong state treatment:

- `working`: bright / animated / pulsing
- `idle`: dim but ready
- `stopped`: subdued / offline
- `blocked`: amber or red, with visible jam marker
- `auth_failed` / `provider_down`: power-loss or disconnected-system feel

### 3. Throughput over decoration

Use motion to communicate system state, not for ornament.

Good:

- moving work items
- pulsing active stations
- queue depth growth
- failure jams and recovery loops

Bad:

- decorative gradients with no meaning
- random glow everywhere
- generic "AI dashboard" chrome

## Information Hierarchy

The order of importance should be:

1. Live flow of work
2. Bottlenecks and failures
3. Operator actions
4. Detailed logs and event history

This means:

- tables are secondary
- raw JSON is tertiary
- status cards support the flow view instead of replacing it

## MVP For `#84`

The first shippable version does not need full terminal-in-browser to be compelling.

### Must-have

- Factory Floor view as the default homepage
- Live stage graph for planner -> implementer -> tester -> reviewer -> docs-release
- SSE-backed updates
- Stage state chips and queue depth
- Recent event timeline
- Run selection with focused details
- Mobile-friendly layout

### Should-have

- Animated work-item transitions
- Retry loop visualization
- "jammed" visual treatment for failed runs
- Compact throughput HUD:
  - active runs
  - completed runs today
  - median cycle time
  - blocked count

### Can-wait

- full xterm.js terminal embedding
- fancy auth UX beyond what remote serve needs
- upload buttons
- rich editing from the browser

## Mobile Direction

This dashboard must read cleanly on a phone because the demo story depends on it.

Mobile should not be a shrunk desktop grid.

Use:

- vertical stage stack or compressed horizontal swimlane
- tap-to-expand run detail drawer
- large state indicators
- one-thumb actions only

The user should be able to glance at the dashboard while walking away from their laptop and immediately know whether the factory is healthy.

## Operator Delight

The Factorio-like appeal comes from clarity plus satisfying systems motion.

Add a few deliberate moments:

- completed PRs visibly exit the line as shipped goods
- retries loop back through a visible return lane
- when all stations are healthy, the floor has a steady, calm "hum"
- when something fails, the break in flow is obvious

Keep this tasteful. The effect should feel "industrial and alive," not gamey.

## Anti-Slop Rules

Avoid these common failures:

- generic observability cards as the whole product
- agent avatars / chatbot framing
- oversaturated neon cyberpunk styling
- motion without information value
- burying the main flow below logs and side panels

If a screenshot could be mistaken for any random SaaS admin panel, the design missed the point.

## Suggested Interaction Model

Primary interaction:

- tap or click a stage to inspect active work there

Secondary interaction:

- tap or click a run to view:
  - current step
  - recent events
  - failure / retry history
  - linked PR or issue

Tertiary interaction:

- send actions or prompts from contextual controls, not from a giant omnibox-first design

## Demo Success Criteria

The dashboard is "good enough" for the marketing demo when a viewer can understand all of this without narration:

- multiple agents are alive on a remote machine
- work enters the system and gets routed automatically
- the system visibly processes it through specialized stages
- a PR comes out the other end

If the UI makes that obvious in 5 to 10 seconds, it is doing its job.
