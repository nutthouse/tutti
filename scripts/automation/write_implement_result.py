#!/usr/bin/env python3
import json
import subprocess
import sys
from pathlib import Path


def run_git(args: list[str]) -> str:
    result = subprocess.run(
        ["git", *args],
        text=True,
        capture_output=True,
        check=True,
    )
    return result.stdout.strip()


def main() -> int:
    if len(sys.argv) != 4:
        print(
            "usage: write_implement_result.py <selected_issue.json> <branch.json> <output.json>",
            file=sys.stderr,
        )
        return 2

    selected_issue = json.loads(Path(sys.argv[1]).read_text())
    branch_info = json.loads(Path(sys.argv[2]).read_text())
    output_path = Path(sys.argv[3])

    target_branch = branch_info["branch"]
    base_branch = branch_info.get("base_branch", "main")
    base_sha = branch_info.get("base_sha")

    subprocess.run(
        ["git", "fetch", "origin", base_branch],
        text=True,
        check=True,
        capture_output=True,
    )

    if not base_sha:
        base_sha = run_git(["rev-parse", f"origin/{base_branch}"])

    commit_sha = run_git(["rev-parse", "HEAD"])
    is_ancestor = subprocess.run(
        ["git", "merge-base", "--is-ancestor", base_sha, "HEAD"],
        text=True,
        capture_output=True,
        check=False,
    ).returncode == 0
    if not is_ancestor:
        print(
            f"HEAD is not based on baseline {base_sha}",
            file=sys.stderr,
        )
        return 1

    if commit_sha == base_sha:
        print(
            f"HEAD commit does not advance beyond origin/{base_branch}",
            file=sys.stderr,
        )
        return 1

    progress_log = run_git(["log", "--oneline", f"{base_sha}..HEAD"])
    if not progress_log:
        print(
            f"No commits found between branch baseline {base_sha} and HEAD",
            file=sys.stderr,
        )
        return 1

    changed_files = [
        line
        for line in run_git(
            ["diff", "--name-only", f"{base_sha}..HEAD"]
        ).splitlines()
        if line.strip()
    ]
    if not changed_files:
        print("HEAD commit does not contain any changed files", file=sys.stderr)
        return 1

    subprocess.run(
        ["git", "push", "origin", f"HEAD:{target_branch}"],
        text=True,
        check=True,
        capture_output=True,
    )

    payload = {
        "issue_number": selected_issue["issue_number"],
        "issue_title": selected_issue["title"],
        "branch": target_branch,
        "commit_sha": commit_sha,
        "changed_files": changed_files,
        "verification_commands": [],
        "blocked_reason": None,
    }

    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(payload, indent=2) + "\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
