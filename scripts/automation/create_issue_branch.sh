#!/usr/bin/env bash
set -euo pipefail

# Create a feature branch from selected issue JSON.
# Usage: create_issue_branch.sh [selected_issue_json] [output_json]

ISSUE_JSON="${1:-.tutti/state/auto/selected_issue.json}"
OUT_FILE="${2:-.tutti/state/auto/branch.json}"
BASE_BRANCH="${BASE_BRANCH:-main}"

mkdir -p "$(dirname "$OUT_FILE")"

ISSUE_NUM=$(python3 - <<'PY' "$ISSUE_JSON"
import json,sys
with open(sys.argv[1], 'r', encoding='utf-8') as f:
    d=json.load(f)
print(d["issue_number"])
PY
)

STAMP=$(date +%Y%m%d%H%M%S)
BRANCH="auto/issue-${ISSUE_NUM}-${STAMP}"

git fetch origin "$BASE_BRANCH"
git checkout -B "$BRANCH" "origin/$BASE_BRANCH"

python3 - <<'PY' "$OUT_FILE" "$BRANCH" "$ISSUE_NUM"
import json,sys
out, branch, issue = sys.argv[1], sys.argv[2], int(sys.argv[3])
with open(out, 'w', encoding='utf-8') as f:
    json.dump({"branch": branch, "issue_number": issue}, f, indent=2)
print(out)
PY
