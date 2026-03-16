#!/usr/bin/env bash
set -euo pipefail

# Release the automation-claimed label from the selected issue.
# Usage: release_claim.sh [selected_issue_json] [reason]

ISSUE_FILE="${1:-.tutti/state/auto/selected_issue.json}"
REASON="${2:-workflow completed}"

if [ ! -f "$ISSUE_FILE" ]; then
  echo "No selected issue file at $ISSUE_FILE — nothing to release." >&2
  exit 0
fi

REPO="${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}"

ISSUE_NUM=$(python3 -c "import json,sys; print(json.load(open(sys.argv[1]))['issue_number'])" "$ISSUE_FILE")

if [ -z "$ISSUE_NUM" ]; then
  echo "Could not read issue_number from $ISSUE_FILE" >&2
  exit 1
fi

# Remove the automation-claimed label (tolerate it already being absent).
gh issue edit "$ISSUE_NUM" --repo "$REPO" --remove-label "automation-claimed" 2>/dev/null || true

# Extract run_id from claim metadata if available.
RUN_ID=$(python3 -c "
import json,sys
d = json.load(open(sys.argv[1]))
print(d.get('claim', {}).get('run_id', 'unknown'))
" "$ISSUE_FILE" 2>/dev/null || echo "unknown")

# Post audit comment.
gh issue comment "$ISSUE_NUM" --repo "$REPO" \
  --body "🤖 **Claim released** — reason: ${REASON} (run \`${RUN_ID}\`)" \
  >/dev/null 2>&1 || true

# Remove claim state file.
CLAIM_FILE=".tutti/state/claims/${ISSUE_NUM}.json"
rm -f "$CLAIM_FILE" 2>/dev/null || true

echo "Released claim on issue #${ISSUE_NUM}: ${REASON}"
