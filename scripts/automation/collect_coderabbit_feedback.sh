#!/usr/bin/env bash
set -euo pipefail

# Collect CodeRabbit review/comments into markdown for follow-up prompt.
# Usage: collect_coderabbit_feedback.sh <pr_number> [output_md]

PR_NUMBER="${1:?PR number required}"
OUT_FILE="${2:-.tutti/state/auto/coderabbit-feedback.md}"
REPO="${GITHUB_REPOSITORY:-$(gh repo view --json nameWithOwner -q .nameWithOwner)}"

mkdir -p "$(dirname "$OUT_FILE")"

DATA=$(gh pr view "$PR_NUMBER" --repo "$REPO" --json comments,reviews,url,title)
INLINE_COMMENTS=$(gh api "repos/$REPO/pulls/$PR_NUMBER/comments")

python3 - <<'PY' "$OUT_FILE" "$DATA" "$INLINE_COMMENTS"
import json,sys,re
out=obj=None
out=sys.argv[1]
obj=json.loads(sys.argv[2])
inline_comments=json.loads(sys.argv[3])

def is_coderabbit(author):
    if not author:
        return False
    login=(author.get("login") or author.get("name") or "").lower()
    return "coderabbit" in login or "code-rabbit" in login or "code_rabbit" in login

lines=[]
lines.append(f"# CodeRabbit feedback for PR #{obj.get('url','').split('/')[-1]}")
lines.append("")
lines.append(f"Title: {obj.get('title','')}")
lines.append(f"URL: {obj.get('url','')}")
lines.append("")

found=False
for review in obj.get("reviews", []):
    if is_coderabbit(review.get("author")):
        found=True
        body=(review.get("body") or "").strip()
        if body:
            lines.append("## Review")
            lines.append(body)
            lines.append("")

for c in obj.get("comments", []):
    if is_coderabbit(c.get("author")):
        found=True
        body=(c.get("body") or "").strip()
        if body:
            lines.append("## Comment")
            lines.append(body)
            lines.append("")

for c in inline_comments:
    # REST PR review comments use `user` instead of `author`
    if is_coderabbit(c.get("user")):
        found=True
        body=(c.get("body") or "").strip()
        path=(c.get("path") or "").strip()
        line_num=c.get("line") or c.get("original_line")
        if body:
            where = f" ({path}:{line_num})" if path and line_num else (f" ({path})" if path else "")
            lines.append(f"## Inline comment{where}")
            lines.append(body)
            lines.append("")

if not found:
    lines.append("No CodeRabbit comments/reviews found.")

with open(out,'w',encoding='utf-8') as f:
    f.write("\n".join(lines).strip()+"\n")
print(out)
PY
