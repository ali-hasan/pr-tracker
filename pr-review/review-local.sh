#!/bin/bash
set -uo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BASE_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"  # this repo's own root (pr-tracker/)

if [[ ! -f "$SCRIPT_DIR/config.sh" ]]; then
  echo "review-local.sh: $SCRIPT_DIR/config.sh not found — copy config.sh.example to config.sh and fill in CLAUDE_BIN + REPOS_DIR" >&2
  exit 1
fi
CLAUDE_BIN=""
REPOS_DIR=""
source "$SCRIPT_DIR/config.sh"
if [[ -z "$CLAUDE_BIN" || -z "$REPOS_DIR" ]]; then
  echo "review-local.sh: CLAUDE_BIN and REPOS_DIR must both be set in $SCRIPT_DIR/config.sh" >&2
  exit 1
fi

LOG_FILE="$SCRIPT_DIR/run.log"
RUNNING_FILE="$SCRIPT_DIR/RUNNING"

export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:$PATH"

REPO=""
BRANCH=""
BASE_BRANCH=""
LAYER1=true
LAYER2=true
EFFORT="low"
LAYER3=true
NOTES=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --repo) REPO="$2"; shift 2 ;;
    --branch) BRANCH="$2"; shift 2 ;;
    --base) BASE_BRANCH="$2"; shift 2 ;;
    --layer1) LAYER1="$2"; shift 2 ;;
    --layer2) LAYER2="$2"; shift 2 ;;
    --effort) EFFORT="$2"; shift 2 ;;
    --layer3) LAYER3="$2"; shift 2 ;;
    --notes) NOTES="$2"; shift 2 ;;
    *) echo "review-local.sh: unknown arg $1" >> "$LOG_FILE"; shift ;;
  esac
done

if [[ -z "$REPO" || -z "$BRANCH" ]]; then
  echo "review-local.sh: --repo and --branch are required" >> "$LOG_FILE"
  exit 1
fi

if [[ -f "$RUNNING_FILE" ]]; then
  echo "=== Local review for $REPO@$BRANCH skipped: another review is already running ===" >> "$LOG_FILE"
  exit 1
fi

case "$REPO" in
  cioplenu/backend) LOCAL_PATH="$REPOS_DIR/api" ;;
  cioplenu/frontend) LOCAL_PATH="$REPOS_DIR/frontend" ;;
  *) echo "review-local.sh: unknown repo $REPO" >> "$LOG_FILE"; exit 1 ;;
esac

if [[ -z "$BASE_BRANCH" ]]; then
  BASE_BRANCH=$(git -C "$LOCAL_PATH" symbolic-ref refs/remotes/origin/HEAD 2>/dev/null | sed 's@^refs/remotes/origin/@@')
  BASE_BRANCH="${BASE_BRANCH:-develop}"
fi

# Resolving refs here, in bash, rather than leaving it to the prompt — a fetch failure or an
# unpushed branch is a deterministic fact about the repo, not a judgment call to hand to the model.
git -C "$LOCAL_PATH" fetch origin "$BASE_BRANCH" "$BRANCH" >> "$LOG_FILE" 2>&1

PUSHED=false
if git -C "$LOCAL_PATH" rev-parse --verify -q "origin/$BRANCH" > /dev/null 2>&1; then
  BRANCH_REF="origin/$BRANCH"
  PUSHED=true
elif git -C "$LOCAL_PATH" rev-parse --verify -q "$BRANCH" > /dev/null 2>&1; then
  BRANCH_REF="$BRANCH"
else
  echo "review-local.sh: branch $BRANCH not found locally or on origin in $LOCAL_PATH" >> "$LOG_FILE"
  exit 1
fi

if git -C "$LOCAL_PATH" rev-parse --verify -q "origin/$BASE_BRANCH" > /dev/null 2>&1; then
  BASE_REF="origin/$BASE_BRANCH"
else
  BASE_REF="$BASE_BRANCH"
fi

BRANCH_SHA=$(git -C "$LOCAL_PATH" rev-parse "$BRANCH_REF")
REPO_SHORT="${REPO#*/}"
BRANCH_SLUG=$(echo "$BRANCH" | tr '/' '-')

echo "$REPO@$BRANCH" > "$RUNNING_FILE"
echo "=== Local review started: $(date), $REPO@$BRANCH ($BRANCH_REF vs $BASE_REF, pushed=$PUSHED) (layer1=$LAYER1 layer2=$LAYER2:$EFFORT layer3=$LAYER3 notes=${NOTES:-none}) ===" >> "$LOG_FILE"

