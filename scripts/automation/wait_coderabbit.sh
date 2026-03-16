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
    exit 1
  fi

  DATA=$(gh pr view "$PR_NUMBER" --repo "$REPO" --json statusCheckRollup)
  RESULT=$(python3 - <<'PY' "$DATA"
import json,sys
obj=json.loads(sys.argv[1])
checks=obj.get("statusCheckRollup") or []
cr=[]
for c in checks:
    name=(c.get("name") or "").lower()
    if "coderabbit" in name or "code rabbit" in name:
        cr.append(c)
if not cr:
    print("NONE")
    raise SystemExit
# if multiple, require all completed and explicitly successful
states=[(c.get("status") or "", c.get("conclusion") or "") for c in cr]
if any(s != "COMPLETED" for s,_ in states):
    print("PENDING")
else:
    allowed=("SUCCESS",)
    bad=[x for x in states if x[1] not in allowed]
    print("FAIL" if bad else "PASS")
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
