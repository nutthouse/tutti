#!/usr/bin/env bash
set -euo pipefail

# Create a feature branch from selected issue JSON.
# Usage: create_issue_branch.sh [selected_issue_json] [output_json]

# Safety: refuse to run destructive commands outside an agent worktree
TOPLEVEL=$(git rev-parse --show-toplevel)
case "$TOPLEVEL" in
  */.tutti/worktrees/*) ;;
  *) echo "FATAL: create_issue_branch.sh must run inside an agent worktree, not $TOPLEVEL" >&2; exit 1 ;;
esac

PROJECT_ROOT="${TOPLEVEL%/.tutti/worktrees/*}"
ISSUE_JSON="${1:-$PROJECT_ROOT/.tutti/state/auto/selected_issue.json}"
OUT_FILE="${2:-$PROJECT_ROOT/.tutti/state/auto/branch.json}"
BASE_BRANCH="${BASE_BRANCH:-main}"

mkdir -p "$(dirname "$OUT_FILE")"

clean_worktree_except_target() {
  git clean -ffdx -e target/
}

assert_clean_baseline_except_target() {
  local dirty
  dirty=$(git status --porcelain --ignored | grep -vE '^(\?\?|!!) target(/|$)' || true)
  if [ -n "$dirty" ]; then
    echo "FATAL: worktree is not clean after reset:" >&2
    echo "$dirty" >&2
    exit 1
  fi
}

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

# Pre-clean: discard any carried state from the current branch
git reset --hard HEAD
clean_worktree_except_target

# Switch to the automation branch from a clean baseline
git checkout -B "$BRANCH" "origin/$BASE_BRANCH"

# Post-clean: guarantee working tree matches origin exactly, ignoring busy build output.
git reset --hard "origin/$BASE_BRANCH"
clean_worktree_except_target

BASE_SHA=$(git rev-parse HEAD)

# Assert clean baseline, allowing concurrent build output in target/.
assert_clean_baseline_except_target

python3 - <<'PY' "$OUT_FILE" "$BRANCH" "$ISSUE_NUM" "$BASE_BRANCH" "$BASE_SHA"
import json,sys
out, branch, issue = sys.argv[1], sys.argv[2], int(sys.argv[3])
base_branch, base_sha = sys.argv[4], sys.argv[5]
with open(out, 'w', encoding='utf-8') as f:
    json.dump({
        "branch": branch,
        "issue_number": issue,
        "base_branch": base_branch,
        "base_sha": base_sha,
    }, f, indent=2)
print(out)
PY
