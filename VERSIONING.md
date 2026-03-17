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

## Post-v0.3.0 policy (current)

`v0.3.0` shipped on 2026-03-17 and is tagged as `v0.3.0`.

Current guidance:

- Continue with patch bumps on merged issue PRs unless the change clearly adds a new user-visible capability or breaks an existing contract.
- Docs/chore-only PRs remain `no-version-bump`.
- Treat the remaining `#30` productization work as part of the `v0.4.0` milestone scope, not as a retroactive gate for `v0.3.0`.

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
