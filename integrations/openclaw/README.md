# OpenClaw Integration Bundle

This folder contains a ready-to-run wrapper that exposes Tutti actions in a single JSON contract for OpenClaw-style agent orchestrators.

## Files

- `tutti_openclaw.py`: executable action wrapper.
- `action-contract.json`: action names, args, and output envelope contract.

## Prerequisites

- `tt` installed and on `PATH`
- workspace has `tutti.toml`
- Python 3 available (`python3`)

## Quickstart

Run from a Tutti workspace root:

```bash
# Show available actions and args
python3 integrations/openclaw/tutti_openclaw.py --help

# Preflight environment and tool requirements
python3 integrations/openclaw/tutti_openclaw.py doctor_check

# Discover workflows
python3 integrations/openclaw/tutti_openclaw.py list_workflows

# Preview a workflow plan
python3 integrations/openclaw/tutti_openclaw.py plan_workflow verify --strict

# Execute workflow and verification with JSON output
python3 integrations/openclaw/tutti_openclaw.py run_workflow verify --strict
python3 integrations/openclaw/tutti_openclaw.py verify_team --strict

# Read latest verify summary and agent states
python3 integrations/openclaw/tutti_openclaw.py read_verify_status
python3 integrations/openclaw/tutti_openclaw.py team_status
```

If your installed `tt` is older than this repo, pin the wrapper to local source:

```bash
python3 integrations/openclaw/tutti_openclaw.py \
  --tt-bin "cargo run --quiet --" \
  doctor_check
```

## JSON Envelope

Every action emits this envelope:

```json
{
  "ok": true,
  "action": "verify_team",
  "command": ["tt", "verify", "--json", "--strict"],
  "exit_code": 0,
  "data": {},
  "stdout": "",
  "stderr": "",
  "parse_error": null,
  "note": null
}
```

Notes:
- `data` is populated for actions backed by Tutti JSON output.
- `stdout`/`stderr` always include raw command output.
- `parse_error` is present if a command was expected to return JSON but parsing failed.
