# Design System — Tutti

## Product Context
- **What this is:** Multi-agent orchestration CLI with a web dashboard for monitoring and controlling AI coding agents
- **Who it's for:** Technical solo power users running fleets of AI agents on local machines or VPS
- **Space/industry:** Developer tools, AI coding infrastructure, observability
- **Project type:** Dashboard (embedded web UI served by a Rust binary)
- **Visual metaphor:** Factory floor / control room — agents are machines, work items flow through stages, bottlenecks and jams are immediately visible. Inspired by Factorio and Gene Kim's factory analogy in The Phoenix Project.

## Aesthetic Direction
- **Direction:** Industrial/Utilitarian — function-first, data-legible, purposeful motion that communicates system state. Not the clean SaaS look (Linear/Vercel). Think factory control room: dense but organized, every element earns its space.
- **Decoration level:** Intentional — subtle texture through borders and surface elevation, not gradients or glow. Stage connectors use line style (dashed/solid/animated) to communicate flow state.
- **Mood:** Calm competence. The feeling of watching a well-tuned system run. When healthy, the dashboard hums. When something breaks, the disruption is obvious without being alarming.
- **Reference sites:** Vercel (sidebar navigation, mobile-first), Linear (dark-mode-first, LCH color innovation), Grafana (data density, observability patterns). Tutti diverges from all three with the factory-floor metaphor and single-view layout.

## Typography
- **Display/Hero:** Instrument Serif — the "tutti" wordmark and h1-level section headers only. Distinctive serif in a space where every peer uses sans-serif. Restricted usage prevents it from feeling editorial.
- **Body/UI:** DM Sans — clean geometric sans, readable at small sizes on mobile. Shared with tutti.dev for brand coherence. Distinctive enough not to read as Linear (Inter) or Vercel (Geist).
- **UI/Labels:** DM Sans (same as body)
- **Data/Tables:** JetBrains Mono — event logs, agent names, timestamps, metrics. Tabular figures built in. The standard for developer tools.
- **Code:** JetBrains Mono
- **Loading:** Google Fonts CDN — `Instrument+Serif`, `DM+Sans:wght@400;500;600;700`, `JetBrains+Mono:wght@400;500;600`
- **Scale:**
  - xs: 11px / 0.6875rem (labels, timestamps)
  - sm: 13px / 0.8125rem (body small, event rows)
  - base: 16px / 1rem (body)
  - lg: 20px / 1.25rem (section subheads)
  - xl: 24px / 1.5rem (page headers)
  - 2xl: 32px / 2rem (section titles — Instrument Serif)
  - 3xl: 48px / 3rem (hero wordmark — Instrument Serif)

## Color
- **Approach:** Restrained — state colors (green/amber/red) carry the product communication. The accent color (indigo) is reserved for interactive/chrome elements only and should never compete with state colors.
- **Background:** #0a0f1e — tutti.dev brand navy. Deep enough for long viewing sessions, not pure black.
- **Surface:** #111827 — elevated cards and panels
- **Surface Elevated:** #1a2236 — hover states, selected items
- **Accent (interactive only):** #6366f1 — buttons, links, focus rings. NOT for system state.
- **Working:** #22c55e — the "factory is humming" signal. Green means the machine is running.
- **Idle:** #6b7280 — dim but present. The station exists, it's just waiting.
- **Blocked/Error:** #ef4444 — unmissable. A jam in the production line.
- **Warning:** #f59e0b — attention needed but not critical. Rate limits, long idle times.
- **Text primary:** #f9fafb
- **Text secondary:** #9ca3af
- **Text dim:** #6b7280
- **Border:** #1f2937
- **Border active:** #374151
- **Dark mode:** Dark-only. No light mode. Dev dashboards are dark. Don't fight it.

### State Color Hierarchy
State colors are the primary visual communication system. In order of visual weight:
1. **Blocked (red)** — highest priority, demands immediate attention
2. **Working (green)** — the healthy default, the steady hum
3. **Warning (amber)** — secondary attention signal
4. **Idle (gray)** — low priority, present but passive
5. **Accent (indigo)** — interactive affordance only, never system state

## Spacing
- **Base unit:** 4px
- **Density:** Comfortable — tighter than Linear (which optimizes for elegance), looser than Grafana (which optimizes for data packing). The factory floor needs to show 5-6 agent stations + HUD + events without scrolling on desktop.
- **Scale:** 2xs(2px) xs(4px) sm(8px) md(16px) lg(24px) xl(32px) 2xl(48px) 3xl(64px)

