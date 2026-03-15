#!/usr/bin/env python3
import json
import subprocess
import sys
from pathlib import Path

selected_issue_file = Path(sys.argv[1] if len(sys.argv) > 1 else ".tutti/state/auto/selected_issue.json")
branch_file = Path(sys.argv[2] if len(sys.argv) > 2 else ".tutti/state/auto/branch.json")
base = sys.argv[3] if len(sys.argv) > 3 else "main"

with selected_issue_file.open("r", encoding="utf-8") as f:
    issue = json.load(f)
with branch_file.open("r", encoding="utf-8") as f:
    branch = json.load(f)["branch"]

issue_number = issue["issue_number"]
title = f"[auto] #{issue_number} {issue['title']}"
body = (
    f"Automated SDLC cycle for #{issue_number}.\n\n"
    "- planner: completed\n"
    "- implementation: completed\n"
    "- tests: updated\n"
    "- docs/changelog: updated\n"
    "- version: bumped if required"
)

existing = subprocess.check_output(
    ["gh", "pr", "list", "--state", "open", "--head", branch, "--json", "number"],
    text=True,
)
prs = json.loads(existing)
if prs:
    print(f"PR already exists: #{prs[0]['number']}")
else:
    subprocess.check_call([
        "gh",
        "pr",
        "create",
        "--title",
        title,
        "--body",
        body,
        "--head",
        branch,
        "--base",
        base,
    ])
