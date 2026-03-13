# Changelog

## 1.0.0 (2026-03-14)

Initial release.

- 16 actions covering the full Tutti agent lifecycle
- JSON envelope on all outputs for reliable machine parsing
- Direct state file reads for status (no output parsing)
- Workflow discovery, planning (dry-run), and execution
- Handoff packet generation, application, and listing
- Permission policy checks
- Configurable `tt` binary path via `--tt-bin` flag or `TUTTI_BIN` env var
