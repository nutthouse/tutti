# VERSIONING.md

Tutti uses **SemVer** (`MAJOR.MINOR.PATCH`).

## Default rule (per merged issue PR)

Every merged issue PR must include:

- [ ] `Cargo.toml` version bump
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

Example:

```bash
git fetch origin --tags
# bump Cargo.toml + CHANGELOG in PR first
# after merge:
git checkout main
git pull --ff-only

git tag v0.2.4
git push origin v0.2.4
```

If tags are batched, document which issues are included in the release notes.
