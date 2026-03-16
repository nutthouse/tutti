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
mv "${OUT_FILE}.tmp" "$OUT_FILE"

echo "$OUT_FILE"
