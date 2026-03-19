#!/usr/bin/env python3
import json
import subprocess
import sys
from pathlib import Path


def git_output(args: list[str]) -> str:
    result = subprocess.run(
        ["git", *args],
        text=True,
        capture_output=True,
        check=True,
    )
    return result.stdout.strip()


def ref_exists(ref: str) -> bool:
    result = subprocess.run(
        ["git", "rev-parse", "--verify", ref],
        text=True,
        capture_output=True,
        check=False,
    )
    return result.returncode == 0


branch_file = Path(sys.argv[1] if len(sys.argv) > 1 else ".tutti/state/auto/branch.json")

with branch_file.open("r", encoding="utf-8") as f:
    branch = json.load(f)["branch"]

subprocess.run(["git", "fetch", "origin", "main"], check=True, capture_output=True, text=True)
subprocess.run(
    ["git", "fetch", "origin", branch],
    check=False,
    capture_output=True,
    text=True,
)

branch_ref = f"origin/{branch}" if ref_exists(f"origin/{branch}") else branch
log = git_output(["log", "--oneline", f"origin/main..{branch_ref}"])
if not log:
    print("No commits found on automation branch vs origin/main")
    sys.exit(1)

print(log.splitlines()[0])
