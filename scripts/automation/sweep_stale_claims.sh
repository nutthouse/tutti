#!/usr/bin/env bash
set -euo pipefail

# Sweep stale claim leases whose TTL has expired.
# Usage: sweep_stale_claims.sh [claims_dir]

CLAIMS_DIR="${1:-.tutti/state/claims}"

if [ ! -d "$CLAIMS_DIR" ]; then
  echo "No claims directory at $CLAIMS_DIR — nothing to sweep."
  exit 0
fi

REPO="${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}"

python3 - "$CLAIMS_DIR" "$REPO" <<'PY'
import json, sys, os, subprocess
from datetime import datetime, timezone

claims_dir, repo = sys.argv[1], sys.argv[2]
released = 0

for fname in os.listdir(claims_dir):
    if not fname.endswith(".json"):
        continue
    path = os.path.join(claims_dir, fname)
    try:
        with open(path) as f:
            lease = json.load(f)
    except (json.JSONDecodeError, OSError):
        continue

    renewed_at = datetime.fromisoformat(lease["renewed_at"].replace("Z", "+00:00"))
    ttl = lease.get("lease_ttl_secs", 1800)
    now = datetime.now(timezone.utc)
    elapsed = (now - renewed_at).total_seconds()

    if elapsed <= ttl:
        remaining = int(ttl - elapsed)
        print(f"  active: issue #{lease['issue_number']} (run={lease['run_id']}, {remaining}s remaining)")
        continue

    issue_num = lease["issue_number"]
    run_id = lease.get("run_id", "unknown")
    expired_ago = int(elapsed - ttl)
    print(f"  stale: issue #{issue_num} (run={run_id}, expired {expired_ago}s ago) — releasing")

    # Remove label.
    subprocess.run(
        ["gh", "issue", "edit", str(issue_num), "--repo", repo, "--remove-label", "automation-claimed"],
        capture_output=True,
    )
    # Post audit comment.
    subprocess.run(
        ["gh", "issue", "comment", str(issue_num), "--repo", repo,
         "--body", f"🤖 **Claim released** — reason: lease expired (sweeper, run `{run_id}`)"],
        capture_output=True,
    )
    # Remove claim file.
    os.remove(path)
    released += 1

if released == 0:
    print("sweep: no stale claims found")
else:
    print(f"sweep: released {released} stale claim(s)")
PY
