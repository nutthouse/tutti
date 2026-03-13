# Changelog

## 1.1.0 (2026-03-14)

- Add `send_prompt` action with `--auto-up`, `--wait`, and `--output` flags
- Add `land_agent` action wrapping `tt land` with `--pr` and `--force` support
- Document 5 new workflow step types: `ensure_running`, `workflow` (nested), `land`, `review`, and `inject_files` on prompt steps
- Document workflow composition patterns for autonomous fix loops
- 18 total actions (up from 16)

## 1.0.0 (2026-03-14)

Initial release.

- 16 actions covering the full Tutti agent lifecycle
- JSON envelope on all outputs for reliable machine parsing
- Direct state file reads for status (no output parsing)
- Workflow discovery, planning (dry-run), and execution
- Handoff packet generation, application, and listing
- Permission policy checks
- Configurable `tt` binary path via `--tt-bin` flag or `TUTTI_BIN` env var
