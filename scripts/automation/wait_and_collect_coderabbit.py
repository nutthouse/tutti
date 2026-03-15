#!/usr/bin/env python3
import json
import subprocess
import sys
from pathlib import Path

branch_file = Path(sys.argv[1] if len(sys.argv) > 1 else ".tutti/state/auto/branch.json")
wait_mins = sys.argv[2] if len(sys.argv) > 2 else "45"
status_file = sys.argv[3] if len(sys.argv) > 3 else ".tutti/state/auto/coderabbit_status.json"
feedback_file = sys.argv[4] if len(sys.argv) > 4 else ".tutti/state/auto/coderabbit-feedback.md"

with branch_file.open("r", encoding="utf-8") as f:
    branch = json.load(f)["branch"]

out = subprocess.check_output(
    ["gh", "pr", "list", "--state", "open", "--head", branch, "--json", "number"],
    text=True,
)
pr = json.loads(out)[0]["number"]

wait_result = subprocess.run(
    ["scripts/automation/wait_coderabbit.sh", str(pr), str(wait_mins), status_file],
    check=False,
)
subprocess.check_call([
    "scripts/automation/collect_coderabbit_feedback.sh",
    str(pr),
    feedback_file,
])

sys.exit(wait_result.returncode)
