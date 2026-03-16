#!/usr/bin/env python3
import json
import subprocess
import sys
from pathlib import Path

branch_file = Path(sys.argv[1] if len(sys.argv) > 1 else ".tutti/state/auto/branch.json")

with branch_file.open("r", encoding="utf-8") as f:
    branch = json.load(f)["branch"]

subprocess.check_call(["git", "fetch", "origin", "main"])
log = subprocess.check_output(["git", "log", "--oneline", f"origin/main..{branch}"], text=True).strip()
if not log:
    print("No commits found on automation branch vs origin/main")
    sys.exit(1)

print(log.splitlines()[0])
