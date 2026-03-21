# Remote Demo Script

This is the truthful demo script for the current product shape.

It is optimized for a short public clip:

1. phone/browser shows a live Tutti dashboard on a VPS
2. a trigger is sent from the phone
3. agents pick up the work
4. a PR appears with code, tests, and review

## Current Truth

- This script assumes [#90](https://github.com/nutthouse/tutti/pull/90) is merged.
- This script assumes [#98](https://github.com/nutthouse/tutti/pull/98) is merged.
- The trigger path is the new generic webhook endpoint, not native GitHub webhook setup yet.
- The remote story is "browser + tunnel + optional SSH shell", not full transparent `TUTTI_REMOTE` client parity.

That is still enough for the marketing claim if we phrase it honestly:

> I run my agent team on a cheap VPS, watch it from my phone, send it work remotely, and get back a reviewed PR.

## Demo Goal

Show this sequence in about 30 seconds:

- remote dashboard is live
- multiple agents are already running
- a phone sends a work trigger
- Tutti starts the SDLC loop
- later, a PR exists with green checks

## Preflight

Before recording, make sure all of this is already true:

- the VPS has a fresh clone on `main`
- GitHub auth is working on the VPS (`gh auth status`)
- required agent CLIs are authenticated on the VPS
- `tt up` already launches the team cleanly
- `tt serve --remote` works and prints a bearer token
- the dashboard is reachable through a tunnel
- the demo issue already exists in GitHub
- PR [#98](https://github.com/nutthouse/tutti/pull/98) is merged so `/v1/webhooks/generic` exists

## Demo Config

Use a small, explicit trigger in `tutti.toml`:

```toml
[observe]
dashboard = true
port = 4040

[[webhook]]
source = "generic"
events = ["issue_url_submitted"]
workflow = "sdlc-auto"
```

This is intentionally simple. For the demo, the webhook only needs to start the workflow.

## One-Time Setup

On the VPS:

```bash
tt up --mode unattended --policy bypass
tt serve --remote --port 4040
```

Keep the `tt serve` process running. It will print a bearer token like:

```text
serve: bearer token: <TOKEN>
```

From your laptop:

```bash
tt remote attach <your-vps-host> 4040
```

That gives you a local tunnel to the VPS. The dashboard will be available at:

```text
http://127.0.0.1:4040/
```

For the phone shot, either:

- open the tunneled dashboard via a device/browser you can screen-capture, or
- expose the VPS through your normal secure tunnel/proxy setup and open that URL on the phone

## Trigger Command

The generic webhook endpoint expects JSON like this:

```json
{
  "source": "generic",
  "event": "issue_url_submitted",
  "workspace": "tutti"
}
```

Use this exact command shape:

```bash
curl -X POST http://127.0.0.1:4040/v1/webhooks/generic \
  -H "Authorization: Bearer <TOKEN>" \
  -H "Content-Type: application/json" \
  -d '{
    "source": "generic",
    "event": "issue_url_submitted",
    "workspace": "tutti"
  }'
```

If you want the phone to "send a GitHub issue URL", use a Shortcut or tiny form that hits the same endpoint and includes the URL in the JSON body for the camera moment:

```json
{
  "source": "generic",
  "event": "issue_url_submitted",
  "workspace": "tutti",
  "issue_url": "https://github.com/nutthouse/tutti/issues/86"
}
```

The current phase-1 webhook implementation does not require `issue_url`, but it makes the demo legible on camera.

## 30-Second Public Cut

### Shot 1: Establish the factory (0:00-0:08)

Screen:

- show the dashboard on phone or browser
- keep the agent lane visible
- let the viewer see multiple agents are already alive

Optional line:

> This is my agent team running on a cheap VPS.

### Shot 2: Send work from the phone (0:08-0:15)

Screen:

- open the trigger Shortcut, mobile form, or terminal snippet
- send the webhook with the issue URL visible if possible

Optional line:

> I can send it work remotely from my phone.

### Shot 3: Watch the orchestra react (0:15-0:22)

Screen:

- cut back to the dashboard
- show states changing from idle to working
- let the viewer see the system wake up

Optional line:

> Planner picks it up, then implementation, tests, docs, review.

### Shot 4: Payoff (0:22-0:30)

Screen:

- cut to GitHub PR list or the PR page
- show the new PR
- keep green checks visible if available

Optional line:

> A little later, it opens a reviewed PR with tests and docs.

## Rehearsal Runbook

Use this exact flow before recording:

1. Start `tt up` on the VPS.
2. Start `tt serve --remote --port 4040` on the VPS.
3. Open the dashboard and confirm it renders.
4. Confirm the bearer token works with `curl /v1/health`.
5. Send one dry-run webhook from the laptop.
6. Watch the workflow start in the dashboard.
7. Confirm a PR appears.
8. Confirm GitHub Actions checks go green.
9. Reset to a clean issue before recording the public take.

## Camera Notes

- Keep the dashboard zoomed so the agent states are readable on a phone screen.
- Do not spend time typing long commands on camera. Use a Shortcut, clipboard snippet, or tiny form.
- Prefer one clean issue over a surprise live coding branch. The goal is legibility, not maximum difficulty.
- If the PR takes too long, cut from trigger to PR after a short time jump. The claim is asynchronous autonomy, not real-time code generation in 10 seconds.

## Honest Narration

Use wording like this:

> I run my agent team on a VPS, watch it from my phone, and trigger work remotely. When it finishes, I get a PR back.

Avoid wording like this for now:

> I text GitHub and it fully understands any issue URL automatically.

That stronger claim should wait for native GitHub webhook setup and a cleaner issue-ingest story.
