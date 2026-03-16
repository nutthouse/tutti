#!/usr/bin/env bash
set -euo pipefail

# Select next GitHub issue for automated SDLC cycle.
# Usage: select_issue.sh [output_json] [label]

OUT_FILE="${1:-.tutti/state/auto/selected_issue.json}"
LABEL="${2:-agent-ops}"
REPO="${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}"

mkdir -p "$(dirname "$OUT_FILE")"

JSON=$(gh issue list --repo "$REPO" --state open --label "$LABEL" --limit 100 \
  --json number,title,url,labels,author,createdAt)

ISSUE_NUM=$(python3 - "$OUT_FILE" "$LABEL" "$JSON" <<'PY'
import json,sys
out, label, raw = sys.argv[1], sys.argv[2], sys.argv[3]
items = json.loads(raw or "[]")
items = [
    i for i in items
    if "automation-claimed" not in {l.get("name") for l in i.get("labels", [])}
]
if not items:
    raise SystemExit(f"No unclaimed open issues found for label '{label}'")
items.sort(key=lambda i: i.get("createdAt") or "")
issue = items[0]
payload = {
    "issue_number": issue["number"],
    "title": issue["title"],
    "url": issue["url"],
    "labels": [l.get("name") for l in issue.get("labels", [])],
    "author": issue.get("author", {}).get("login"),
    "created_at": issue.get("createdAt"),
}
with open(f"{out}.tmp", "w", encoding="utf-8") as f:
    json.dump(payload, f, indent=2)
print(issue["number"])
PY
)

gh issue edit "$ISSUE_NUM" --repo "$REPO" --add-label "automation-claimed" >/dev/null

# Generate a unique run ID for claim tracking.
RUN_ID="${GITHUB_RUN_ID:-local-$(date +%s)-$$}"
LEASE_TTL="${CLAIM_LEASE_TTL:-1800}"

ISSUE_DETAILS=$(gh issue view "$ISSUE_NUM" --repo "$REPO" --json body)
python3 - "$OUT_FILE" "$ISSUE_DETAILS" "$RUN_ID" "$LEASE_TTL" "$REPO" <<'PY'
import json,sys
from datetime import datetime, timezone
out, details_raw = sys.argv[1], sys.argv[2]
run_id, lease_ttl, repo = sys.argv[3], int(sys.argv[4]), sys.argv[5]
with open(f"{out}.tmp", "r", encoding="utf-8") as f:
    payload = json.load(f)
details = json.loads(details_raw or "{}")
payload["body"] = (details.get("body") or "").strip()
# Embed claim metadata into the selected issue state.
now = datetime.now(timezone.utc).isoformat()
payload["claim"] = {
    "run_id": run_id,
    "claimed_at": now,
    "renewed_at": now,
    "lease_ttl_secs": lease_ttl,
    "repo": repo,
}
with open(f"{out}.tmp", "w", encoding="utf-8") as f:
    json.dump(payload, f, indent=2)
PY

mv "${OUT_FILE}.tmp" "$OUT_FILE"

# Persist claim lease for the Rust-side sweeper / auto-release.
CLAIMS_DIR="$(dirname "$OUT_FILE")/../../state/claims"
mkdir -p "$CLAIMS_DIR"
python3 - "$CLAIMS_DIR" "$ISSUE_NUM" "$RUN_ID" "$LEASE_TTL" "$REPO" <<'PY'
import json,sys,os
from datetime import datetime, timezone
claims_dir, issue_num = sys.argv[1], sys.argv[2]
run_id, lease_ttl, repo = sys.argv[3], int(sys.argv[4]), sys.argv[5]
now = datetime.now(timezone.utc).isoformat()
lease = {
    "issue_number": int(issue_num),
    "repo": repo,
    "run_id": run_id,
    "claimed_at": now,
    "renewed_at": now,
    "lease_ttl_secs": lease_ttl,
}
path = os.path.join(claims_dir, f"{issue_num}.json")
with open(path, "w", encoding="utf-8") as f:
    json.dump(lease, f, indent=2)
PY

# Post audit comment on the issue.
gh issue comment "$ISSUE_NUM" --repo "$REPO" --body "🤖 **Claim acquired** by run \`$RUN_ID\` — lease ${LEASE_TTL}s" >/dev/null 2>&1 || true

echo "$OUT_FILE"
