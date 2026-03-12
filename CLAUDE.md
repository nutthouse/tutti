# Tutti — Project Conventions

## Language & Tooling
- Rust (stable), single crate, binary name `tt`
- No async in Phase 1 — all tmux interaction is synchronous `std::process::Command`
- Dependencies: clap (derive), serde + toml, thiserror, comfy-table, colored, chrono, which

## Architecture
- `src/cli/` — Clap command handlers, one file per subcommand
- `src/config/` — TOML config types and parsing
- `src/runtime/` — RuntimeAdapter trait + per-runtime implementations
- `src/session/` — tmux session management wrappers
- `src/worktree/` — Git worktree lifecycle
- `src/state/` — .tutti/ directory and agent state persistence
- `src/error.rs` — Central error types via thiserror

## Naming
- Musical terminology: voices (running agents), arrangements (configs), movements (phases), phrases (prompts)
- tmux sessions: `tutti-{team}-{agent}`
- Git branches for worktrees: `tutti/{agent}`
- State files: `.tutti/state/{agent}.json`

## Testing
- Unit tests in each module (`#[cfg(test)] mod tests`)
- Integration tests that need tmux should check for its presence first
- Use `tempfile` or `std::env::temp_dir()` for filesystem tests

## Error Handling
- All public functions return `Result<T, TuttiError>`
- Use `thiserror` for error variants, not anyhow
- User-facing errors should include actionable guidance

## Style
- `cargo fmt` and `cargo clippy` must pass (CI enforced)
- Prefer explicit imports over glob imports
- Keep functions focused — if it's doing two things, split it