# Built with printf (%s substitution, never shell re-interpreted) so backticks, apostrophes and
# whatever the user types into --notes can't break quoting or get executed as commands.
LAYERS=""
if [[ "$LAYER1" == true ]]; then
  LAYERS+=$(printf '\n**Layer 1 — repo approval-readiness rubric.** Read and apply %s/.github/skills/code-review/SKILL.md. This decides auto-approvable vs needs-human-review per that repos own stated categories. Keep the reasoning to one short line.\n' "$LOCAL_PATH")
fi
if [[ "$LAYER2" == true ]]; then
  LAYERS+=$(printf '\n**Layer 2 — the `code-review` skill.** Invoke it (`Skill` tool, skill name `code-review`) with args `%s review the diff already gathered in Step 1 above — this is a local branch with no GitHub PR, do not try to fetch one`. Pass `%s` exactly as the effort level, do not change it.\n' "$EFFORT" "$EFFORT")
fi
if [[ "$LAYER3" == true ]]; then
  LAYERS+=$(printf '\n**Layer 3 — your own criteria.** Read %s/pr-review/review-criteria.md and apply anything written there on top of the layers above (it may be empty, skip if so).\n' "$BASE_DIR")
fi
if [[ -n "$NOTES" ]]; then
  LAYERS+=$(printf '\n**Layer 4 — this runs specific ask.** %s Weight this heavily, its what was specifically asked to be looked at on this re-run.\n' "$NOTES")
fi

# report-format.md's line-verification and permalink rules assume a commit that exists on GitHub.
# That's true once a branch is pushed, but an unpushed branch has no GitHub-side content to verify
# against or link to — so this swaps in a local equivalent of both for that case: verify against the
# actual local file via `git show`, and link to an exact line with a `vscode://file/<path>:<line>`
# URL (the same scheme the tracker app's own Report button uses) instead of a GitHub blob permalink.
if [[ "$PUSHED" == true ]]; then
  LINK_INSTRUCTIONS=$(printf 'The branch is pushed to GitHub, so follow report-format.md exactly as written: verify every line against `gh api -H "Accept: application/vnd.github.raw" "repos/%s/contents/<path>?ref=%s"`, and link each citation to `https://github.com/%s/blob/%s/<path>#L<line>`.\n' "$REPO" "$BRANCH_SHA" "$REPO" "$BRANCH_SHA")
else
  LINK_INSTRUCTIONS=$(printf 'This branch has not been pushed, so report-format.md'"'"'s gh-api verification and GitHub permalinks do not apply — there is nothing on GitHub to point at yet. Verify every line instead against the real local file:\n```\ngit -C %s show %s:<path> | grep -n "<unique snippet>"\n```\nand link each citation to `vscode://file/%s/<path>:<line>` instead of a GitHub URL — that opens the exact line locally in VS Code, which is the closest equivalent available before this is pushed.\n' "$LOCAL_PATH" "$BRANCH_REF" "$LOCAL_PATH")
fi

TEMPLATE_FILE=$(mktemp)
# One trap covering both cleanup jobs — a second `trap ... EXIT` call doesn't add to the first,
# it silently replaces it, which is exactly what broke RUNNING-lock cleanup once before.
trap 'rm -f "$RUNNING_FILE" "$TEMPLATE_FILE"' EXIT

# Single-quoted heredoc: nothing in here is expanded by bash, so the backticks and $ signs used
# for markdown code formatting stay literal instead of being run as commands.
cat <<'TEMPLATE_EOF' > "$TEMPLATE_FILE"
You are reviewing a local git branch that has no GitHub PR yet, triggered manually from the tracker
app. No one is watching this run interactively, don't ask questions, make the most reasonable call on
anything ambiguous. Never push, never open a PR, never post anything to GitHub or YouTrack. This is a
read-only, local-report-only workflow — exactly like reviewing a real PR, just sourced from git instead
of `gh`.

Repo: {{REPO}}
Local repo path: {{LOCAL_PATH}}
Branch: {{BRANCH}} (resolved to {{BRANCH_REF}}, currently at {{BRANCH_SHA}})
Base branch: {{BASE_BRANCH}} (resolved to {{BASE_REF}})
Status file: {{BASE_DIR}}/pr-review/status.json
Report output dir: {{BASE_DIR}}/pr-reviews/

## Step 1 — gather context

```
git -C {{LOCAL_PATH}} log {{BASE_REF}}..{{BRANCH_REF}} --format='%H %s%n%b'
git -C {{LOCAL_PATH}} diff {{BASE_REF}}...{{BRANCH_REF}}
```

This is the same three-dot diff `gh pr diff` would show for an equivalent PR against {{BASE_REF}}.

Read `{{BASE_DIR}}/pr-review/report-format.md` now — it has the mandatory rules for verifying
line numbers, building the permalink for every citation, and the exact voice and structure the report
must end up in. Apply all of it to everything below, EXCEPT for line verification and link format,
which follow this instead: {{LINK_INSTRUCTIONS}}

### Find the YouTrack ticket(s)