## Layout
- **Approach:** Grid-disciplined — strict zones on desktop (HUD top, factory floor center, events/detail bottom). On phone, collapses to vertical stack.
- **No sidebar.** This is a single-view factory floor, not a multi-page app. Maximum screen real estate for the flow visualization.
- **Grid:** Single column on mobile (<768px). On desktop, factory floor spans full width; HUD and events use flexible columns.
- **Max content width:** None — the factory floor uses available width.
- **Border radius:** sm: 4px (badges, small chips), md: 6px (cards, buttons), lg: 8px (stations, panels). No fully rounded elements — the aesthetic is industrial, not playful.
- **Phone-first:** Vertical stage stack on mobile. Large touch targets (min 44px). One-thumb actions. State readable at arm's length.

## Motion
- **Approach:** Intentional — motion equals information. No decorative animation.
- **Working pulse:** 2s ease-in-out infinite on working stations. Subtle green glow oscillation. This creates the "humming factory" feel.
- **State transitions:** 150-250ms ease-out. Idle→Working fades green in. Working→Blocked flashes red border once to draw the eye.
- **Easing:** enter(ease-out) exit(ease-in) move(ease-in-out)
- **Duration:** micro(50-100ms) short(150-250ms) medium(250-400ms) long(400-700ms)
- **Anti-patterns:** No entrance animations on page load. No hover glow that doesn't communicate state. No loading spinners that obscure content. No transition on every CSS property — only animate what changed.

## Dashboard-Specific Guidance

### HUD Metrics
Dashboard metrics should surface operational state, not vanity counters:
- Good: "current bottleneck," "blocked count," "waiting on human," "current run stage"
- Bad: "total events," "uptime percentage," generic counters

### Information Hierarchy
1. Live flow of work (center canvas)
2. Bottlenecks and failures (state colors, jammed visualization)
3. Operator-relevant metrics (HUD)
4. Event timeline (supporting detail)
5. Selected-run/selected-stage detail (on interaction)

### Agent Focus Mode (The Bigger IDE)
Click a stage card → factory floor fades, full-screen agent view appears. The Factorio zoom-in.

**Layout**: terminal pane (70% width) + sidebar (30%) + fixed prompt bar at bottom.
- Terminal: `--bg` background, `--font-mono` 12px, 1.6 line-height. Tool calls in `--working` green, prompts in `--accent` indigo.
- Sidebar sections (top to bottom, most→least dynamic): Usage → Changes → Progress.
- Prompt bar: 48px height, matches dispatch panel styling.
- Context % fill bar: `--working` green (≤70%), `--auth-fail` amber (70-90%), `--blocked` red (>90%).

**Mobile (<768px)**: terminal full-width, sidebar sections in horizontally swipeable tabs (44px touch targets), prompt bar fixed at bottom with 16px font (prevents iOS zoom).

**Transitions**: enter with `ease-out` 250ms opacity, exit with `ease-in` 200ms. Smart auto-scroll: terminal stays at bottom unless user scrolled up.

**Empty states**: "Agent is not running." with green "Start Agent" CTA button. "No changes yet." for empty diff. "—" for missing usage data.

### Anti-Slop Rules
- No generic observability cards as the whole product
- No agent avatars or chatbot framing
- No oversaturated neon cyberpunk styling
- No motion without information value
- No burying the flow view below logs and side panels
- No 3-column feature grids with icons in colored circles
- No gradient buttons as primary CTA pattern
- If a screenshot could be mistaken for any random SaaS admin panel, the design missed the point

## Decisions Log
| Date | Decision | Rationale |
|------|----------|-----------|
| 2026-03-21 | Initial design system created | /design-consultation based on TuttiWorks identity + competitive research (Vercel, Linear, Grafana) |
| 2026-03-21 | Instrument Serif for display | Risk: no dev dashboard uses serif. Gain: distinctive, authoritative, breaks from Linear-clone look |
| 2026-03-21 | No sidebar | Risk: breaks dashboard convention. Gain: max screen real estate for factory floor, matches single-view product |
| 2026-03-21 | Green for working (not accent) | Risk: accent feels secondary. Gain: factory metaphor demands green=running, natural state/action separation |
| 2026-03-21 | Dark-only, no light mode | Safe choice: every dev dashboard is dark. Users expect it. Simplifies implementation. |
| 2026-03-21 | Accent for interactive only | Feedback: indigo was competing with state colors. Accent reserved for buttons/links/focus, never system state. |
