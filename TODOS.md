# TODOS

## Agent Framework

### Atomic file writes
**Priority:** P2
Write tool and apply_patch tool use `std::fs::write` which truncates on crash.
Switch to write-to-tempfile + rename pattern for crash-safe writes.
**Depends on:** None
**Context:** Identified in adversarial review of agentic spine (2026-04-13).

### Shell env scrubbing for air-gapped mode
**Priority:** P2
Shell tool inherits tutti's full environment including API keys. When
`network_boundary = "air-gapped"`, env_clear() + allowlist (PATH, HOME, LANG)
would prevent secrets from leaking via `env` or credential helpers.
**Depends on:** None
**Context:** Codex adversarial review. The network_boundary blocklist is
best-effort, not a security boundary. Real containment needs OS-level controls.

### apply_patch mixed line ending support
**Priority:** P2
Current normalization picks one style (CRLF or LF) based on first occurrence.
Files with mixed endings (common after bad git merges) may fail to match.
Try both variants before reporting "not found."
**Depends on:** None

### Context window management
**Priority:** P1
Agent loop currently fails with error when conversation exceeds context window.
Post-spine: implement context compaction (summarize older messages).
**Depends on:** Agent loop wired into CLI (Week 2)

### Anthropic Messages API adapter
**Priority:** P1
Second provider adapter. The internal Vec<ContentBlock> format already handles
Anthropic's block-based response format.
**Depends on:** None

### Wire tt run --direct into CLI
**Priority:** P0
Add Direct variant to ResolvedStep in automation/mod.rs. Parse [providers.*]
and [workflow.steps.policy] from tutti.toml. Add --direct flag to tt run.
**Depends on:** Agent loop + tools (done), automation pipeline investigation

### tt replay command
**Priority:** P1
Read SQLite event log and reconstruct the decision trail for a given run_id.
Human-readable and --json output modes.
**Depends on:** tt run --direct wired up

## Completed
