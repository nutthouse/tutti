# VERSIONING.md

Tutti uses **SemVer** (`MAJOR.MINOR.PATCH`) for Cargo/package releases and
tags. The root `VERSION` file keeps the same release version with a fourth
coordination slot (`MAJOR.MINOR.PATCH.MICRO`) for gstack ship queues.

## Default rule (per merged issue PR)

Every merged issue PR must include:

- [ ] `Cargo.toml` version bump
- [ ] `VERSION` bump
- [ ] `CHANGELOG.md` entry
- [ ] release impact noted in PR description

If a PR does **not** change behavior (docs/chore only), explicitly mark it as **no-version-bump** in PR notes.

---

## Bump policy

### PATCH (`v0.2.x -> v0.2.x+1`)
Use for:
- bug fixes
- reliability hardening
- internal orchestration improvements
- non-breaking CLI/workflow behavior refinements

### MINOR (`v0.x -> v0.(x+1).0`)
Use for:
- new user-visible capability
- new CLI commands/subcommands
- workflow contract changes that operators must adapt to
- autonomy milestone releases (meaningfully stronger unattended operation)

### MAJOR (`v1.x -> v2.0.0`)
Use for:
- breaking API/CLI contract changes
- incompatible config/workflow format changes

---

## v0.3.0 trigger (agreed current policy)
Move to `v0.3.0` when all are true:

1. `#28` shipped (stale review gate handling)
2. `#30` shipped (state machine + run ledger)
3. `#35` and/or `#37` shipped (permission/dry-run preflight usability)
4. At least one full issue completed unattended with no manual rescue

Until then, continue with patch bumps on issue delivery.

---

## Tagging policy

- Prefer **tag per merged issue PR** once CI is green on `main`.
- Tag format: `vX.Y.Z`
- Tag must reference `origin/main` merge commit for the release.
- `Cargo.toml` must equal `X.Y.Z`.
- `VERSION` must equal `X.Y.Z` or `X.Y.Z.0`.

Example:

```bash
git fetch origin --tags
# bump Cargo.toml + VERSION + CHANGELOG in PR first
# after merge:
git checkout main
git pull --ff-only

git tag v0.10.1
git push origin v0.10.1
```

If tags are batched, document which issues are included in the release notes.

## Automated release workflow

`.github/workflows/release.yml` runs on `vX.Y.Z` tag pushes and on manual
dispatch for an existing tag. It performs the release gate in one place:

- validates the tag, `Cargo.toml`, and `VERSION` agree
- extracts release notes from `CHANGELOG.md`
- runs `cargo fmt --all -- --check`
- runs `cargo check --locked`
- runs `cargo clippy --locked -- -D warnings`
- runs `cargo test --locked`
- runs `cargo package --locked`
- builds `tt` archives for Linux x86_64, macOS arm64, and macOS x86_64
- uploads SHA-256 checksums for each archive
- creates or updates the GitHub Release
- publishes to crates.io when `CARGO_REGISTRY_TOKEN` is configured

If `CARGO_REGISTRY_TOKEN` is not configured, the GitHub Release still publishes
and the crates.io step is skipped with an explicit note.

## Release checklist

1. Merge the release PR into `main`.
2. Wait for `CI` and `CodeQL` on `main` to pass.
3. Create the tag from the merge commit on `main`.
4. Push the tag.
5. Watch the `Release` workflow.
6. Confirm the GitHub Release contains all platform archives and checksums.
7. Confirm `cargo install tutti --version X.Y.Z` works once crates.io publish
   has completed.
