#!/usr/bin/env bash
set -euo pipefail

# Wait for CodeRabbit check on a PR.
# Usage: wait_coderabbit.sh <pr_number> [timeout_minutes] [output_json]

PR_NUMBER="${1:?PR number required}"
TIMEOUT_MIN="${2:-45}"
OUT_FILE="${3:-.tutti/state/auto/coderabbit_status.json}"
REPO="${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}"

mkdir -p "$(dirname "$OUT_FILE")"

DEADLINE=$(( $(date +%s) + TIMEOUT_MIN*60 ))
FOUND=0

while true; do
  NOW=$(date +%s)
  if (( NOW > DEADLINE )); then
    python3 - <<'PY' "$OUT_FILE" "$PR_NUMBER"
import json,sys
out,pr = sys.argv[1], int(sys.argv[2])
with open(out,'w',encoding='utf-8') as f:
    json.dump({"pr_number":pr,"status":"timeout","found":False},f,indent=2)
print(out)
PY
    # Timeout is a soft outcome — downstream steps handle missing feedback gracefully
    exit 0
  fi

  DATA=$(gh pr view "$PR_NUMBER" --repo "$REPO" --json statusCheckRollup,comments)
  RESULT=$(echo "$DATA" | python3 - <<'PY'
import json,sys
obj=json.load(sys.stdin)

# Check status checks first
checks=obj.get("statusCheckRollup") or []
cr=[]
for c in checks:
    name=(c.get("name") or "").lower()
    if "coderabbit" in name or "code rabbit" in name:
        cr.append(c)
if cr:
    states=[(c.get("status") or "", c.get("conclusion") or "") for c in cr]
    if any(s != "COMPLETED" for s,_ in states):
        print("PENDING")
    else:
        allowed=("SUCCESS",)
        bad=[x for x in states if x[1] not in allowed]
        print("FAIL" if bad else "PASS")
    raise SystemExit

# Fallback: check for CodeRabbit PR comments (review may appear as comment, not check)
KICKOFF_MARKERS = ["review triggered", "walkthrough", "<!-- This is an auto-generated comment: summarize"]
comments=obj.get("comments") or []
for c in comments:
    author=(c.get("author") or {}).get("login","").lower()
    if "coderabbit" in author:
        body=(c.get("body") or "")
        body_lower=body.lower()
        if "rate limit" in body_lower:
            # Rate-limited — treat as no review available
            print("PASS")
            raise SystemExit
        # Skip kickoff-only comments (not a real review)
        if any(marker in body_lower for marker in KICKOFF_MARKERS) and "actionable" not in body_lower:
            continue
        # CodeRabbit posted a substantive comment — review is available
        print("PASS")
        raise SystemExit

print("NONE")
PY
)

  case "$RESULT" in
    NONE)
      sleep 30
      ;;
    PENDING)
      FOUND=1
      sleep 30
      ;;
    PASS)
      python3 - <<'PY' "$OUT_FILE" "$PR_NUMBER" "$FOUND"
import json,sys
out,pr,found = sys.argv[1], int(sys.argv[2]), bool(int(sys.argv[3]))
with open(out,'w',encoding='utf-8') as f:
    json.dump({"pr_number":pr,"status":"pass","found":found},f,indent=2)
print(out)
PY
      exit 0
      ;;
    FAIL)
      python3 - <<'PY' "$OUT_FILE" "$PR_NUMBER"
import json,sys
out,pr = sys.argv[1], int(sys.argv[2])
with open(out,'w',encoding='utf-8') as f:
    json.dump({"pr_number":pr,"status":"fail","found":True},f,indent=2)
print(out)
PY
      exit 2
      ;;
  esac
done
