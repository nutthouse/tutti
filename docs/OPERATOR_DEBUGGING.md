# Operator Debugging Guide

When an unattended workflow fails, this guide walks you from the failure notification to the root cause.

## The Debugging Loop

```
Run fails → identify step → check agent → read logs → fix → retry
    │            │               │             │
    ▼            ▼               ▼             ▼
tt runs     tt runs <id>    tt attach     tt logs
            (step timeline)  tt peek      events.jsonl
```

## Step 1: Find the Failed Run

```bash
tt runs
```

This shows all recent runs with status (success/failed), workflow name, and timestamp. Find the failed run and note its run ID.

## Step 2: Inspect the Failed Step

```bash
tt runs <run-id>
```

This shows the step-by-step timeline with:
- Step index and type (prompt, command, ensure_running)
- Agent responsible
- Duration
- Status (success/failed/timed_out)
- Failure message

Look for the first failed step — that's where the pipeline broke.

## Step 3: Check the Agent

```bash
tt status                    # overview of all agents
tt peek <agent>              # last N lines of terminal output
tt attach <agent>            # connect to the agent's tmux session
```

If the dashboard is running (`tt serve`), click the failed agent's stage card to enter **Agent Focus Mode** — you'll see the live terminal output, usage stats, and any code changes the agent made.

## Step 4: Read the Logs

```bash
tt logs                      # recent agent logs
```

For deeper inspection, check the event history:

```bash
cat .tutti/state/events.jsonl | grep <agent-name> | tail -20
```

Workflow step intents and outcomes are stored per-run:

```bash
ls .tutti/state/workflow-intents/
ls .tutti/state/workflow-outputs/
```

## Common Failure Patterns

### Provider Failure (auth expired, rate limited)

**Symptoms**: `agent.auth_failed` or `agent.rate_limited` events. Step fails with "auth_failed" or times out.

**Diagnosis**:
```bash
tt status          # look for "auth-fail" or "blocked" state
tt peek <agent>    # check terminal for auth error messages
```

**Fix**: Refresh credentials (`claude auth login`), check API key validity, or wait for rate limit to clear. The `tt serve` probe loop will auto-detect recovery via `agent.auth_recovered` events.

### Policy Failure (command blocked)

**Symptoms**: Step fails with "policy" in the message. The `policy-decisions.jsonl` log shows a denied action.

**Diagnosis**:
```bash
cat .tutti/state/policy-decisions.jsonl | tail -5
```

**Fix**: Review the policy configuration in `tutti.toml` under `[permissions]`. Adjust command allowlists or switch to a less restrictive mode for the failing step.

### Tool/Implementation Failure (no commit produced)

**Symptoms**: `implement_code` step fails with "completed without a commit beyond the branch baseline."

**Diagnosis**:
```bash
# Check the implementer's worktree
cd .tutti/worktrees/implementer
git log --oneline -5        # any new commits?
git status                  # uncommitted changes?
git diff                    # what was the agent working on?
```

**Common causes**:
- Issue scope too large for one slice — the planner's `first_slice` was too ambitious
- Agent ran out of time exploring the codebase (increase `startup_grace_secs` in `tutti.toml`)
- Worktree on wrong branch from a prior failed run (clean with `tt down --all` and re-run)

**Fix**: For large issues, break them into smaller issues. For timeout issues, increase `startup_grace_secs` on the `implement_code` step in `tutti.toml` (default: 30s, recommend: 120s for complex tasks).

### Timeout Failure

**Symptoms**: Step status shows `timed_out: true`.

**Diagnosis**: The agent was still working when `wait_timeout_secs` expired.

**Fix**: Increase `wait_timeout_secs` for the failing step in `tutti.toml`, or reduce the scope of work the agent is asked to do.

## Using the Dashboard for Debugging

With `tt serve --port 4040` running, open `http://localhost:4040` in your browser:

1. **Factory floor**: see which agents are working/idle/stopped/blocked at a glance
2. **Click a stage card**: zoom into Agent Focus Mode to see:
   - Live terminal output (what the agent is doing right now)
   - Token usage (how much context has been consumed)
   - Git diff (what code changes the agent has made)
   - Context health % (is the agent running out of context window?)
3. **Send a prompt**: type in the prompt bar to give the agent additional instructions
4. **Event timeline**: scroll the bottom panel to see recent events across all agents

## Resuming a Failed Run

After fixing the root cause:

```bash
tt run --resume <run-id>
```

This picks up from the failed step, skipping already-completed steps. The agent sessions are preserved — no need to re-boot the choir.
