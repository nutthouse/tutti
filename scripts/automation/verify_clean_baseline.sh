#!/usr/bin/env bash
set -euo pipefail

# Verify that an agent worktree has a clean baseline matching branch.json.
# Usage: verify_clean_baseline.sh [branch_json_path]

# Safety: must be in an agent worktree
TOPLEVEL=$(git rev-parse --show-toplevel)
case "$TOPLEVEL" in
  */.tutti/worktrees/*) ;;
  *) echo "FATAL: verify_clean_baseline.sh must run inside an agent worktree, not $TOPLEVEL" >&2; exit 1 ;;
esac

# Resolve branch.json — try argument as-is, then relative to project root
BRANCH_JSON="${1:-.tutti/state/auto/branch.json}"
if [ ! -f "$BRANCH_JSON" ]; then
  ROOT=$(dirname "$(git rev-parse --git-common-dir)")
  BRANCH_JSON="$ROOT/$BRANCH_JSON"
fi
if [ ! -f "$BRANCH_JSON" ]; then
  echo "FATAL: branch.json not found at $BRANCH_JSON" >&2
  exit 1
fi

BASE_SHA=$(python3 -c "import json; print(json.load(open('$BRANCH_JSON'))['base_sha'])")
HEAD_SHA=$(git rev-parse HEAD)

if [ "$HEAD_SHA" != "$BASE_SHA" ]; then
  echo "FATAL: HEAD ($HEAD_SHA) != base_sha ($BASE_SHA)" >&2
  exit 1
fi

DIRTY=$(git status --porcelain)
if [ -n "$DIRTY" ]; then
  echo "FATAL: worktree not clean:" >&2
  echo "$DIRTY" >&2
  exit 1
fi

echo "Baseline verified: HEAD=$HEAD_SHA, clean=true"