Search for `DEV-\d+` across the branch name and every commit's headline + body from the `git log` above.
Call `mcp__youtrack__get_issue` for each unique ticket ID found. If none found, say so explicitly.

### Review the diff
{{LAYERS}}
Do NOT check out a different branch than what's already checked out, run a build, or run tests. Do not
post findings anywhere, only write the local report. Collapse every layer above into ONE flat list of
comments.

## Step 2 — write the report

Follow report-format.md's voice and structure exactly, with two differences from a normal PR report:
title the report `# {{REPO_SHORT}} — {{BRANCH}} (local branch, no PR yet)` instead of the usual
`#<number> — <title>` line, and use `https://github.com/{{REPO}}/tree/{{BRANCH}}` in place of the PR
url on the line under it (ticket id/"none found" stays the same). Write both files to
`{{BASE_DIR}}/pr-reviews/{{REPO_SHORT}}-{{BRANCH_SLUG}}.md` and its JSON sidecar
(`{{REPO_SHORT}}-{{BRANCH_SLUG}}.json`), same as a PR review — overwrite if they already exist. In the
JSON sidecar, set `"headRefOid"` to {{BRANCH_SHA}}. Every comment's `"inline"` should be computed the
same way as for a PR: true only if the line falls inside a hunk of the diff from Step 1 — this sidecar
won't actually be publishable from the tracker app (there's no PR to attach a GitHub review to), but
keeping the same shape costs nothing and means it's ready to go the moment this does become a real PR.

## Step 3 — record status, notify, and open the report

1. Merge an entry into status.json (read the existing object first, add/replace this key, write the
   whole object back — don't lose other entries): key it `"{{REPO}}@{{BRANCH}}"` (an `@`, not a `#` —
   that's how the tracker app tells a local-branch review apart from a PR review).
   ```json
   {
     "title": "{{BRANCH}}",
     "url": "https://github.com/{{REPO}}/tree/{{BRANCH}}",
     "outcome": "approve" | "request-changes" | "needs-discussion",
     "reportPath": "{{BASE_DIR}}/pr-reviews/{{REPO_SHORT}}-{{BRANCH_SLUG}}.md",
     "finishedAt": "<ISO 8601 UTC timestamp, from `date -u +%Y-%m-%dT%H:%M:%SZ`>"
   }
   ```
   Derive `outcome` per report-format.md's "Outcome field" section. Do not touch state.json — local
   branch reviews aren't deduped against a stored commit, every run here is a deliberate one-off.
2. Fire a notification and open the report:
   ```
   osascript -e 'display notification "{{REPO_SHORT}}@{{BRANCH}} reviewed" with title "Claude PR Review" sound name "Glass"'
   open "vscode://file/{{BASE_DIR}}/pr-reviews/{{REPO_SHORT}}-{{BRANCH_SLUG}}.md"
   ```
TEMPLATE_EOF

export RL_REPO="$REPO" RL_BRANCH="$BRANCH" RL_BRANCH_REF="$BRANCH_REF" RL_BRANCH_SHA="$BRANCH_SHA" \
  RL_BASE_BRANCH="$BASE_BRANCH" RL_BASE_REF="$BASE_REF" RL_LOCAL_PATH="$LOCAL_PATH" \
  RL_BASE_DIR="$BASE_DIR" RL_REPO_SHORT="$REPO_SHORT" RL_BRANCH_SLUG="$BRANCH_SLUG" \
  RL_LAYERS="$LAYERS" RL_LINK_INSTRUCTIONS="$LINK_INSTRUCTIONS" RL_TEMPLATE_FILE="$TEMPLATE_FILE"

PROMPT=$(python3 <<'PYEOF'
import os
with open(os.environ["RL_TEMPLATE_FILE"]) as f:
    text = f.read()
for name in [
    "REPO", "BRANCH", "BRANCH_REF", "BRANCH_SHA", "BASE_BRANCH", "BASE_REF",
    "LOCAL_PATH", "BASE_DIR", "REPO_SHORT", "BRANCH_SLUG", "LAYERS", "LINK_INSTRUCTIONS",
]:
    text = text.replace("{{" + name + "}}", os.environ["RL_" + name])
print(text)
PYEOF
)

"$CLAUDE_BIN" -p "$PROMPT" \
  --allowedTools "Bash(git -C $REPOS_DIR/api:*)" "Bash(git -C $REPOS_DIR/frontend:*)" "Bash(gh api repos/cioplenu/*)" "Bash(osascript:*)" "Bash(open vscode://*)" "Read" "Write" "mcp__youtrack__get_issue" "Skill" \
  --permission-prompts none \
  >> "$LOG_FILE" 2>&1

echo "=== Local review finished: $(date), $REPO@$BRANCH ===" >> "$LOG_FILE"
